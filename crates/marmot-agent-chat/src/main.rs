//! `marmot-agent-chat`: a terminal client that plays the person, for trying the core by hand.
//!
//! It holds one person's account, talks to the agent in a Marmot group, shows each of the agent's
//! items as it changes, and sends answers and controls. How it shows an item is a plain reference
//! for the apps. When an item's first version, kind 1201, or a later one, MDK's edit of it, arrives,
//! it shows the item as MDK keeps it, one row at its first event with its latest version; and it
//! sends answers and controls as kind 1202 (D-076). How to try the core with it is in
//! `docs/try.md`.
//!
//! Each line from a chat starts with the chat's label, and the shortcuts answer only in the chat
//! being talked in, so a waiting item from another chat shows how to switch to it instead.
//!
//! Typed lines are messages, sent exactly as typed, slash commands included (D-031). Lines that
//! start with `:` are for this client:
//!
//! ```text
//! :invite <key>          start a direct chat with the agent
//! :group <name> <key>    make a named group with the agent in it
//! :chat <label>          talk in the chat whose lines start with that label
//! :allow   :deny         answer the latest approval waiting in this chat with its one choice
//!                        that allows, or denies; with more than one, :choose
//! :choose <id>           answer it with the choice of that id
//! :answer <text>         answer the question waiting in this chat with text
//! :pick <id>             answer it with the option of that id
//!                        (a question answered by a plain message takes neither: type it)
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

struct Client {
    runtime: MarmotAppRuntime,
    me: String,
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
    let runtime = MarmotAppRuntime::new(app);
    let existing = account_home.accounts().unwrap_or_default();
    let me = match existing.into_iter().find(|a| a.local_signing) {
        Some(account) => account.account_id_hex,
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
            made.account.account_id_hex
        }
    };
    println!("you: {me}");
    let mut events = runtime.subscribe();
    runtime.start().await.map_err(|_| "MDK could not start")?;
    let mut client = Client {
        runtime: runtime.clone(),
        me,
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
                    client.received(received.message);
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
                let (group, agent) = self.items.chat(label)?;
                let group = hex::decode(&group).map_err(|_| "not a chat")?;
                self.chat = Some((GroupId::new(group), PubKey(agent)));
                println!("· talking in {label}");
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
        let (target, question, secret) = {
            let (target, seen) = self
                .waiting(|b| matches!(b, Body::Question(_)))
                .ok_or("no question is waiting in this chat")?;
            let Body::Question(item) = seen.read.item.event.body() else {
                unreachable!()
            };
            let asked = first_question(item)?;
            (target.clone(), asked.id.clone(), asked.secret)
        };
        if secret {
            let (group, _) = self.chat.as_ref().ok_or("no chat yet")?;
            let state = self.runtime.group_mls_state(&self.me, group).await;
            if state.map_or(true, |s| s.member_count != 2) {
                return Err(
                    "a secret is answered only in a two-person chat; use the terminal".into(),
                );
            }
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

    fn received(&mut self, message: marmot_app::ReceivedMessage) {
        if message.sender == self.me {
            return;
        }
        let name = clean(message.sender_display_name.as_deref().unwrap_or("agent"));
        let group = hex::encode(message.group_id.as_slice());
        if message.kind == CHAT {
            println!(
                "{} {name}: {}",
                chat_label(&group),
                clean(&message.plaintext)
            );
            return;
        }
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
        let selected = (self.chat.as_ref()).map(|(g, a)| (hex::encode(g.as_slice()), a.as_str()));
        let selected = selected.as_ref().map(|(g, a)| (g.as_str(), *a));
        for line in self.items.arrived(&message, selected, row) {
            println!("{line}");
        }
    }
}

impl Items {
    /// Takes in an item event that arrived, its first version or a later one, and gives the lines
    /// to show for it: none when it changes nothing the chat shows. `row` gives the item as MDK
    /// keeps it, by its first event.
    fn arrived(
        &mut self,
        message: &marmot_app::ReceivedMessage,
        selected: Option<(&str, &str)>,
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
        let lines = lines_for(&read, &group, here);
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

    /// The chat whose label is `label`, as its group's hex id, and the agent that sent its
    /// latest item.
    fn chat(&self, label: &str) -> Result<(String, String), String> {
        (self.known.values())
            .filter(|s| chat_label(&s.group) == label)
            .max_by_key(|s| s.order)
            .map(|s| (s.group.clone(), s.sender.clone()))
            .ok_or_else(|| format!("no chat {label} has shown an item"))
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

/// A short name for a chat, from its group's hex id: its first eight digits.
fn chat_label(group: &str) -> String {
    group.chars().take(8).collect()
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
/// message takes no request: the harness waits for the person's next message.
fn first_question(item: &Question) -> Result<&AskedOne, String> {
    if item.answer_via() == AnswerVia::Message {
        return Err("this question is answered by a plain message: type your answer".into());
    }
    item.questions()
        .first()
        .ok_or_else(|| "the question asks nothing".to_string())
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
/// rendering, and for one that waits, what can be answered. The shortcuts are offered only `here`,
/// in the chat being talked in, where they answer; elsewhere, how to switch to its chat.
fn lines_for(read: &Read, group: &str, here: bool) -> Vec<String> {
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
    let chat = chat_label(group);
    let mut lines = vec![format!("{chat} [{kind} · {state}] {title}: {text}")];
    if header.state != State::Waiting {
        return lines;
    }
    match event.body() {
        Body::Approval(approval) => {
            let choices: Vec<String> = approval
                .choices
                .iter()
                .map(|c| format!("{} ({})", clean(&c.label), clean(&c.id)))
                .collect();
            if here {
                let shortcuts: Vec<&str> = [
                    (only(approval, Effect::Allow).is_ok()).then_some(":allow"),
                    (only(approval, Effect::Deny).is_ok()).then_some(":deny"),
                    Some(":choose <id>"),
                ]
                .into_iter()
                .flatten()
                .collect();
                lines.push(format!(
                    "    {}: {}",
                    shortcuts.join(", "),
                    choices.join(", ")
                ));
            } else {
                lines.push(format!("    choices: {}", choices.join(", ")));
            }
        }
        Body::Question(question) => {
            let by_message = question.answer_via() == AnswerVia::Message;
            for asked in question.questions() {
                let secret = if asked.secret { " (secret)" } else { "" };
                lines.push(format!("    ? {}{secret}", clean(&asked.text)));
                for option in &asked.options {
                    let (id, label) = (clean(&option.id), clean(&option.label));
                    if here && !by_message {
                        lines.push(format!("      :pick {id}  {label}"));
                    } else {
                        lines.push(format!("      - {label}"));
                    }
                }
            }
            if here && by_message {
                lines.push("    reply with a plain message: the agent takes your next one".into());
            } else if here {
                lines.push("    :answer <text> answers the first question".into());
            }
        }
        _ => {}
    }
    if !here && matches!(event.body(), Body::Approval(_) | Body::Question(_)) {
        lines.push(format!(
            "    in another chat: :chat {chat} to answer it there"
        ));
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

    /// Version `version` of one item, as an event of its sender: its first, or an edit of
    /// `first`.
    fn version(id: &str, sender: &str, version: u64, first: Option<&str>) -> ReceivedVersion {
        let header = Header {
            harness: "stub".into(),
            session: None,
            turn: None,
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
        };
        let out = carrier::item(&item).unwrap();
        ReceivedVersion(marmot_app::ReceivedMessage {
            authority: None,
            message_id_hex: id.into(),
            source_message_id_hex: id.into(),
            sender: sender.into(),
            sender_display_name: None,
            group_id: GroupId::new(vec![1; 16]),
            source_epoch: 1,
            retention: None,
            plaintext: out.content.clone(),
            kind: out.kind,
            tags: out.tags.clone(),
            recorded_at: 1,
            received_at: 1,
        })
    }

    struct ReceivedVersion(marmot_app::ReceivedMessage);

    const CHAT_A: u8 = 0xa1;
    const CHAT_B: u8 = 0xb2;

    /// The first version of `event`, sent by `sender` to the chat whose group id is all `chat`.
    fn first_of(id: &str, sender: &str, chat: u8, event: AgentEvent) -> ReceivedVersion {
        let item = Item {
            event,
            version: 1,
            first: None,
            started_by: None,
            settles: Vec::new(),
        };
        let out = carrier::item(&item).unwrap();
        ReceivedVersion(marmot_app::ReceivedMessage {
            group_id: GroupId::new(vec![chat; 16]),
            ..version(id, sender, 1, None).0
        })
        .with(out)
    }

    impl ReceivedVersion {
        fn with(mut self, out: carrier::Outgoing) -> Self {
            self.0.plaintext = out.content;
            self.0.kind = out.kind;
            self.0.tags = out.tags;
            self
        }
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

    /// Takes in `received` as the chat shows it, with chat A and its agent selected.
    fn shown_in_a(items: &mut Items, received: &ReceivedVersion) -> Vec<String> {
        let selected = group_hex(CHAT_A);
        items.arrived(&received.0, Some((&selected, AGENT)), |_| {
            Some(received.row())
        })
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
        let (label_a, label_b) = (
            chat_label(&group_hex(CHAT_A)),
            chat_label(&group_hex(CHAT_B)),
        );
        assert_ne!(label_a, label_b);
        assert!(lines_a[0].starts_with(&label_a), "{lines_a:?}");
        assert!(lines_b[0].starts_with(&label_b), "{lines_b:?}");
        assert!(lines_a.iter().any(|l| l.contains(":choose")), "{lines_a:?}");
        assert!(
            !lines_b.iter().any(|l| l.contains(":choose")),
            "{lines_b:?}"
        );
        assert!(lines_b.iter().any(|l| l.contains(":chat")), "{lines_b:?}");
        let (answered, _) = (items.waiting(&group_hex(CHAT_A), AGENT, |b| {
            matches!(b, Body::Approval(_))
        }))
        .expect("chat A's approval waits");
        assert_eq!(answered, &EventKey::from("ea"));
    }

    // #140. `:chat` selects a chat by the label its lines show, with the agent that sent its
    // items.
    #[test]
    fn a_chat_is_selected_by_its_label() {
        let mut items = Items::default();
        let approval = recorded("acp/goose/expected/approval_allowed.jsonl", "approval");
        shown_in_a(&mut items, &first_of("eb", AGENT, CHAT_B, approval));
        let label = chat_label(&group_hex(CHAT_B));
        assert_eq!(
            items.chat(&label),
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
        let refused = first_question(question).expect_err("answered by a message");
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
        assert!(first_question(question).is_ok());
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

    // An edit that comes before its first event shows nothing until the first event comes, and
    // then the item as MDK has it, with the edit's version.
    #[test]
    fn an_edit_before_its_first_event_is_held() {
        let mut items = Items::default();
        let edit = version("e2", AGENT, 2, Some("e1"));
        assert!(
            items.arrived(&edit.0, None, |_| None).is_empty(),
            "nothing yet"
        );
        assert!(shown(&items).is_empty());
        let first = version("e1", AGENT, 1, None);
        let lines = items.arrived(&first.0, None, |id| (id == "e1").then(|| edit.row()));
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
            items.arrived(&forged.0, None, |_| None).is_empty(),
            "nothing yet"
        );
        items.arrived(&first.0, None, |_| Some(first.row()));
        assert_eq!(shown(&items), [(AGENT.to_string(), 1)]);
        assert!(
            items
                .arrived(&forged.0, None, |_| Some(first.row()))
                .is_empty()
        );
        assert_eq!(shown(&items), [(AGENT.to_string(), 1)]);
    }
}
