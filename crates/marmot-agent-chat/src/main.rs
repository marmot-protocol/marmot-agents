//! `marmot-agent-chat`: a terminal client that plays the person, for trying the core by hand.
//!
//! It holds one person's account, talks to the agent in a Marmot group, shows each of the agent's
//! items as it changes, and sends answers and controls. How it shows an item is a plain reference
//! for the apps. When an item's first version, kind 1201, or a later one, MDK's edit of it, arrives,
//! it shows the item as MDK keeps it, one row at its first event with its latest version; and it
//! sends answers and controls as kind 1202 (D-076). How to try the core with it is in
//! `docs/try.md`.
//!
//! Each line from a chat starts with the chat's label, which names its group and the agent in it:
//! two agents in one group are two chats. A waiting item shows its choices or options, with their
//! ids, and how it is answered, whichever chat it is in; the shortcuts answer only in the chat
//! being talked in, so one from another chat says how to switch to it first, and switching shows
//! again what waits there.
//!
//! Typed lines are messages, sent exactly as typed, slash commands included (D-031). Lines that
//! start with `:` are for this client:
//!
//! ```text
//! :invite <key>          start a direct chat with the agent
//! :group <name> <key>    make a named group with the agent in it
//! :chat <label>          talk in the chat whose lines start with that label, and show
//!                        what waits in it
//! :allow   :deny         answer the latest approval waiting in this chat with its one choice
//!                        that allows, or denies; with more than one, :choose
//! :choose <id>           answer it with the choice of that id
//! :answer <text>         answer the question waiting in this chat with text
//! :pick <id>             answer it with the option of that id
//!                        (a question answered by a plain message takes neither: type it,
//!                        in a group mentioning the agent; a secret only where you two are
//!                        alone, else in the harness's terminal)
//! :stop                  interrupt the running turn
//! :new                   start a new session
//! :set <id> <value>      change one of the session's settings
//! :quit
//! ```

use cgka_traits::{GroupId, TransportEndpoint};
use marmot_account::AccountHome;
use marmot_agent::carrier::{self, Asked, AskedReply, EDIT_KIND, ITEM_KIND, Read};
use marmot_agent::{EventKey, PubKey};
use marmot_agent_events::State;
use marmot_agent_events::body::{
    AnswerVia, Approval, Asked as AskedOne, Body, Choice, Effect, Question,
};
use marmot_app::{
    AccountSetupRequest, MarmotApp, MarmotAppConfig, MarmotAppEvent, MarmotAppRuntime,
};
use std::collections::HashMap;
use std::process::ExitCode;
use tokio::io::{AsyncBufReadExt, BufReader};

/// The kind of a chat message.
const CHAT: u64 = 9;
/// A NIP-22 comment: a subagent's words, in its thread (D-122).
const COMMENT: u64 = 1111;

struct Client {
    runtime: MarmotAppRuntime,
    app: MarmotApp,
    me: String,
    /// The person's account, as MDK labels it.
    label: String,
    /// The chat being talked in, and the agent in it.
    chat: Option<(GroupId, PubKey)>,
    /// Each item's latest version, as the chat shows it.
    items: Items,
}

/// Each item's latest version, by its first event, with where it came from and when.
#[derive(Default)]
struct Items {
    known: HashMap<EventKey, Seen>,
    /// How many item versions have arrived, to tell the latest.
    arrived: u64,
}

/// An item as MDK keeps it: one row at its first event, holding the latest version its sender
/// made, an edit that came before its first event held until it does
/// (`TimelineMessageRecord::agent_item`).
struct Row {
    sender: String,
    plaintext: String,
    tags: Vec<Vec<String>>,
}

/// What a chat is, for how to answer in it: a direct chat, where the agent hears every message,
/// or a group, where it hears only one that mentions it or replies to it; and how many are in it,
/// as a secret is answered only where two are. `None` when MDK could not say.
#[derive(Clone, Copy, Debug, Default)]
struct Place {
    direct: bool,
    members: Option<usize>,
}

impl Place {
    /// Whether only the person and the agent are in it.
    fn two(&self) -> bool {
        self.members == Some(2)
    }
}

/// An item's latest version, and where and when it arrived.
struct Seen {
    read: Read,
    group: String,
    sender: String,
    order: u64,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(home) = args.get(1) else {
        eprintln!("usage: marmot-agent-chat <home> (--serve <port> | --relay <url>)");
        return ExitCode::from(2);
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(4 * 1024 * 1024)
        .build()
        .expect("the async runtime starts");
    match runtime.block_on(run(home, &args[2..])) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("marmot-agent-chat: {why}");
            ExitCode::FAILURE
        }
    }
}

async fn run(home: &str, options: &[String]) -> Result<(), String> {
    let (url, _relay) = match options {
        [flag, port] if flag == "--serve" => {
            let port: u16 = port.parse().map_err(|_| "--serve takes a port")?;
            let builder = nostr_relay_builder::RelayBuilder::default().port(port);
            let relay = nostr_relay_builder::LocalRelay::new(builder);
            relay.run().await.map_err(|_| "the relay could not start")?;
            let url = relay.url().await.to_string();
            println!("relay: {url}");
            (url, Some(relay))
        }
        [flag, url] if flag == "--relay" => (url.clone(), None),
        _ => return Err("give --serve <port> or --relay <url>".into()),
    };
    marmot_agent::marmot::start_sqlite();
    let account_home = AccountHome::open(home);
    let settings = MarmotAppConfig::default().with_allow_loopback_relay_endpoints(true);
    let app = MarmotApp::try_with_relays_and_account_home_and_config(
        home,
        vec![url.clone()],
        account_home.clone(),
        settings,
    )
    .map_err(|_| "the home could not be opened; is another client using it?")?;
    let runtime = MarmotAppRuntime::new(app.clone());
    let existing = account_home.accounts().unwrap_or_default();
    let (me, label) = match existing.into_iter().find(|a| a.local_signing) {
        Some(account) => (account.account_id_hex, account.label),
        None => {
            let endpoint = TransportEndpoint(url.clone());
            let setup = AccountSetupRequest {
                default_relays: vec![endpoint.clone()],
                bootstrap_relays: vec![endpoint],
                publish_initial_key_package: true,
                ..AccountSetupRequest::default()
            };
            let made = runtime
                .create_identity(setup)
                .await
                .map_err(|_| "the person's identity could not be made")?;
            (made.account.account_id_hex, made.account.label)
        }
    };
    println!("you: {me}");
    let mut events = runtime.subscribe();
    runtime.start().await.map_err(|_| "MDK could not start")?;
    let mut client = Client {
        runtime: runtime.clone(),
        app,
        me,
        label,
        chat: None,
        items: Items::default(),
    };
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    if line.trim() == ":quit" {
                        break;
                    }
                    if let Err(why) = client.typed(line.trim()).await {
                        println!("! {why}");
                    }
                }
                _ => break,
            },
            event = events.recv() => {
                if let Ok(MarmotAppEvent::MessageReceived(received)) = event
                    && received.account_id_hex == client.me
                {
                    client.received(received.message).await;
                }
            }
        }
    }
    runtime.shutdown().await;
    Ok(())
}

impl Client {
    async fn typed(&mut self, line: &str) -> Result<(), String> {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            [] => Ok(()),
            [":invite", key] => {
                let key = marmot_app::account_id_hex_from_ref(key).map_err(|_| "not a key")?;
                let group = self
                    .runtime
                    .start_direct_conversation(&self.me, &key)
                    .await
                    .map_err(|_| "the agent could not be invited; is it on the relay?")?;
                self.chat = Some((group, PubKey(key)));
                println!("· invited; waiting for the agent to accept");
                Ok(())
            }
            [":group", name, key] => {
                let key = marmot_app::account_id_hex_from_ref(key).map_err(|_| "not a key")?;
                let made = self
                    .runtime
                    .create_group(&self.me, name, std::slice::from_ref(&key), None)
                    .await
                    .map_err(|_| "the group could not be made")?;
                let group = made.operation.ok_or("the agent could not be added")?;
                let npub = marmot_app::npub_for_account_id(&key).unwrap_or_default();
                self.chat = Some((group, PubKey(key)));
                println!("· group made; address the agent by writing nostr:{npub}");
                Ok(())
            }
            [":chat", label] => {
                let (hex_id, agent) = self.items.chat(label)?;
                let group = GroupId::new(hex::decode(&hex_id).map_err(|_| "not a chat")?);
                let place = self.place(&group).await;
                println!("· talking in {label}");
                for line in self.items.waiting_lines(&hex_id, &agent, place) {
                    println!("{line}");
                }
                self.chat = Some((group, PubKey(agent)));
                Ok(())
            }
            [":allow"] => self.approve(|a| only(a, Effect::Allow)).await,
            [":deny"] => self.approve(|a| only(a, Effect::Deny)).await,
            [":choose", id] => {
                let id = id.to_string();
                self.approve(move |a| {
                    (a.choices.iter())
                        .find(|c| c.id == id)
                        .ok_or_else(|| "no such choice".to_string())
                })
                .await
            }
            [":answer", ..] => {
                let text = line.trim_start_matches(":answer").trim().to_string();
                self.answer(None, Some(text)).await
            }
            [":pick", id] => self.answer(Some(id.to_string()), None).await,
            [":stop"] => self.ask(None, Asked::Interrupt, "Stop").await,
            [":new"] => self.ask(None, Asked::NewSession, "New session").await,
            [":set", setting, value] => {
                let words = format!("{setting}: {value}");
                let asked = Asked::Set {
                    setting: setting.to_string(),
                    value: value.to_string(),
                };
                self.ask(None, asked, &words).await
            }
            [first, ..] if first.starts_with(':') => Err("not a command this client knows".into()),
            _ => {
                let (group, _) = self.chat.as_ref().ok_or("no chat yet: :invite the agent")?;
                self.runtime
                    .send_message(&self.me, group, line.as_bytes().to_vec())
                    .await
                    .map(drop)
                    .map_err(|_| "the message could not be sent".into())
            }
        }
    }

    /// Answers the latest approval waiting in this chat with the choice `wanted` picks from it.
    async fn approve(
        &mut self,
        wanted: impl Fn(&Approval) -> Result<&Choice, String>,
    ) -> Result<(), String> {
        let (target, choice) = {
            let (target, seen) = self
                .waiting(|b| matches!(b, Body::Approval(_)))
                .ok_or("no approval is waiting in this chat")?;
            let Body::Approval(approval) = seen.read.item.event.body() else {
                unreachable!()
            };
            let choice = wanted(approval)?;
            (target.clone(), choice.clone())
        };
        let asked = Asked::Choice { choice: choice.id };
        self.ask(Some(target), asked, &choice.label).await
    }

    /// Answers the first question of the question item waiting in this chat, with an option or
    /// text. A secret is sent only in a two-person chat, where nobody else reads it (D-041).
    async fn answer(&mut self, option: Option<String>, text: Option<String>) -> Result<(), String> {
        let (group, agent) = self.chat.clone().ok_or("no chat yet")?;
        let place = self.place(&group).await;
        let (target, question, secret) = {
            let (target, seen) = self
                .waiting(|b| matches!(b, Body::Question(_)))
                .ok_or("no question is waiting in this chat")?;
            let Body::Question(item) = seen.read.item.event.body() else {
                unreachable!()
            };
            let asked = first_question(item, place, agent.as_str())?;
            (target.clone(), asked.id.clone(), asked.secret)
        };
        if secret && !place.two() {
            return Err("a secret is answered only in a two-person chat; use the terminal".into());
        }
        let reply = AskedReply {
            question,
            options: option.into_iter().collect(),
            text,
        };
        let asked = Asked::Questions {
            questions: vec![reply],
        };
        self.ask(Some(target), asked, "Answered").await
    }

    /// The latest item waiting in this chat, from its agent, of the type `kind` picks.
    fn waiting(&self, kind: impl Fn(&Body) -> bool) -> Option<(&EventKey, &Seen)> {
        let (group, agent) = self.chat.as_ref()?;
        let group = hex::encode(group.as_slice());
        self.items.waiting(&group, agent.as_str(), kind)
    }

    async fn ask(
        &mut self,
        target: Option<EventKey>,
        asked: Asked,
        words: &str,
    ) -> Result<(), String> {
        let (group, agent) = self.chat.as_ref().ok_or("no chat yet")?;
        let out = carrier::request(agent, target.as_ref(), &asked, words);
        self.runtime
            .send_custom_event(&self.me, group, out.kind, out.tags, out.content)
            .await
            .map(drop)
            .map_err(|_| "it could not be sent".into())
    }

    /// Whether `group` is a direct chat, and how many are in it, as MDK has it now.
    async fn place(&self, group: &GroupId) -> Place {
        let hex = hex::encode(group.as_slice());
        let direct = (self.app.groups(&self.label).ok())
            .and_then(|groups| groups.into_iter().find(|g| g.group_id_hex == hex))
            .is_some_and(|g| g.direct_member_ids_hex.is_some());
        let members = (self.runtime.group_mls_state(&self.me, group).await)
            .ok()
            .map(|s| s.member_count);
        Place { direct, members }
    }

    async fn received(&mut self, message: marmot_app::ReceivedMessage) {
        if message.sender == self.me {
            return;
        }
        if let Some(line) = self.items.words_line(&message) {
            println!("{line}");
            return;
        }
        let group = hex::encode(message.group_id.as_slice());
        let row = |first: &str| {
            let row = self
                .runtime
                .timeline_message(&self.me, &group, first)
                .ok()??;
            (row.kind == ITEM_KIND && row.agent_item.is_some()).then_some(Row {
                sender: row.sender,
                plaintext: row.plaintext,
                tags: row.tags,
            })
        };
        let place = self.place(&message.group_id).await;
        let selected = (self.chat.as_ref()).map(|(g, a)| (hex::encode(g.as_slice()), a.as_str()));
        let selected = selected.as_ref().map(|(g, a)| (g.as_str(), *a));
        for line in self.items.arrived(&message, selected, place, row) {
            println!("{line}");
        }
    }
}

impl Items {
    /// The line for words sent to the chat, or `None` for anything else: a chat message, or a
    /// thread comment, which this client, having no threads, shows inline, marked as in a thread.
    /// A subagent's words are a thread comment by the agent whose root is the agent's `task` item
    /// of a subagent (D-122): they are marked as in the thread of the subagent whose `task` item
    /// they answer, by what that item says it is for, once it has shown. Any other comment, as a
    /// person's reply in a thread, is shown as a reply in a thread.
    fn words_line(&self, message: &marmot_app::ReceivedMessage) -> Option<String> {
        let name = clean(message.sender_display_name.as_deref().unwrap_or("agent"));
        let group = hex::encode(message.group_id.as_slice());
        let chat = chat_label(&group, &message.sender);
        let text = clean(&message.plaintext);
        match message.kind {
            CHAT => Some(format!("{chat} {name}: {text}")),
            COMMENT => {
                let named = |tag: &str| {
                    let id = (message.tags.iter())
                        .find(|t| t.first().map(String::as_str) == Some(tag))
                        .and_then(|t| t.get(1))?;
                    self.known.get(&EventKey(id.clone()))
                };
                // A task row of a subagent's that the comment's author sent.
                let subagents = |seen: &&Seen| {
                    seen.sender == message.sender
                        && matches!(seen.read.item.event.body(),
                            Body::Task(task) if task.kind == "subagent")
                };
                let place = match named("E").filter(subagents) {
                    Some(root) => {
                        let task = named("e").filter(subagents).unwrap_or(root);
                        format!("in the thread of {}", task_label(task))
                    }
                    None => "in a thread".to_string(),
                };
                Some(format!("{chat} {name}, {place}: {text}"))
            }
            _ => None,
        }
    }

    /// Takes in an item event that arrived, its first version or a later one, and gives the lines
    /// to show for it: none when it changes nothing the chat shows. `row` gives the item as MDK
    /// keeps it, by its first event.
    fn arrived(
        &mut self,
        message: &marmot_app::ReceivedMessage,
        selected: Option<(&str, &str)>,
        place: Place,
        row: impl FnOnce(&str) -> Option<Row>,
    ) -> Vec<String> {
        // Only to know which item changed: what it now is comes from MDK's row of it, which
        // takes a version only from the item's sender and holds an edit until its first event.
        let first = match message.kind {
            ITEM_KIND => message.message_id_hex.clone(),
            EDIT_KIND => match carrier::edit_target(&message.tags) {
                Some(first) => first.to_string(),
                None => return Vec::new(),
            },
            _ => return Vec::new(),
        };
        let Some(row) = row(&first) else {
            return Vec::new();
        };
        let Some(read) = carrier::read_item(ITEM_KIND, &row.plaintext, &row.tags) else {
            return Vec::new();
        };
        let first = EventKey(first);
        if (self.known.get(&first))
            .is_some_and(|known| known.read.item.version >= read.item.version)
        {
            return Vec::new();
        }
        let group = hex::encode(message.group_id.as_slice());
        let here = selected == Some((group.as_str(), row.sender.as_str()));
        let lines = lines_for(&read, &group, &row.sender, place, here);
        self.arrived += 1;
        let seen = Seen {
            read,
            group,
            sender: row.sender,
            order: self.arrived,
        };
        self.known.insert(first, seen);
        lines
    }

    /// The chat whose label is `label`: its group's hex id, and the agent in it that the label
    /// names.
    fn chat(&self, label: &str) -> Result<(String, String), String> {
        (self.known.values())
            .find(|s| chat_label(&s.group, &s.sender) == label)
            .map(|s| (s.group.clone(), s.sender.clone()))
            .ok_or_else(|| format!("no chat {label} has shown an item"))
    }

    /// The lines for every item waiting in `group` from `agent`, oldest first, with the shortcuts
    /// that answer them: for a chat just selected.
    fn waiting_lines(&self, group: &str, agent: &str, place: Place) -> Vec<String> {
        let mut waiting: Vec<&Seen> = (self.known.values())
            .filter(|s| s.group == group && s.sender == agent)
            .filter(|s| s.read.item.event.header().state == State::Waiting)
            .collect();
        waiting.sort_by_key(|s| s.order);
        (waiting.into_iter())
            .flat_map(|s| lines_for(&s.read, group, agent, place, true))
            .collect()
    }

    /// The latest item waiting in `group` from `agent`, of the type `kind` picks.
    fn waiting(
        &self,
        group: &str,
        agent: &str,
        kind: impl Fn(&Body) -> bool,
    ) -> Option<(&EventKey, &Seen)> {
        (self.known.iter())
            .filter(|(_, s)| s.group == group && s.sender == agent)
            .filter(|(_, s)| s.read.item.event.header().state == State::Waiting)
            .filter(|(_, s)| kind(s.read.item.event.body()))
            .max_by_key(|(_, s)| s.order)
    }
}

/// A short name for a chat with one agent in it, from its group's hex id and the agent's key: the
/// first eight digits of each. Two agents in one group are two chats.
/// What a subagent's `task` row says it is for: the first line of its text, its path or what it
/// was asked, or, with none, its title, which a harness may give every subagent alike.
fn task_label(seen: &Seen) -> String {
    let header = seen.read.item.event.header();
    let text = header.text.lines().find(|l| !l.trim().is_empty());
    clean(text.unwrap_or(&header.title))
}

fn chat_label(group: &str, agent: &str) -> String {
    let short = |hex: &str| hex.chars().take(8).collect::<String>();
    format!("{}/{}", short(group), short(agent))
}

/// The one choice of `approval` with `effect`. With more than one, such as Goose's "allow always"
/// listed before "allow once", none: which is meant is the person's to say with `:choose`.
fn only(approval: &Approval, effect: Effect) -> Result<&Choice, String> {
    let does = match effect {
        Effect::Allow => "allows",
        Effect::Deny => "denies",
    };
    let mut matching = approval.choices.iter().filter(|c| c.effect == effect);
    match (matching.next(), matching.next()) {
        (Some(choice), None) => Ok(choice),
        (None, _) => Err(format!("no choice {does}")),
        (Some(_), Some(_)) => {
            let ids: Vec<String> = (approval.choices.iter())
                .filter(|c| c.effect == effect)
                .map(|c| format!("{} ({})", clean(&c.id), clean(&c.label)))
                .collect();
            Err(format!(
                "more than one choice {does}: :choose one of {}",
                ids.join(", ")
            ))
        }
    }
}

/// The question an answer by request answers: the item's first. A question answered by a plain
/// message takes no request: the harness waits for the person's next message, which
/// [`by_message`] says how to send.
fn first_question<'a>(
    item: &'a Question,
    place: Place,
    agent: &str,
) -> Result<&'a AskedOne, String> {
    if item.answer_via() == AnswerVia::Message {
        return Err(format!(
            "this question is answered by a plain message: {}",
            by_message(item, place, agent)
        ));
    }
    item.questions()
        .first()
        .ok_or_else(|| "the question asks nothing".to_string())
}

/// How to answer a question answered by a plain message in `place`. A secret is typed only where
/// the person and the agent are alone (D-041); in a group, the core hears only a message that
/// mentions the agent or replies to it, a named group of two included.
fn by_message(item: &Question, place: Place, agent: &str) -> String {
    if secret_where_others_read(item, place) {
        return "it asks for a secret, and others read this chat: answer it in the harness's \
                terminal"
            .into();
    }
    let reply = "reply with a plain message: the agent takes your next one";
    if place.direct {
        return reply.into();
    }
    let npub = marmot_app::npub_for_account_id(agent).unwrap_or_default();
    format!("{reply}; in a group it hears only one that mentions it, so start with nostr:{npub}")
}

/// Whether `item` asks for a secret in a chat others read, so it is answered in the harness's
/// terminal instead.
fn secret_where_others_read(item: &Question, place: Place) -> bool {
    item.questions().iter().any(|q| q.secret) && !place.two()
}

/// Text from the chat as it may be printed: no control characters, so nothing received can move
/// the cursor or overwrite what the person reads before they answer.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect()
}

/// The lines for an item's latest version: its chat's label, its type and state, title and plain
/// rendering, and for one that waits, its choices or options with their ids and how it is
/// answered, in every chat. The shortcuts are offered only `here`, in the chat being talked in,
/// where they answer; elsewhere the lines say how to switch to its chat.
fn lines_for(read: &Read, group: &str, agent: &str, place: Place, here: bool) -> Vec<String> {
    let event = &read.item.event;
    let header = event.header();
    let kind = match event.body() {
        Body::Turn(_) => "turn",
        Body::Message(_) => "message",
        Body::Reasoning(_) => "reasoning",
        Body::Tool(_) => "tool",
        Body::Approval(_) => "approval",
        Body::Question(_) => "question",
        Body::Plan(_) => "plan",
        Body::Task(_) => "task",
        Body::Notice(_) => "notice",
        Body::Error(_) => "error",
        Body::Session(_) => "session",
    };
    let state = format!("{:?}", header.state).to_lowercase();
    let mut text = clean(&header.text);
    if text.len() > 160 {
        let mut end = 160;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    let title = clean(&header.title);
    let chat = chat_label(group, agent);
    let mut lines = vec![format!("{chat} [{kind} · {state}] {title}: {text}")];
    if header.state != State::Waiting {
        return lines;
    }
    // How it is answered, said the same in every chat: here as it is done, elsewhere after
    // switching to its chat, as the shortcuts answer in the chat being talked in.
    let how = |how: String| {
        if here {
            format!("    {how}")
        } else {
            format!("    in another chat: :chat {chat}, then {how}")
        }
    };
    match event.body() {
        Body::Approval(approval) => {
            let choices: Vec<String> = approval
                .choices
                .iter()
                .map(|c| format!("{} ({})", clean(&c.label), clean(&c.id)))
                .collect();
            let shortcuts: Vec<&str> = [
                (only(approval, Effect::Allow).is_ok()).then_some(":allow"),
                (only(approval, Effect::Deny).is_ok()).then_some(":deny"),
                Some(":choose <id>"),
            ]
            .into_iter()
            .flatten()
            .collect();
            if here {
                lines.push(format!(
                    "    {}: {}",
                    shortcuts.join(", "),
                    choices.join(", ")
                ));
            } else {
                lines.push(format!("    choices: {}", choices.join(", ")));
                lines.push(how(format!("answer with {}", shortcuts.join(", "))));
            }
        }
        Body::Question(question) => {
            let by_request = question.answer_via() != AnswerVia::Message;
            for asked in question.questions() {
                let secret = if asked.secret { " (secret)" } else { "" };
                lines.push(format!("    ? {}{secret}", clean(&asked.text)));
                for option in &asked.options {
                    let (id, label) = (clean(&option.id), clean(&option.label));
                    lines.push(match (by_request, here) {
                        (true, true) => format!("      :pick {id}  {label}"),
                        (true, false) => format!("      {id}  {label}"),
                        (false, _) => format!("      - {label}"),
                    });
                }
            }
            if by_request && here {
                lines.push("    :answer <text> answers the first question".into());
            } else if by_request {
                lines.push(how(
                    "answer with :answer <text> or :pick <id>, for the first question".into(),
                ));
            } else {
                let by_message = by_message(question, place, agent);
                // A secret goes to the terminal, from whichever chat.
                if secret_where_others_read(question, place) {
                    lines.push(format!("    {by_message}"));
                } else {
                    lines.push(how(by_message));
                }
            }
        }
        _ => {}
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use marmot_agent::carrier::Item;
    use marmot_agent_events::body::{StartedBy, Turn};
    use marmot_agent_events::{AgentEvent, Header};

    const AGENT: &str = "aa";
    const STRANGER: &str = "bb";

    const CHAT_A: u8 = 0xa1;
    const CHAT_B: u8 = 0xb2;

    /// An item event as MDK hands it over: `out`, as the carrier serialized it, sent by `sender`
    /// as event `id` to the chat whose group id is all `chat`.
    fn received(id: &str, sender: &str, chat: u8, out: carrier::Outgoing) -> ReceivedVersion {
        ReceivedVersion(marmot_app::ReceivedMessage {
            authority: None,
            message_id_hex: id.into(),
            source_message_id_hex: id.into(),
            sender: sender.into(),
            sender_display_name: None,
            group_id: GroupId::new(vec![chat; 16]),
            source_epoch: 1,
            retention: None,
            plaintext: out.content,
            kind: out.kind,
            tags: out.tags,
            recorded_at: 1,
            received_at: 1,
        })
    }

    struct ReceivedVersion(marmot_app::ReceivedMessage);

    /// Version `version` of one item, as an event of its sender: its first, or an edit of
    /// `first`.
    fn version(id: &str, sender: &str, version: u64, first: Option<&str>) -> ReceivedVersion {
        let header = Header {
            harness: "stub".into(),
            session: None,
            turn: None,
            subagent: None,
            item: "t".into(),
            seq: 1,
            state: State::Running,
            title: format!("v{version}"),
            text: format!("version {version}"),
        };
        let body = Body::Turn(Turn {
            started_by: StartedBy::User,
            duration_ms: None,
            usage: None,
        });
        let item = Item {
            event: AgentEvent::new(header, body, None).unwrap(),
            version,
            first: first.map(EventKey::from),
            started_by: None,
            settles: Vec::new(),
            thread: None,
        };
        received(id, sender, CHAT_A, carrier::item(&item).unwrap())
    }

    /// The first version of `event`, sent by `sender` to the chat whose group id is all `chat`.
    fn first_of(id: &str, sender: &str, chat: u8, event: AgentEvent) -> ReceivedVersion {
        let item = Item {
            event,
            version: 1,
            first: None,
            started_by: None,
            settles: Vec::new(),
            thread: None,
        };
        received(id, sender, chat, carrier::item(&item).unwrap())
    }

    /// The waiting event of `kind` in a recording's expected events, as the driver makes it.
    fn recorded(path: &str, kind: &str) -> AgentEvent {
        let path = format!(
            "{}/../../fixtures/recordings/{path}",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(&path).unwrap();
        (text.lines())
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .find(|e| e["type"] == kind && e["state"] == "waiting")
            .map(|e| serde_json::from_value(e).unwrap())
            .unwrap_or_else(|| panic!("no waiting {kind} in {path}"))
    }

    fn group_hex(chat: u8) -> String {
        hex::encode([chat; 16])
    }

    /// A direct chat, of the person and the agent.
    const DIRECT: Place = Place {
        direct: true,
        members: Some(2),
    };

    /// Takes in `received` as the chat shows it, with chat A and its agent selected, a direct
    /// chat.
    fn shown_in_a(items: &mut Items, received: &ReceivedVersion) -> Vec<String> {
        shown_with(items, received, AGENT, DIRECT)
    }

    /// Takes in `received` as the chat shows it, with chat A and `agent` selected, in `place`.
    fn shown_with(
        items: &mut Items,
        received: &ReceivedVersion,
        agent: &str,
        place: Place,
    ) -> Vec<String> {
        let selected = group_hex(CHAT_A);
        items.arrived(&received.0, Some((&selected, agent)), place, |_| {
            Some(received.row())
        })
    }

    /// The label the lines of an item start with, which `:chat` takes.
    fn label_of(lines: &[String]) -> &str {
        lines[0].split(' ').next().unwrap()
    }

    /// The waiting event of `kind` in a recording's expected events, changed by `change`.
    fn recorded_with(
        path: &str,
        kind: &str,
        change: impl FnOnce(&mut serde_json::Value),
    ) -> AgentEvent {
        let mut event = serde_json::to_value(recorded(path, kind)).unwrap();
        change(&mut event);
        serde_json::from_value(event).unwrap()
    }

    /// Whether any line, its indent left out, starts with `start`.
    fn any_starts(lines: &[String], start: &str) -> bool {
        lines.iter().any(|l| l.trim_start().starts_with(start))
    }

    /// Whether any line holds `part`.
    fn any_holds(lines: &[String], part: &str) -> bool {
        lines.iter().any(|l| l.contains(part))
    }

    // #140. Every line names the chat it is from, and only an item in the chat being talked in
    // offers the shortcuts, which answer that chat's item: with two chats, the approval the
    // shortcuts answer is the one shown with them, even when another chat's came after it.
    #[test]
    fn the_shortcuts_are_offered_only_for_the_chat_they_answer_in() {
        let mut items = Items::default();
        let approval = recorded("acp/goose/expected/approval_allowed.jsonl", "approval");
        let in_a = first_of("ea", AGENT, CHAT_A, approval.clone());
        let in_b = first_of("eb", AGENT, CHAT_B, approval);
        let lines_a = shown_in_a(&mut items, &in_a);
        let lines_b = shown_in_a(&mut items, &in_b);
        let (label_a, label_b) = (label_of(&lines_a), label_of(&lines_b));
        assert_ne!(label_a, label_b);
        assert!(label_a.starts_with(&group_hex(CHAT_A)[..8]), "{lines_a:?}");
        assert!(label_b.starts_with(&group_hex(CHAT_B)[..8]), "{lines_b:?}");
        assert!(lines_a.iter().any(|l| l.contains(":choose")), "{lines_a:?}");
        // Chat B's says how it is answered only after how to switch to chat B.
        let switch = format!(":chat {label_b}");
        assert!(any_holds(&lines_b, ":choose"), "{lines_b:?}");
        assert!(
            (lines_b.iter())
                .filter(|l| l.contains(":choose"))
                .all(|l| l.find(&switch) < l.find(":choose")),
            "{lines_b:?}"
        );
        let (answered, _) = (items.waiting(&group_hex(CHAT_A), AGENT, |b| {
            matches!(b, Body::Approval(_))
        }))
        .expect("chat A's approval waits");
        assert_eq!(answered, &EventKey::from("ea"));
        // Review of #140. A question of either kind in chat B says from its first lines how it is
        // answered, with its options' ids, and how to switch to its chat; no shortcut is offered,
        // since one would answer in chat A.
        for (id, path, how) in [
            (
                "eq1",
                "claude/expected/question.jsonl",
                ":answer <text> or :pick <id>",
            ),
            ("eq2", "codex/expected/question.jsonl", "plain message"),
        ] {
            let event = recorded(path, "question");
            let Body::Question(question) = event.body() else {
                panic!("a question")
            };
            let lines = shown_in_a(&mut items, &first_of(id, AGENT, CHAT_B, event.clone()));
            assert_eq!(label_of(&lines), label_b, "{lines:?}");
            assert!(any_holds(&lines, how), "{lines:?}");
            assert!(any_holds(&lines, &switch), "{lines:?}");
            assert!(!any_starts(&lines, ":pick"), "{lines:?}");
            if question.answer_via() == AnswerVia::Request {
                for option in &question.questions()[0].options {
                    let shown = format!("{}  {}", option.id, option.label);
                    assert!(any_holds(&lines, &shown), "{lines:?}");
                }
            }
        }
    }

    // Review of #140. Selecting a chat shows again what waits in it, with the shortcuts that now
    // answer it.
    #[test]
    fn a_chat_selected_shows_what_waits_in_it_with_its_shortcuts() {
        let mut items = Items::default();
        let event = recorded("claude/expected/question.jsonl", "question");
        let lines = shown_in_a(&mut items, &first_of("eq", AGENT, CHAT_B, event));
        assert!(!any_starts(&lines, ":pick"), "{lines:?}");
        let (group, agent) = items.chat(label_of(&lines)).unwrap();
        let again = items.waiting_lines(&group, &agent, DIRECT);
        assert!(any_starts(&again, ":pick o0"), "{again:?}");
        assert!(any_starts(&again, ":answer"), "{again:?}");
        assert!(!any_holds(&again, ":chat"), "{again:?}");
    }

    // Review of #140. Two agents in one group are two chats, each with its own label: `:chat`
    // selects the agent whose label is typed, not the one that sent the latest item.
    #[test]
    fn two_agents_in_one_group_are_selected_apart() {
        const OTHER: &str = "cc";
        let mut items = Items::default();
        let approval = recorded("acp/goose/expected/approval_allowed.jsonl", "approval");
        let first = shown_in_a(&mut items, &first_of("e1", AGENT, CHAT_B, approval.clone()));
        let later = shown_in_a(&mut items, &first_of("e2", OTHER, CHAT_B, approval));
        assert_ne!(label_of(&first), label_of(&later));
        let chat = |lines: &[String]| items.chat(label_of(lines));
        assert_eq!(chat(&first), Ok((group_hex(CHAT_B), AGENT.to_string())));
        assert_eq!(chat(&later), Ok((group_hex(CHAT_B), OTHER.to_string())));
    }

    // Review of #141. In a group the core hears only a message that mentions the agent or
    // replies to it, a named group of two included: a question answered by a plain message says
    // to mention the agent there, and so does the refusal of `:answer` and `:pick`. In a direct
    // chat every message reaches it.
    #[test]
    fn a_plain_answer_in_a_group_says_to_mention_the_agent() {
        let agent = format!("a9{:062x}", 1);
        let npub = marmot_app::npub_for_account_id(&agent).unwrap();
        let mention = format!("nostr:{npub}");
        let event = recorded("codex/expected/question.jsonl", "question");
        let Body::Question(question) = event.body() else {
            panic!("a question")
        };
        let named = Place {
            direct: false,
            members: Some(2),
        };
        let item = first_of("eq", &agent, CHAT_A, event.clone());
        let lines = shown_with(&mut Items::default(), &item, &agent, named);
        assert!(any_holds(&lines, &mention), "{lines:?}");
        let refused = first_question(question, named, &agent).expect_err("by a message");
        assert!(refused.contains(&mention), "{refused}");
        let lines = shown_with(&mut Items::default(), &item, &agent, DIRECT);
        assert!(!any_holds(&lines, "nostr:"), "{lines:?}");
        assert!(any_holds(&lines, "plain message"), "{lines:?}");
    }

    // Review of #141. A secret answered by a plain message is typed only where the person and
    // the agent are alone (D-041): anywhere else the chat points to the harness's terminal.
    #[test]
    fn a_secret_answered_by_a_message_is_not_typed_where_others_read_it() {
        let event = recorded_with("codex/expected/question.jsonl", "question", |e| {
            e["body"]["questions"][0]["secret"] = true.into();
        });
        let Body::Question(question) = event.body() else {
            panic!("a question")
        };
        let three = Place {
            direct: false,
            members: Some(3),
        };
        let item = first_of("eq", AGENT, CHAT_A, event.clone());
        let lines = shown_with(&mut Items::default(), &item, AGENT, three);
        assert!(!any_holds(&lines, "plain message"), "{lines:?}");
        assert!(any_holds(&lines, "terminal"), "{lines:?}");
        let refused = first_question(question, three, AGENT).expect_err("by a message");
        assert!(refused.contains("terminal"), "{refused}");
        let lines = shown_with(&mut Items::default(), &item, AGENT, DIRECT);
        assert!(any_holds(&lines, "plain message"), "{lines:?}");
    }

    // #140. `:chat` selects a chat by the label its lines show, with the agent that sent its
    // items.
    #[test]
    fn a_chat_is_selected_by_its_label() {
        let mut items = Items::default();
        let approval = recorded("acp/goose/expected/approval_allowed.jsonl", "approval");
        let lines = shown_in_a(&mut items, &first_of("eb", AGENT, CHAT_B, approval));
        assert_eq!(
            items.chat(label_of(&lines)),
            Ok((group_hex(CHAT_B), AGENT.to_string()))
        );
        assert!(items.chat("ffff").is_err());
    }

    // #137. Goose lists a standing approval before the one-time one, and both allow: `:allow`
    // picks neither, and asks for `:choose`. With one choice that allows, it takes that one.
    #[test]
    fn allow_refuses_when_more_than_one_choice_allows() {
        let event = recorded("acp/goose/expected/approval_allowed.jsonl", "approval");
        let Body::Approval(approval) = event.body() else {
            panic!("an approval")
        };
        assert!(
            approval
                .choices
                .iter()
                .filter(|c| c.effect == Effect::Allow)
                .count()
                > 1
        );
        let refused = only(approval, Effect::Allow).expect_err("two choices allow");
        assert!(refused.contains(":choose"), "{refused}");
        let mut one = approval.clone();
        one.choices.retain(|c| c.id != "c0" && c.id != "c3");
        assert_eq!(only(&one, Effect::Allow).map(|c| c.id.as_str()), Ok("c1"));
        assert_eq!(only(&one, Effect::Deny).map(|c| c.id.as_str()), Ok("c2"));
        let item = first_of("ea", AGENT, CHAT_A, event.clone());
        let lines = shown_in_a(&mut Items::default(), &item);
        assert!(!lines.iter().any(|l| l.contains(":allow")), "{lines:?}");
    }

    // #141. The recorded Codex question is answered by a plain message: the chat says so, offers
    // neither `:answer` nor `:pick`, and both refuse it.
    #[test]
    fn a_question_answered_by_a_message_asks_for_a_message() {
        let event = recorded("codex/expected/question.jsonl", "question");
        let Body::Question(question) = event.body() else {
            panic!("a question")
        };
        assert_eq!(question.answer_via(), AnswerVia::Message);
        let item = first_of("eq", AGENT, CHAT_A, event.clone());
        let lines = shown_in_a(&mut Items::default(), &item);
        assert!(
            lines.iter().any(|l| l.contains("plain message")),
            "{lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|l| l.contains(":pick") || l.contains(":answer")),
            "{lines:?}"
        );
        let refused =
            first_question(question, Place::default(), AGENT).expect_err("answered by a message");
        assert!(refused.contains("plain message"), "{refused}");
    }

    // A question answered by request still offers both.
    #[test]
    fn a_question_answered_by_request_offers_answer_and_pick() {
        let event = recorded("claude/expected/question.jsonl", "question");
        let Body::Question(question) = event.body() else {
            panic!("a question")
        };
        assert_eq!(question.answer_via(), AnswerVia::Request);
        let item = first_of("eq", AGENT, CHAT_A, event.clone());
        let lines = shown_in_a(&mut Items::default(), &item);
        assert!(lines.iter().any(|l| l.contains(":answer")), "{lines:?}");
        assert!(first_question(question, Place::default(), AGENT).is_ok());
    }

    impl ReceivedVersion {
        /// The row MDK would keep for the item, if this were its latest version from its
        /// sender.
        fn row(&self) -> Row {
            Row {
                sender: self.0.sender.clone(),
                plaintext: self.0.plaintext.clone(),
                tags: (self.0.tags.iter())
                    .filter(|t| t[0] != "e")
                    .cloned()
                    .collect(),
            }
        }
    }

    fn shown(items: &Items) -> Vec<(String, u64)> {
        (items.known.values())
            .map(|s| (s.sender.clone(), s.read.item.version))
            .collect()
    }

    /// A subagent's `task` item, as event `id`, titled as Codex titles every one, and saying what
    /// it is for, "look around".
    fn task_row(id: &str) -> ReceivedVersion {
        let header = Header {
            harness: "stub".into(),
            session: None,
            turn: None,
            subagent: None,
            item: "task:a".into(),
            seq: 1,
            state: State::Running,
            title: "Subagent working".into(),
            text: "look around".into(),
        };
        let body = Body::Task(marmot_agent_events::body::Task {
            kind: "subagent".into(),
            description: None,
            subject: None,
        });
        first_of(
            id,
            AGENT,
            CHAT_A,
            AgentEvent::new(header, body, None).unwrap(),
        )
    }

    /// A message of the agent's, of `kind`, with `tags`, as MDK hands it over.
    fn words(
        id: &str,
        kind: u64,
        text: &str,
        tags: Vec<Vec<String>>,
    ) -> marmot_app::ReceivedMessage {
        let mut message = received(
            id,
            AGENT,
            CHAT_A,
            carrier::Outgoing {
                kind,
                content: text.into(),
                tags,
            },
        )
        .0;
        message.sender_display_name = Some("Codex".into());
        message
    }

    // A subagent's words are a thread comment, kind 1111 (D-122): the terminal client, which has no
    // threads, shows them inline as the agent's words, marked as in the thread of the subagent
    // whose `task` item they answer; the agent's own words are shown as before.
    #[test]
    fn a_subagents_words_are_shown_inline_marked_as_in_its_thread() {
        let mut items = Items::default();
        let said = words("c1", 9, "all done", Vec::new());
        assert_eq!(
            items.words_line(&said),
            Some(format!(
                "{} Codex: all done",
                chat_label(&group_hex(CHAT_A), AGENT)
            ))
        );
        let comment = |parent: &str| {
            let tags = vec![
                vec!["E".into(), parent.into(), String::new(), AGENT.into()],
                vec!["K".into(), "1201".into()],
                vec!["P".into(), AGENT.into()],
                vec!["e".into(), parent.into(), String::new(), AGENT.into()],
                vec!["k".into(), "1201".into()],
            ];
            words("c2", 1111, "found two\nfiles", tags)
        };
        // Its subagent's row not seen yet: shown as a reply in a thread, nothing more.
        let line = items.words_line(&comment("t1")).unwrap();
        assert_eq!(
            line,
            format!(
                "{} Codex, in a thread: found two files",
                chat_label(&group_hex(CHAT_A), AGENT)
            )
        );
        items.arrived(&task_row("t1").0, None, DIRECT, |_| {
            Some(task_row("t1").row())
        });
        // Marked by what the row says the subagent is for, not by its title, which is every
        // subagent's.
        let line = items.words_line(&comment("t1")).unwrap();
        assert_eq!(
            line,
            format!(
                "{} Codex, in the thread of look around: found two files",
                chat_label(&group_hex(CHAT_A), AGENT)
            )
        );
        // A person's reply in that thread is not the subagent's.
        let mut reply = comment("t1");
        reply.sender = STRANGER.into();
        reply.sender_display_name = Some("Bob".into());
        let line = items.words_line(&reply).unwrap();
        assert_eq!(
            line,
            format!(
                "{} Bob, in a thread: found two files",
                chat_label(&group_hex(CHAT_A), STRANGER)
            )
        );
        // Anything else is not words.
        assert_eq!(items.words_line(&words("c3", 7, "+", Vec::new())), None);
    }

    // An edit that comes before its first event shows nothing until the first event comes, and
    // then the item as MDK has it, with the edit's version.
    #[test]
    fn an_edit_before_its_first_event_is_held() {
        let mut items = Items::default();
        let edit = version("e2", AGENT, 2, Some("e1"));
        assert!(
            items
                .arrived(&edit.0, None, Place::default(), |_| None)
                .is_empty(),
            "nothing yet"
        );
        assert!(shown(&items).is_empty());
        let first = version("e1", AGENT, 1, None);
        let lines = items.arrived(&first.0, None, Place::default(), |id| {
            (id == "e1").then(|| edit.row())
        });
        assert_eq!(lines.len(), 1);
        assert_eq!(shown(&items), [(AGENT.to_string(), 2)]);
    }

    // An edit from anyone but the item's sender changes nothing, even when it comes first.
    #[test]
    fn an_edit_from_another_sender_changes_nothing() {
        let mut items = Items::default();
        let first = version("e1", AGENT, 1, None);
        let forged = version("e9", STRANGER, 5, Some("e1"));
        assert!(
            items
                .arrived(&forged.0, None, Place::default(), |_| None)
                .is_empty(),
            "nothing yet"
        );
        items.arrived(&first.0, None, Place::default(), |_| Some(first.row()));
        assert_eq!(shown(&items), [(AGENT.to_string(), 1)]);
        assert!(
            items
                .arrived(&forged.0, None, Place::default(), |_| Some(first.row()))
                .is_empty()
        );
        assert_eq!(shown(&items), [(AGENT.to_string(), 1)]);
    }
}
