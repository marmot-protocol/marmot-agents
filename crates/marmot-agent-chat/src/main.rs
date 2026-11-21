//! `marmot-agent-chat`: a terminal client that plays the person, for trying the core by hand.
//!
//! It holds one person's account, talks to the agent in a Marmot group, shows each of the agent's
//! items as it changes, and sends answers and controls. How it shows an item is a plain reference
//! for the apps. It knows the temporary carrier of D-069. How to try the core with it is in
//! `docs/try.md`.
//!
//! Typed lines are messages, sent exactly as typed, slash commands included (D-031). Lines that
//! start with `:` are for this client:
//!
//! ```text
//! :invite <key>          start a direct chat with the agent
//! :group <name> <key>    make a named group with the agent in it
//! :allow   :deny         answer the latest approval waiting in this chat, allowing once
//! :choose <id>           answer it with the choice of that id
//! :answer <text>         answer the question waiting in this chat with text
//! :pick <id>             answer it with the option of that id
//! :stop                  interrupt the running turn
//! :new                   start a new session
//! :set <id> <value>      change one of the session's settings
//! :quit
//! ```

use cgka_traits::{GroupId, TransportEndpoint};
use marmot_account::AccountHome;
use marmot_agent::carrier::{self, Asked, AskedReply, ITEM_KIND, Read};
use marmot_agent::{EventKey, PubKey};
use marmot_agent_events::State;
use marmot_agent_events::body::{Body, Choice, Effect};
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
    /// Each item's latest version, by its first event, with where it came from and when.
    items: HashMap<EventKey, Seen>,
    /// How many item versions have arrived, to tell the latest.
    arrived: u64,
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
        items: HashMap::new(),
        arrived: 0,
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
            [":allow"] => self.approve(|c| c.effect == Effect::Allow).await,
            [":deny"] => self.approve(|c| c.effect == Effect::Deny).await,
            [":choose", id] => self.approve(|c| c.id == *id).await,
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

    /// Answers the latest approval waiting in this chat with the first choice `wanted` picks. The
    /// first choice that allows is "Allow once"; a standing one comes after it.
    async fn approve(&mut self, wanted: impl Fn(&Choice) -> bool) -> Result<(), String> {
        let (target, choice) = {
            let (target, seen) = self
                .waiting(|b| matches!(b, Body::Approval(_)))
                .ok_or("no approval is waiting in this chat")?;
            let Body::Approval(approval) = seen.read.item.event.body() else {
                unreachable!()
            };
            let choice = approval
                .choices
                .iter()
                .find(|c| wanted(c))
                .ok_or("no such choice")?;
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
            let asked = item
                .questions()
                .first()
                .ok_or("the question asks nothing")?;
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
        self.items
            .iter()
            .filter(|(_, s)| s.group == group && s.sender == agent.as_str())
            .filter(|(_, s)| s.read.item.event.header().state == State::Waiting)
            .filter(|(_, s)| kind(s.read.item.event.body()))
            .max_by_key(|(_, s)| s.order)
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
        if message.kind == CHAT {
            println!("{name}: {}", clean(&message.plaintext));
            return;
        }
        if message.kind != ITEM_KIND {
            return;
        }
        let Some(read) = carrier::read_item(&message.plaintext, &message.tags) else {
            return;
        };
        let first = read
            .item
            .first
            .clone()
            .unwrap_or_else(|| EventKey(message.message_id_hex.clone()));
        if self
            .items
            .get(&first)
            .is_some_and(|known| known.read.item.version >= read.item.version)
        {
            return;
        }
        for line in lines_for(&read) {
            println!("{line}");
        }
        self.arrived += 1;
        let seen = Seen {
            read,
            group: hex::encode(message.group_id.as_slice()),
            sender: message.sender,
            order: self.arrived,
        };
        self.items.insert(first, seen);
    }
}

/// Text from the chat as it may be printed: no control characters, so nothing received can move
/// the cursor or overwrite what the person reads before they answer.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect()
}

/// The lines for an item's latest version: its type and state, title and plain rendering, and for
/// one that waits, what can be answered.
fn lines_for(read: &Read) -> Vec<String> {
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
    let mut lines = vec![format!("  [{kind} · {state}] {title}: {text}")];
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
            lines.push(format!(
                "    :allow, :deny or :choose <id>: {}",
                choices.join(", ")
            ));
        }
        Body::Question(question) => {
            for asked in question.questions() {
                let secret = if asked.secret { " (secret)" } else { "" };
                lines.push(format!("    ? {}{secret}", clean(&asked.text)));
                for option in &asked.options {
                    let (id, label) = (clean(&option.id), clean(&option.label));
                    lines.push(format!("      :pick {id}  {label}"));
                }
            }
            lines.push("    :answer <text> answers the first question".into());
        }
        _ => {}
    }
    lines
}
