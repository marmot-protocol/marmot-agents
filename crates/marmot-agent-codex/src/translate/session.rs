use super::{Known, Sent, Translator, native, show};
use crate::messages::{self, Envelope, ErrorReply, ListReply, Policy, Sandbox};
use marmot_agent_events::body::{Action, Error, Session, Setting};
use marmot_agent_events::{AgentEvent, Body, State};
use serde_json::value::RawValue;

impl Translator {
    pub(super) fn response(&mut self, m: &Envelope<'_>, out: &mut Vec<AgentEvent>) {
        let sent = m.id.as_ref().and_then(|id| self.requests.remove(id));
        let (method, owner) = match sent {
            Some(Sent { method, owner }) => (Some(method), owner),
            None => (None, (self.session.clone(), self.turn.clone())),
        };
        // A change of settings Codex took holds from now, whether or not Codex reports it again.
        let asked = m.id.as_ref().and_then(|id| self.asked_settings.remove(id));
        if let (Some((model, effort)), None) = (asked, m.error) {
            self.known.model = model.or(self.known.model.take());
            self.known.effort = effort.or(self.known.effort.take());
        }
        if let Some(error) = m.error {
            self.starts.retain(|(id, ..)| Some(id) != m.id.as_ref()); // that request starts no turn

            let Some(e) = self.read::<ErrorReply>("an error reply", error.get()) else {
                return;
            };
            // Numbered like every item the driver names, not by request id, which Codex reuses.
            let item = self.made_up("error");
            let body = Body::Error(Error {
                request: method.clone(),
                code: e.code,
                will_retry: None,
            });
            let text = e.message.unwrap_or_default();
            let kind = format!("error response to {}", show(method.as_deref()));
            let native = native(&kind, error.get());
            let title = "The harness refused a request";
            let state = State::Failed;
            // It belongs where the request was sent, whatever has finished or changed since.
            return self.emit_owned(out, owner, &item, body, state, title, &text, native);
        }
        // A reply holds a result or an error. Read from nothing, one without either would make
        // an event whose native message never arrived (D-007), so it is unreadable.
        let Some(result) = m.result.map(RawValue::get) else {
            self.unreadable += 1;
            return;
        };
        match method.as_deref() {
            Some("model/list") => {
                let Some(list) = self.read::<ListReply>("a reply", result) else {
                    return;
                };
                self.models = Some(
                    list.data
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|x| x.id)
                        .collect(),
                );
                self.publish_lists("model/list", result, out);
            }
            Some("collaborationMode/list") => {
                let Some(list) = self.read::<ListReply>("a reply", result) else {
                    return;
                };
                self.modes = Some(
                    list.data
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|x| x.mode)
                        .collect(),
                );
                self.publish_lists("collaborationMode/list", result, out);
            }
            // The person's next message answers a question asked in a message once Codex takes
            // it: one refused, or still on its way, answers nothing (#36).
            Some("turn/start" | "turn/steer") => {
                self.close_message_questions(State::Done, "Answered by the next message", out);
            }
            Some(started @ ("thread/start" | "thread/resume")) => {
                let Some(settings) = self.read::<messages::Settings>("a reply", result) else {
                    return;
                };
                if let Some(thread) = &settings.thread {
                    self.use_thread(thread.id.clone(), out);
                    let kind = format!("response to {started}");
                    self.thread_settings(&settings, &kind, result, out);
                }
            }
            _ => {}
        }
    }

    /// A list of models or modes that arrives after the session item reaches it.
    fn publish_lists(&mut self, method: &str, result: &str, out: &mut Vec<AgentEvent>) {
        if self.session.is_some() {
            let kind = format!("response to {method}");
            self.thread_settings(&messages::Settings::default(), &kind, result, out);
        }
    }

    /// A second thread replaces the first as this chat's session. Codex sends nothing to say the
    /// first is over, so the driver ends its session item.
    fn use_thread(&mut self, thread: Option<String>, out: &mut Vec<AgentEvent>) {
        if let Some(old) = self.session.clone()
            && thread.as_ref() != Some(&old)
        {
            let text = "Not answered: a new session started";
            self.close_message_questions(State::Cancelled, text, out);
            self.end_session(&old, out);
            self.known = Known::default();
            self.answered.clear();
            // A turn the old thread was asked to start is not this thread's.
            self.starts
                .retain(|(.., named)| named.as_ref() != Some(&old));
            // A session the chat goes back to runs again, though its settings may be unchanged.
            self.sessions
                .remove(&format!("session:{}", show(thread.as_deref())));
        }
        self.session = thread;
    }

    pub(super) fn thread_settings(
        &mut self,
        src: &messages::Settings,
        native_kind: &str,
        native_text_: &str,
        out: &mut Vec<AgentEvent>,
    ) {
        let sandbox = src
            .sandbox_policy
            .as_ref()
            .or(src.sandbox.as_ref())
            .and_then(|s| match s {
                Sandbox::Word(word) => Some(word.clone()),
                Sandbox::Object { kind } => kind.clone(),
            });
        let version = match &src.thread {
            Some(thread) => thread.cli_version.clone(),
            None => src.cli_version.clone(),
        };
        let known = &mut self.known;
        for (slot, value) in [
            (&mut known.version, version),
            (&mut known.model, src.model.clone()),
            (
                &mut known.effort,
                src.effort.clone().or(src.reasoning_effort.clone()),
            ),
            (
                &mut known.approval_policy,
                src.approval_policy.as_ref().and_then(|p| match p {
                    Policy::Word(word) => Some(word.clone()),
                    Policy::Object(kinds) => kinds.keys().next().cloned(),
                }),
            ),
            (&mut known.sandbox, sandbox),
            (
                &mut known.mode,
                src.collaboration_mode.as_ref().and_then(|m| m.mode.clone()),
            ),
        ] {
            if value.is_some() {
                *slot = value;
            }
        }
        let k = &self.known;
        let settings = vec![
            setting(
                "model",
                "Model",
                k.model.clone(),
                self.models.clone(),
                false,
            ),
            setting("effort", "Reasoning effort", k.effort.clone(), None, false),
            setting("mode", "Mode", k.mode.clone(), self.modes.clone(), false),
            setting(
                "approval_policy",
                "Approval policy",
                k.approval_policy.clone(),
                None,
                true,
            ),
            setting("sandbox", "Sandbox", k.sandbox.clone(), None, true),
        ];
        let session = Session {
            harness_version: k.version.clone().unwrap_or_default(),
            settings,
            actions: vec![Action {
                id: "compact".into(),
                label: "Compact the conversation".into(),
                description: None,
            }],
            commands: Vec::new(),
        };
        if self.subagent.is_some() {
            return; // a subagent's thread is no session of the chat's (D-117)
        }
        let sid = show(self.session.as_deref());
        let item = format!("session:{sid}");
        if self.sessions.get(&item) == Some(&session) {
            return; // a harness that repeats its settings makes no edit
        }
        self.sessions.insert(item.clone(), session.clone());
        let text = session
            .settings
            .iter()
            .filter_map(|s| s.value.as_ref().map(|v| format!("{}: {v}", s.label)))
            .collect::<Vec<_>>()
            .join(", ");
        let turn = self.turn.take(); // a session belongs to no turn
        let native = native(native_kind, native_text_);
        let body = Body::Session(session);
        self.emit(out, &item, body, State::Running, "Session", &text, native);
        self.turn = turn;
    }

    /// Nothing Codex sends says a session is over. The driver ends the item itself, when the
    /// chat's session is replaced.
    fn end_session(&mut self, sid: &str, out: &mut Vec<AgentEvent>) {
        let item = format!("session:{sid}");
        let Some(prev) = self.open_event(&item) else {
            return;
        };
        let turn = self.turn.take();
        let (body, text) = (prev.body().clone(), prev.header().text.clone());
        self.emit(out, &item, body, State::Done, "Session ended", &text, None);
        self.turn = turn;
    }
}

fn setting(
    id: &str,
    label: &str,
    value: Option<String>,
    options: Option<Vec<String>>,
    approve: bool,
) -> Setting {
    Setting {
        id: id.to_string(),
        label: label.to_string(),
        value,
        options,
        approve,
    }
}
