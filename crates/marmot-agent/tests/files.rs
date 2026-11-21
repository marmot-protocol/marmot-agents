//! An agent sends a file to the chat (D-090), end to end through MDK: a person and the agent on
//! MDK's test relay, with a stand-in Blossom server for the encrypted file, and a stand-in harness
//! that asks the core to send the file each prompt names. What the person sees is read back from
//! the person's own storage, and the file is downloaded and decrypted as the person's app would.

mod common;

use common::{World, eventually};
use marmot_agent::carrier::ITEM_KIND;
use marmot_agent_driver::{
    Answer, Capabilities, Control, Driver, Emitted, FilesIn, Harness, MidTurn, Output, Prompt,
    Refusal, SendFile, Sessions,
};
use marmot_agent_events::body::{Body, Message, NoticeKind, StartedBy, Turn};
use marmot_agent_events::{AgentEvent, Header, State};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// A harness that, for each prompt, starts a turn and asks to send the file whose path is the
/// prompt's text; once the core says how it went, it says "sent" or "refused" and ends the turn.
struct Sender {
    out: mpsc::UnboundedReceiver<Output>,
    to_self: mpsc::UnboundedSender<Output>,
    /// The prompt each file request came from.
    asked: HashMap<String, Prompt>,
    /// How each file went, in order.
    results: Arc<Mutex<Vec<Result<(), Refusal>>>>,
    count: u64,
    seq: u64,
}

impl Sender {
    fn new(results: Arc<Mutex<Vec<Result<(), Refusal>>>>) -> Self {
        let (to_self, out) = mpsc::unbounded_channel();
        Self {
            out,
            to_self,
            asked: HashMap::new(),
            results,
            count: 0,
            seq: 0,
        }
    }

    fn emit(&mut self, prompt: &Prompt, n: u64, item: &str, state: State, body: Body, text: &str) {
        self.seq += 1;
        let header = Header {
            harness: "sender".into(),
            session: Some("s1".into()),
            turn: Some(format!("t{n}").into()),
            item: format!("{item}{n}").into(),
            seq: self.seq,
            state,
            title: item.into(),
            text: text.into(),
        };
        let prompts = match body {
            Body::Turn(_) => vec![prompt.message.clone()],
            _ => Vec::new(),
        };
        let _ = self.to_self.send(Output::Event(Emitted {
            event: AgentEvent::new(header, body, None).unwrap(),
            chat: Some(prompt.chat.id.clone()),
            prompts,
            settles: Vec::new(),
        }));
    }
}

fn turn() -> Body {
    Body::Turn(Turn {
        started_by: StartedBy::User,
        duration_ms: None,
        usage: None,
    })
}

impl Driver for Sender {
    fn harness(&self) -> Harness {
        Harness {
            name: "sender".into(),
            version: None,
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            interrupt: false,
            mid_turn: MidTurn::Queue,
            sessions: Sessions::Core,
            resume: false,
            approvals: false,
            files_in: FilesIn::None,
            approval_commands: Vec::new(),
        }
    }

    async fn prompt(&mut self, prompt: Prompt) -> Result<(), Refusal> {
        self.count += 1;
        let n = self.count;
        self.emit(&prompt, n, "turn", State::Running, turn(), "Working");
        let id = format!("call-{n}");
        self.asked.insert(id.clone(), prompt.clone());
        let _ = self.to_self.send(Output::SendFile(SendFile {
            id,
            chat: prompt.chat.id.clone(),
            path: prompt.text.clone().into(),
        }));
        Ok(())
    }

    async fn answer(&mut self, _answer: Answer) -> Result<(), Refusal> {
        Ok(())
    }

    async fn control(&mut self, _control: Control) -> Result<(), Refusal> {
        Ok(())
    }

    async fn next_event(&mut self) -> Option<Output> {
        self.out.recv().await
    }

    async fn file_sent(&mut self, id: String, result: Result<(), Refusal>) {
        let prompt = self
            .asked
            .remove(&id)
            .expect("a file this harness asked for");
        let n: u64 = id.trim_start_matches("call-").parse().unwrap();
        let said = if result.is_ok() { "sent" } else { "refused" };
        self.results.lock().unwrap().push(result);
        let words = format!("{said} {n}");
        let message = Body::Message(Message::default());
        self.emit(&prompt, n, "words", State::Done, message, &words);
        self.emit(&prompt, n, "turn", State::Done, turn(), "Done");
    }
}

/// A stand-in Blossom server: it keeps each uploaded blob under its hash and serves it back.
async fn blossom() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let blobs: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::default();
    let base = url.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let blobs = blobs.clone();
            let base = base.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0u8; 8192];
                let end = loop {
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                };
                let head = String::from_utf8_lossy(&request[..end]).to_string();
                let length: usize = head
                    .lines()
                    .find_map(|l| {
                        let (name, value) = l.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().ok())?
                    })
                    .unwrap_or(0);
                while request.len() < end + length {
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let body = request[end..end + length].to_vec();
                let mut words = head.split_whitespace();
                let (method, path) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
                let (status, kind, reply) = match method {
                    "PUT" => {
                        let hash = hex::encode(Sha256::digest(&body));
                        let descriptor = serde_json::json!({
                            "url": format!("{base}/{hash}.bin"), "sha256": hash,
                            "size": body.len(), "type": "application/octet-stream", "uploaded": 1,
                        });
                        blobs.lock().unwrap().insert(hash, body);
                        (201, "application/json", descriptor.to_string().into_bytes())
                    }
                    "GET" => {
                        let hash = path.trim_start_matches('/').split('.').next().unwrap_or("");
                        match blobs.lock().unwrap().get(hash) {
                            Some(blob) => (200, "application/octet-stream", blob.clone()),
                            None => (404, "text/plain", b"not found".to_vec()),
                        }
                    }
                    _ => (404, "text/plain", b"not found".to_vec()),
                };
                let head = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    reply.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(&reply).await;
            });
        }
    });
    url
}

// The agent sends a file from its working folder: the person receives it as an ordinary chat
// message carrying an encrypted attachment, tagged with the turn, and can download and read it.
// A file outside the folder, and a link in the folder that leads out of it, are refused: the
// person sees a refused notice that names no path, and the harness hears why.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_agent_sends_a_file_from_its_folder_and_nothing_from_outside() {
    let work = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("notes.txt"), "hello from the agent\n").unwrap();
    std::fs::write(outside.path().join("secret.txt"), "not for the chat\n").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        work.path().join("shortcut.txt"),
    )
    .unwrap();
    let settings = format!("[codex]\ncwd = \"{}\"\n", work.path().display());
    let mut world = World::new(&settings).await;
    world.marmot.media_server = Some(blossom().await);
    world.runtime.start().await.unwrap();
    let results = Arc::new(Mutex::new(Vec::new()));
    let core = world.start(Sender::new(results.clone()));
    let group = world
        .runtime
        .start_direct_conversation(&world.person, &world.agent)
        .await
        .unwrap();
    let group_hex = world.joined(&group).await;

    let ask = |path: &std::path::Path| path.display().to_string().into_bytes();
    let say = |text: Vec<u8>| world.runtime.send_message(&world.person, &group, text);
    say(ask(&work.path().join("notes.txt"))).await.unwrap();
    world.words(&group_hex, "sent 1").await;
    say(ask(&outside.path().join("secret.txt"))).await.unwrap();
    world.words(&group_hex, "refused 2").await;
    say(ask(&work.path().join("shortcut.txt"))).await.unwrap();
    world.words(&group_hex, "refused 3").await;

    // The file: one chat message from the agent with an attachment, tagged with its turn.
    let with_file: Vec<_> = world
        .sent_by_agent(&group_hex, 9)
        .into_iter()
        .filter(|m| m.tags.iter().any(|t| t[0] == "imeta"))
        .collect();
    assert_eq!(with_file.len(), 1, "only the file in the folder was sent");
    let message = &with_file[0];
    let turn_tag = message
        .tags
        .iter()
        .find(|t| t[0] == "turn")
        .expect("a turn");
    assert_eq!(turn_tag[1], "t1", "tagged with the turn it was sent in");
    let imeta = message.tags.iter().find(|t| t[0] == "imeta").unwrap();
    let reference =
        marmot_app::media_attachment_from_imeta_tag(imeta, message.source_epoch, true).unwrap();
    assert_eq!(reference.file_name, "notes.txt");
    assert_eq!(reference.media_type, "text/plain");
    let downloaded = world
        .runtime
        .download_media(&world.person, &group, reference)
        .await
        .unwrap();
    assert_eq!(downloaded.plaintext, b"hello from the agent\n");

    // The refusals: the harness heard why, and the person saw it, without a path.
    let results = results.lock().unwrap().clone();
    assert!(results[0].is_ok());
    for refused in &results[1..] {
        let refusal = refused.as_ref().unwrap_err();
        assert_eq!(refusal.reason, marmot_agent_driver::Reason::NotAllowed);
    }
    let notices = eventually(|| {
        let notices: Vec<_> = world
            .items(&group_hex)
            .into_iter()
            .map(|(_, read)| read.item.event)
            .filter(|e| matches!(e.body(), Body::Notice(n) if n.kind == NoticeKind::Refused))
            .collect();
        (notices.len() == 2).then_some(notices)
    })
    .await;
    let private = [
        outside.path().display().to_string(),
        work.path().display().to_string(),
        "secret".into(),
        "shortcut".into(),
    ];
    for notice in &notices {
        let Body::Notice(body) = notice.body() else {
            unreachable!()
        };
        assert_eq!(body.reason.as_deref(), Some("not_allowed"));
        let shown = serde_json::to_string(notice).unwrap();
        for value in &private {
            assert!(!shown.contains(value.as_str()), "a notice names {value}");
        }
    }
    let everything: Vec<String> = world
        .sent_by_agent(&group_hex, 9)
        .into_iter()
        .chain(world.sent_by_agent(&group_hex, ITEM_KIND))
        .map(|m| format!("{} {:?}", m.plaintext, m.tags))
        .collect();
    assert!(
        everything.iter().all(|m| !m.contains("not for the chat")),
        "nothing from outside reached the chat"
    );

    core.stop().await;
    world.runtime.shutdown().await;
}
