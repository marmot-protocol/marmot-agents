//! `marmot-agent-chat`: a terminal client that plays the person, for trying the core by hand.
//!
//! It holds one person's account, talks to the agent in a Marmot group, shows each of the agent's
//! items as it changes, and sends answers and controls. How it shows an item is a plain reference
//! for the apps. It knows the temporary carrier of D-069.
//!
//! A try on one machine, with nothing else running:
//!
//! ```text
//! marmot-agent-chat /tmp/person --serve 7777    # a relay on port 7777, and the person's account
//! marmot-agent init settings.toml               # relays = ["ws://127.0.0.1:7777"], owner = the
//! marmot-agent run settings.toml                #   person's key, allow_loopback_relays = true
//! :invite <the agent's key>                     # typed in the chat, then talk to it
//! ```
//!
//! Typed lines are messages, sent exactly as typed, slash commands included (D-031). Lines that
//! start with `:` are for this client:
//!
//! ```text
//! :invite <key>          start a direct chat with the agent
//! :group <name> <key>    make a named group with the agent in it
//! :allow   :deny         answer the approval that is waiting
//! :answer <text>         answer the question that is waiting
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
use marmot_agent_events::body::{Body, Effect};
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
    /// Each item's latest version, by its first event.
    items: HashMap<EventKey, Read>,
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
                let group = self
                    .runtime
                    .start_direct_conversation(&self.me, key)
                    .await
                    .map_err(
                        |_| "the agent could not be invited; is its key package on the relay?",
                    )?;
                self.chat = Some((group, PubKey::from(*key)));
                println!("· invited; waiting for the agent to accept");
                Ok(())
            }
            [":group", name, key] => {
                let made = self
                    .runtime
                    .create_group(&self.me, name, &[key.to_string()], None)
                    .await
                    .map_err(|_| "the group could not be made")?;
                let group = made.operation.ok_or("the agent could not be added")?;
                self.chat = Some((group, PubKey::from(*key)));
                println!("· group made; mention the agent with nostr:<its npub> to address it");
                Ok(())
            }
            [":allow"] => self.approve(Effect::Allow).await,
            [":deny"] => self.approve(Effect::Deny).await,
            [":answer", ..] => self.answer(line.trim_start_matches(":answer").trim()).await,
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

    /// Answers the approval that is waiting with its first choice of the given effect.
    async fn approve(&mut self, effect: Effect) -> Result<(), String> {
        let (target, choice) = self
            .waiting()
            .find_map(|(first, read)| match read.item.event.body() {
                Body::Approval(a) => {
                    let choice = a.choices.iter().find(|c| c.effect == effect)?;
                    Some((first.clone(), choice.clone()))
                }
                _ => None,
            })
            .ok_or("no approval is waiting")?;
        let asked = Asked::Choice { choice: choice.id };
        self.ask(Some(target), asked, &choice.label).await
    }

    /// Answers the first question of the question item that is waiting with typed text.
    async fn answer(&mut self, text: &str) -> Result<(), String> {
        let (target, question) = self
            .waiting()
            .find_map(|(first, read)| match read.item.event.body() {
                Body::Question(q) => Some((first.clone(), q.questions().first()?.id.clone())),
                _ => None,
            })
            .ok_or("no question is waiting")?;
        let reply = AskedReply {
            question,
            options: Vec::new(),
            text: Some(text.to_string()),
        };
        let asked = Asked::Questions {
            questions: vec![reply],
        };
        self.ask(Some(target), asked, "Answered").await
    }

    fn waiting(&self) -> impl Iterator<Item = (&EventKey, &Read)> {
        self.items
            .iter()
            .filter(|(_, r)| r.item.event.header().state == State::Waiting)
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
        let name = message
            .sender_display_name
            .clone()
            .unwrap_or_else(|| "agent".into());
        if message.kind == CHAT {
            println!("{name}: {}", message.plaintext);
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
            .is_some_and(|known| known.item.version >= read.item.version)
        {
            return;
        }
        println!("{}", line_for(&read));
        self.items.insert(first, read);
    }
}

/// One line for an item's latest version: its type and state, title and plain rendering.
fn line_for(read: &Read) -> String {
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
    let hint = match event.body() {
        Body::Approval(_) if header.state == State::Waiting => "  (:allow or :deny)",
        Body::Question(_) if header.state == State::Waiting => "  (:answer <text>)",
        _ => "",
    };
    let mut text = header.text.replace('\n', " ");
    if text.len() > 160 {
        let mut end = 160;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    format!("  [{kind} · {state}] {}: {text}{hint}", header.title)
}
