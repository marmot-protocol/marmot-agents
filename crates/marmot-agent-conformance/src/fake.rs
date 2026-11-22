//! The engine every fake harness runs on, and what a fake remembers hearing.
//!
//! A fake harness is a [`Protocol`]: it reads one frame the driver wrote, and answers with frames
//! of its own, in the shapes the recordings show. The engine reads the driver's lines, hands each
//! to the protocol, and writes what it answers, with the faults the [`Knobs`] ask for: frames
//! written in pieces, replies late, replies after the notifications that follow them and out of
//! order between requests, requests never answered (or answered only much later), a harness that
//! stops reading its input, and a harness that dies after a given number of frames or when told
//! to. Every line the driver writes is kept, and a line that is not whole JSON is kept apart: a
//! driver must never leave half a message on the harness's input (S11). What is left unfinished
//! when the driver closes the input is not a line: a harness never reads it as a message.
//!
//! What a fake understood is kept as [`Heard`], the same for every harness, so the suite checks
//! what reached the harness without knowing its protocol.

pub mod acp;
pub mod codex;
mod mcp;
pub mod pi;
mod replay;

pub use replay::{Progress, Replay};

use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

/// The faults a fake harness puts in what it writes and how it answers. They are read for every
/// frame, so a scenario can change them between steps.
#[derive(Clone, Debug, Default)]
pub struct Knobs {
    /// Every frame is written in three pieces, with a pause between them.
    pub partial: bool,
    /// Every reply to a request comes this late. Notifications are not held back.
    pub late: Option<Duration>,
    /// Everything the harness says comes this late, in order.
    pub slow: Option<Duration>,
    /// Requests and notifications of these methods (or Pi types) are held, unheard and
    /// unanswered, for as long as they are listed here. Taken off the list, a held frame is heard
    /// and answered then, late, as a harness that was stuck would.
    pub never: Vec<String>,
    /// The harness stops reading its input, and keeps it open, until this is cleared. The driver's
    /// writes stop once the pipe is full.
    pub deaf: bool,
    /// When set, only the harness process started with this tag is deaf; the others read on.
    pub deaf_only: Option<String>,
    /// A reply comes after the notifications that follow from its request, and every other reply
    /// is held back a little, so a later request's reply overtakes it.
    pub reorder: bool,
    /// The harness dies on reading the n-th frame from the driver, counted over every process of
    /// the fake ([`Fake::frames`]), before it answers that frame.
    pub die_after: Option<usize>,
}

/// What a fake harness understood the driver to say, the same for every harness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    /// A session opened: a new one, or one taken up again by its id, in the folder the harness
    /// was told to work in, when it says (D-101).
    Opened {
        session: String,
        resumed: bool,
        folder: Option<String>,
    },
    /// A prompt's text, in a session: a new turn, or one added to a running turn. `refused` when
    /// the harness turned it away, as when the session is busy: it reached the harness all the
    /// same, so a copy refused still counts against S4.
    Prompt {
        session: String,
        text: String,
        refused: bool,
    },
    /// An answer to an approval (allowed or not) or a question (the text typed or chosen), in the
    /// session that asked.
    Answer {
        session: String,
        allow: Option<bool>,
        text: Option<String>,
    },
    /// What the driver's own tool to send a file gave back, in the session that called it: whether
    /// it was sent, and the words the model reads.
    FileResult {
        session: String,
        sent: bool,
        text: String,
    },
    /// A control: `interrupt`, `new_session` or `set`.
    Control {
        session: Option<String>,
        what: String,
    },
}

/// One thing a protocol answers with.
#[derive(Debug)]
pub enum Out {
    /// A reply to the driver's request: the knobs can hold it back.
    Reply(Value),
    /// Anything else the harness says, in order.
    Say(Value),
    /// The harness dies here, after what it said before.
    Die,
}

/// What a protocol answers one frame with.
#[derive(Debug, Default)]
pub struct Outbox {
    items: Vec<Out>,
}

impl Outbox {
    /// A reply to a request.
    pub fn reply(&mut self, message: Value) {
        self.items.push(Out::Reply(message));
    }

    /// A notification, or a request of the harness's own.
    pub fn say(&mut self, message: Value) {
        self.items.push(Out::Say(message));
    }

    /// The harness dies once what came before is written.
    pub fn die(&mut self) {
        self.items.push(Out::Die);
    }
}

/// What a prompt asks a fake harness to do, by a word in its text. Every fake reads the same
/// words, so one scenario drives every harness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    /// Answers in words, and the turn ends.
    Reply,
    /// Asks for approval before a command (Pi, which has no approvals, asks a question), and goes
    /// on once answered.
    Ask,
    /// Asks a question with two options, and goes on once answered.
    Quiz,
    /// Runs a command until the turn is interrupted.
    Sleep,
    /// Starts a command, then dies, as a crash would.
    Die,
    /// Calls the driver's `send_file` tool with [`file_for`] the prompt, and goes on once it has
    /// the result. A harness the driver gives no such tool answers in words.
    File,
}

impl Script {
    /// The words, as a prompt carries them.
    pub const WORDS: [(&str, Script); 5] = [
        ("#ask", Script::Ask),
        ("#quiz", Script::Quiz),
        ("#sleep", Script::Sleep),
        ("#die", Script::Die),
        ("#file", Script::File),
    ];

    /// What a prompt's text asks for.
    pub fn of(text: &str) -> Self {
        (Self::WORDS.iter())
            .find(|(word, _)| text.contains(word))
            .map_or(Script::Reply, |(_, script)| *script)
    }
}

/// The relative path a fake asks the driver to send for a prompt: it carries the prompt's chat mark
/// and message id, its first two words, so a file handed to the wrong chat shows it.
pub fn file_for(prompt: &str) -> String {
    let mut words = prompt.split_whitespace();
    let mark = words.next().unwrap_or_default();
    let message = words.next().unwrap_or_default();
    format!("out/{mark} {message}.txt")
}

static NEXT: AtomicU64 = AtomicU64::new(1);

/// An id no fake has made before in this test program, as a harness's own ids are: a Codex turn or
/// an ACP session is never named again after a restart.
pub fn fresh(kind: &str) -> String {
    format!("{kind}-{:06}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// A harness's side of the conversation: what it answers to each frame from the driver.
pub trait Protocol: Send + 'static {
    /// What the harness says as it starts, before it reads anything.
    fn start(&mut self, out: &mut Outbox, heard: &mut Vec<Heard>) {
        let _ = (out, heard);
    }

    /// Answers one frame from the driver, and notes what it understood.
    fn hear(&mut self, frame: &Value, out: &mut Outbox, heard: &mut Vec<Heard>);

    /// The harness has ended, by dying or because its input closed: what was running is cut off.
    fn ended(&mut self) {}

    /// What the engine waits on, besides the driver's frames, for something the harness does off
    /// the pipe: an ACP agent's call of an MCP tool over HTTP. When it is notified, the engine
    /// calls [`Protocol::woken`].
    fn waker(&self) -> Option<Arc<tokio::sync::Notify>> {
        None
    }

    /// Something the harness did off the pipe has come back: it answers as it would to a frame.
    fn woken(&mut self, out: &mut Outbox, heard: &mut Vec<Heard>) {
        let _ = (out, heard);
    }
}

impl Protocol for Box<dyn Protocol> {
    fn start(&mut self, out: &mut Outbox, heard: &mut Vec<Heard>) {
        (**self).start(out, heard);
    }

    fn hear(&mut self, frame: &Value, out: &mut Outbox, heard: &mut Vec<Heard>) {
        (**self).hear(frame, out, heard);
    }

    fn ended(&mut self) {
        (**self).ended();
    }

    fn waker(&self) -> Option<Arc<tokio::sync::Notify>> {
        (**self).waker()
    }

    fn woken(&mut self, out: &mut Outbox, heard: &mut Vec<Heard>) {
        (**self).woken(out, heard);
    }
}

/// A frame's kind: its JSON-RPC method, or its Pi record type, or `reply` for a JSON-RPC reply.
pub fn key(frame: &Value) -> String {
    frame["method"]
        .as_str()
        .or(frame["type"].as_str())
        .unwrap_or("reply")
        .to_string()
}

/// Whether a frame is a reply to a request: a JSON-RPC reply, or a Pi response.
pub fn is_reply(frame: &Value) -> bool {
    (frame.get("id").is_some() && frame.get("method").is_none() && frame.get("type").is_none())
        || frame["type"] == "response"
}

/// One thing a harness process did, by its tag: read a line, or died.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Did {
    Read(Option<String>),
    Died(Option<String>),
}

#[derive(Default)]
struct Shared {
    knobs: Knobs,
    heard: Vec<Heard>,
    frames: usize,
    malformed: Vec<String>,
    deaths: usize,
    /// The tag of each process that died, not yet taken.
    dead: Vec<Option<String>>,
    /// Every line read and every death, in order, with the tag of the process.
    log: Vec<Did>,
}

/// How long a killed harness may go on writing what it had said before its output is cut off. A
/// driver that stops reading leaves the harness blocked in a write; a kill still ends it.
const KILL_GRACE: Duration = Duration::from_millis(100);

/// Which running harnesses a kill is for: all of them, or those serving one tag (Pi's chat key).
#[derive(Clone, Debug, Default)]
struct Kill {
    count: u64,
    tag: Option<String>,
}

/// A fake harness: the knobs, what it heard, and every process it runs. Cloned handles share all
/// of it, across restarts of the driver.
#[derive(Clone)]
pub struct Fake {
    shared: Arc<Mutex<Shared>>,
    kill: Arc<watch::Sender<Kill>>,
    /// Bumped whenever the knobs change, so a harness waiting to read sees it at once.
    turned: Arc<watch::Sender<u64>>,
}

impl Default for Fake {
    fn default() -> Self {
        Self::new()
    }
}

impl Fake {
    /// A fake that has heard nothing yet, with no faults.
    pub fn new() -> Self {
        let (kill, _) = watch::channel(Kill::default());
        let (turned, _) = watch::channel(0);
        Self {
            shared: Arc::default(),
            kill: Arc::new(kill),
            turned: Arc::new(turned),
        }
    }

    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().expect("the fake's state")
    }

    /// The faults in force.
    pub fn knobs(&self) -> Knobs {
        self.shared().knobs.clone()
    }

    /// Changes the faults from the next frame on.
    pub fn set_knobs(&self, knobs: Knobs) {
        self.shared().knobs = knobs;
        self.turned.send_modify(|n| *n += 1);
    }

    /// Everything understood so far, in order.
    pub fn heard(&self) -> Vec<Heard> {
        self.shared().heard.clone()
    }

    /// How many lines the driver has written, whole or not.
    pub fn frames(&self) -> usize {
        self.shared().frames
    }

    /// The lines the driver wrote that were not whole JSON.
    pub fn malformed(&self) -> Vec<String> {
        self.shared().malformed.clone()
    }

    /// How many harness processes have died, by a kill, a knob or a script.
    pub fn deaths(&self) -> usize {
        self.shared().deaths
    }

    /// Where the record of lines read and deaths stands now, for [`Fake::died_reading`].
    pub fn mark(&self) -> usize {
        self.shared().log.len()
    }

    /// Whether, since `mark`, a harness process that `mine` accepts by its tag read a line and
    /// then died: it may have died reading what the driver wrote, and cannot say what it heard.
    /// A process that died, or read, for another chat does not count.
    pub fn died_reading(&self, mark: usize, mine: impl Fn(Option<&str>) -> bool) -> bool {
        // Copied, so `mine` runs with the fake unlocked.
        let since = self.shared().log.get(mark..).unwrap_or_default().to_vec();
        since.iter().enumerate().any(|(at, did)| match did {
            Did::Read(tag) if mine(tag.as_deref()) => since[at..]
                .iter()
                .any(|later| *later == Did::Died(tag.clone())),
            _ => false,
        })
    }

    /// The processes that died since this was last asked, by their tags.
    pub fn take_dead(&self) -> Vec<Option<String>> {
        std::mem::take(&mut self.shared().dead)
    }

    /// Every running harness process dies now.
    pub fn kill(&self) {
        self.kill.send_modify(|k| {
            k.count += 1;
            k.tag = None;
        });
    }

    /// The harness processes serving `tag` die now.
    pub fn kill_tagged(&self, tag: &str) {
        self.kill.send_modify(|k| {
            k.count += 1;
            k.tag = Some(tag.to_string());
        });
    }

    /// Waits until what was heard passes `done`, for at most `within`.
    pub async fn wait_heard(&self, within: Duration, done: impl Fn(&[Heard]) -> bool) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let heard = self.heard();
            if done(&heard) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    /// Runs one harness process: reads the driver's frames from `reader`, and writes what
    /// `protocol` answers to `writer`. A `tag` lets [`Fake::kill_tagged`] reach it.
    pub fn serve<P: Protocol>(
        &self,
        reader: impl AsyncRead + Unpin + Send + 'static,
        writer: impl AsyncWrite + Unpin + Send + 'static,
        protocol: P,
        tag: Option<String>,
    ) -> JoinHandle<()> {
        let fake = self.clone();
        let kill = self.kill.subscribe();
        tokio::spawn(engine(reader, writer, protocol, fake, tag, kill))
    }
}

enum Write {
    Line(Value),
    Die,
}

async fn engine<P: Protocol>(
    reader: impl AsyncRead + Unpin + Send + 'static,
    writer: impl AsyncWrite + Unpin + Send + 'static,
    mut protocol: P,
    fake: Fake,
    tag: Option<String>,
    mut kill: watch::Receiver<Kill>,
) {
    kill.mark_unchanged();
    let mut turned = fake.turned.subscribe();
    let (tx, rx) = mpsc::unbounded_channel();
    let mut writes = tokio::spawn(write_loop(writer, rx, fake.clone()));
    let mut reader = BufReader::new(reader);
    // One line as it is read; kept across a wait given up on, so nothing of it is lost.
    let mut line = Vec::new();
    let mut replies = 0usize;
    let mut out = Outbox::default();
    let mut heard = Vec::new();
    protocol.start(&mut out, &mut heard);
    fake.shared().heard.extend(heard);
    let mut dead = dispatch(out, &fake.knobs(), &tx, &mut replies);
    // Frames held by `Knobs::never`, to be heard once taken off it.
    let mut held: Vec<Value> = Vec::new();
    let mut killed = false;
    let waker = protocol.waker();
    'process: while !dead {
        turned.borrow_and_update();
        let knobs = fake.knobs();
        // A held frame taken off the list is heard now.
        let (now, still): (Vec<Value>, Vec<Value>) =
            held.drain(..).partition(|f| !knobs.never.contains(&key(f)));
        held = still;
        for frame in now {
            dead = hear(&mut protocol, &frame, &fake, &knobs, &tx, &mut replies);
            if dead {
                break 'process;
            }
        }
        let woken = async {
            match &waker {
                Some(waker) => waker.notified().await,
                None => std::future::pending().await,
            }
        };
        let read = tokio::select! {
            read = reader.read_until(b'\n', &mut line), if !deaf(&knobs, tag.as_deref()) => read,
            _ = turned.changed() => continue,
            () = woken => {
                let mut out = Outbox::default();
                let mut heard = Vec::new();
                protocol.woken(&mut out, &mut heard);
                fake.shared().heard.extend(heard);
                dead = dispatch(out, &knobs, &tx, &mut replies);
                continue;
            }
            changed = kill.changed() => {
                if changed.is_err() {
                    continue;
                }
                let k = kill.borrow_and_update().clone();
                if k.tag.is_none() || k.tag == tag {
                    let _ = tx.send(Write::Die);
                    dead = true;
                    killed = true;
                    break 'process;
                }
                continue;
            }
        };
        // The harness's input closed, with what was unfinished never a line: it exits, and its
        // output closes.
        if !matches!(read, Ok(n) if n > 0) || line.last() != Some(&b'\n') {
            break;
        }
        let text = String::from_utf8_lossy(&line)
            .trim_end_matches(['\n', '\r'])
            .to_string();
        line.clear();
        let total = {
            let mut shared = fake.shared();
            shared.frames += 1;
            shared.log.push(Did::Read(tag.clone()));
            shared.frames
        };
        let Ok(frame) = serde_json::from_str::<Value>(&text) else {
            fake.shared().malformed.push(text);
            continue;
        };
        let knobs = fake.knobs();
        if knobs.die_after == Some(total) {
            let _ = tx.send(Write::Die);
            dead = true;
            break;
        }
        if knobs.never.contains(&key(&frame)) {
            held.push(frame);
            continue;
        }
        dead = hear(&mut protocol, &frame, &fake, &knobs, &tx, &mut replies);
    }
    protocol.ended();
    drop(tx);
    drop(reader);
    if killed {
        // A process killed in the middle of a write that the driver no longer reads is cut off
        // there; one that dies by its own script writes what it said first.
        if tokio::time::timeout(KILL_GRACE, &mut writes).await.is_err() {
            writes.abort();
            let _ = writes.await;
        }
    } else {
        let _ = writes.await;
    }
    // Counted once both ends are closed, so a driver that writes after a death is seen counts
    // as writing to a harness that has gone.
    if dead {
        let mut shared = fake.shared();
        shared.deaths += 1;
        shared.log.push(Did::Died(tag.clone()));
        shared.dead.push(tag);
    }
}

/// Whether the harness process started with `tag` has stopped reading.
fn deaf(knobs: &Knobs, tag: Option<&str>) -> bool {
    knobs.deaf && (knobs.deaf_only.is_none() || knobs.deaf_only.as_deref() == tag)
}

/// Hands one frame to the protocol, and what it answers to the writer. True if the harness died.
fn hear<P: Protocol>(
    protocol: &mut P,
    frame: &Value,
    fake: &Fake,
    knobs: &Knobs,
    tx: &mpsc::UnboundedSender<Write>,
    replies: &mut usize,
) -> bool {
    let mut out = Outbox::default();
    let mut heard = Vec::new();
    protocol.hear(frame, &mut out, &mut heard);
    fake.shared().heard.extend(heard);
    dispatch(out, knobs, tx, replies)
}

/// Hands what a protocol answered to the writer, as the knobs say. True if the harness died.
fn dispatch(
    out: Outbox,
    knobs: &Knobs,
    tx: &mpsc::UnboundedSender<Write>,
    replies: &mut usize,
) -> bool {
    let mut items = out.items;
    if knobs.reorder {
        // Replies after what follows from them; the order among each kind is kept.
        items.sort_by_key(|item| matches!(item, Out::Reply(_)));
    }
    for item in items {
        match item {
            Out::Say(message) => {
                let _ = tx.send(Write::Line(message));
            }
            Out::Reply(message) => {
                *replies += 1;
                let mut delay = knobs.late.unwrap_or_default();
                if knobs.reorder && *replies % 2 == 1 {
                    delay += Duration::from_millis(20);
                }
                if delay.is_zero() {
                    let _ = tx.send(Write::Line(message));
                } else {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let _ = tx.send(Write::Line(message));
                    });
                }
            }
            Out::Die => {
                let _ = tx.send(Write::Die);
                return true;
            }
        }
    }
    false
}

async fn write_loop(
    mut writer: impl AsyncWrite + Unpin + Send + 'static,
    mut rx: mpsc::UnboundedReceiver<Write>,
    fake: Fake,
) {
    while let Some(write) = rx.recv().await {
        let knobs = fake.knobs();
        let partial = knobs.partial;
        if let Some(slow) = knobs.slow {
            tokio::time::sleep(slow).await;
        }
        let written = match write {
            Write::Line(message) => {
                let line = format!("{message}\n");
                write_line(&mut writer, line.as_bytes(), partial).await
            }
            Write::Die => {
                if partial {
                    // Dies in the middle of a frame.
                    let _ = writer.write_all(br#"{"jsonrpc":"2.0","met"#).await;
                    let _ = writer.flush().await;
                }
                break;
            }
        };
        if written.is_err() {
            break;
        }
    }
    let _ = writer.shutdown().await;
}

async fn write_line(
    writer: &mut (impl AsyncWrite + Unpin),
    bytes: &[u8],
    partial: bool,
) -> std::io::Result<()> {
    if !partial || bytes.len() < 3 {
        writer.write_all(bytes).await?;
        return writer.flush().await;
    }
    let third = bytes.len() / 3;
    for piece in [
        &bytes[..third],
        &bytes[third..2 * third],
        &bytes[2 * third..],
    ] {
        writer.write_all(piece).await?;
        writer.flush().await?;
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    Ok(())
}

/// The driver's end of the harness's input, through which a deliberate fault can write as if the
/// driver had ([`Injector`](crate::faults::Injector)). What is injected goes before the driver's
/// next bytes.
pub struct Tap<W> {
    inner: W,
    pending: Arc<Mutex<Vec<u8>>>,
}

/// What a [`Tap`] writes before the driver's next bytes, shared with the injector that fills it.
pub type Pending = Arc<Mutex<Vec<u8>>>;

impl<W> Tap<W> {
    /// Wraps a writer; what is put in `pending` goes first.
    pub fn new(inner: W, pending: Pending) -> Self {
        Self { inner, pending }
    }
}

/// What injects into every [`Tap`] made with `pending`.
pub fn injector(pending: &Pending) -> crate::faults::Injector {
    let pending = pending.clone();
    Arc::new(move |bytes: &[u8]| pending.lock().expect("the tap").extend_from_slice(bytes))
}

impl<W: AsyncWrite + Unpin> AsyncWrite for Tap<W> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        use std::task::Poll;
        let this = &mut *self;
        loop {
            let mut pending = this.pending.lock().expect("the tap");
            if pending.is_empty() {
                break;
            }
            match std::pin::Pin::new(&mut this.inner).poll_write(cx, &pending) {
                Poll::Ready(Ok(n)) => {
                    pending.drain(..n);
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }
        std::pin::Pin::new(&mut this.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
