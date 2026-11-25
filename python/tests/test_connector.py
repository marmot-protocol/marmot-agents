"""The Hermes plugin's transport, without Hermes: the connector against a stand-in for Hermes's
adapter, and the socket connection against a stand-in for the core.

What Hermes calls is written as the recordings hold it (`fixtures/recordings/hermes`)."""

import asyncio
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from marmot_hermes.connector import EFFECT_SECONDS, Connector
from marmot_hermes.core import PROTOCOL, CoreConnection, Refusal, Refused
from protocol import Reads, core_requests, event_problems, problems, unknown_reads

CHAT = {"id": "c-1", "group": False, "name": "Alice"}
GROUP = {"id": "c-2", "group": True, "name": "Family"}
ALICE = {"id": "u-1", "name": "Alice"}
BOB = {"id": "u-2", "name": "Bob"}
# Hermes's replies, by their keys in its catalog, as the pinned Hermes renders them in English.
PHRASES = json.loads((Path(__file__).parent / "hermes_phrases.json").read_text(encoding="utf-8"))["phrases"]


def phrase(key, **fields):
    """What Hermes's `agent.i18n.t(key, **fields)` says, as the pinned Hermes says it."""
    return PHRASES.get(key, key).format(**fields)


def switched(model):
    """Hermes's reply to a `/model` it took (`_model_switch_confirmation`), its first lines."""
    return "\n".join([phrase("gateway.model.switched", model=model),
                      phrase("gateway.model.provider_label", provider="Custom endpoint"),
                      phrase("gateway.model.session_only_hint")])


class FakeHermes:
    """Records what the connector asks of Hermes. `answer(text, message_id)`, when set, plays what
    Hermes does with a typed message."""

    def __init__(self):
        self.typed = []
        self.cards = []
        self.resolves = 1
        self.questions = []
        self.question_answer = True
        self.answer = None
        self.fails = None  # an error inject raises: Hermes did not take the message
        self.model_reads = []
        self.read_model = None  # what Hermes says when a session's model is read
        self.confirms = []  # each answer to a confirmation, as the plugin gave it
        self.waiting_confirm = {}  # Hermes's session key -> the confirmation it waits on there
        self.confirmed = ""  # what Hermes says once a confirmation is answered: words, or words for a choice
        self.refuses = {}  # a person Hermes does not let answer a confirmation -> its words
        self.pause = None  # what a confirmed command waits on before it finishes, if anything
        self.look_fails = None  # an error reading which confirmation waits raises

    async def inject(self, chat, sender, message_id, text, reply_to):
        if self.fails is not None:
            raise self.fails
        self.typed.append((chat["id"], sender["id"], message_id, text))
        if self.answer is not None:
            self.answer(text, message_id)

    def resolve_card(self, session_key, choice, request_id, actor):
        if actor in self.refuses:
            raise Refusal("not_supported", self.refuses[actor])
        self.cards.append((session_key, choice, request_id, actor))
        return self.resolves

    def resolve_question(self, clarify_id, text, actor):
        if actor in self.refuses:
            raise Refusal("not_supported", self.refuses[actor])
        self.questions.append((clarify_id, text))
        if isinstance(self.question_answer, Exception):
            raise self.question_answer
        return self.question_answer

    def resolve_confirm(self, session_key, confirm_id, choice, actor):
        """As `tools.slash_confirm.resolve`: only the confirmation Hermes waits on in that session, by its
        id, is answered; any other answers nothing. Hermes's own check of who may answer refuses each
        person in `refuses`, and the confirmation still waits. Hermes takes it at once, and what it says
        comes once the command finishes, after `pause` when that is set."""
        self.confirms.append((session_key, confirm_id, choice, actor))
        if self.waiting_confirm.get(session_key) != confirm_id:
            return None
        if actor in self.refuses:
            raise Refusal("not_supported", self.refuses[actor])
        said = self.confirmed(choice) if callable(self.confirmed) else self.confirmed
        del self.waiting_confirm[session_key]

        async def finished():
            if self.pause is not None:
                await self.pause.wait()
            if isinstance(said, BaseException):
                raise said
            return said
        return asyncio.Task(finished(), loop=asyncio.get_running_loop(), eager_start=True)

    def confirm_waiting(self, session_key):
        """As `tools.slash_confirm.get_pending`, by Hermes's own limit: the id of the confirmation that waits
        in that session, or None."""
        if self.look_fails is not None:
            raise self.look_fails
        return self.waiting_confirm.get(session_key)

    async def read_model_settings(self, chat, sender):
        self.model_reads.append((chat["id"], sender["id"]))
        if self.read_model is not None:
            self.read_model()

    def phrase(self, key, **fields):
        return phrase(key, **fields)


def hook(method, **params):
    return {"dir": "in", "via": "hook", "msg": {"method": method, "params": params}}


def started(message_id, text="hello", chat="c-1", user="u-1", chat_type="dm"):
    source = {"_type": "SessionSource", "platform": "marmot", "chat_id": chat, "chat_type": chat_type, "user_id": user}
    return hook("on_processing_start", event={"_type": "MessageEvent", "text": text, "message_id": message_id,
                                              "source": source})


def complete(message_id, outcome="success"):
    return hook("on_processing_complete", message_id=message_id, outcome=outcome)


def said(content, reply_to=None, notify=False, chat="c-1"):
    return hook("send", chat_id=chat, content=content, reply_to=reply_to, metadata={"notify": True} if notify else None)


def card(request_id, command, session_key="agent:main:marmot:dm:c-1", chat="c-1"):
    prompt = {"_type": "ExecApprovalPrompt", "chat_id": chat, "session_key": session_key,
              "command": command, "description": "delete in root path", "request_id": request_id,
              "actions": [["Allow Once", "once", "primary"], ["Allow Session", "session", None],
                          ["Always Allow", "always", None], ["Deny", "deny", "danger"]]}
    return hook("_send_exec_approval_prompt", prompt=prompt)


KEY = "agent:main:marmot:dm:c-1"
YES = {"questions": [{"question": "q0", "options": ["o0"]}]}


def prompt(message, text="hello", chat=CHAT, sender=ALICE):
    return {"message": message, "text": text, "chat": chat, "sender": sender, "attachments": []}


def model_settings(user="u-1", model="m", read=None):
    """A read of a session's model and Hermes's answer, the plugin's `read` id on both when given. Hermes
    names a direct chat's session after the chat alone, as `fixtures/recordings/hermes` shows, and a
    group's after the chat and the sender."""
    source = {"chat_id": "c-1", "chat_type": "dm", "user_id": user}
    named = {} if read is None else {"id": read}
    return [{"dir": "out", "via": "call", "msg": {"method": "get_session_model_settings", **named,
                                                   "params": {"source": source}}},
            {"dir": "in", "via": "call", "msg": {"method": "get_session_model_settings", **named, "result": {
                "session_key": "agent:main:marmot:dm:c-1", "model": model, "provider": "custom"}}}]


class Connected(unittest.IsolatedAsyncioTestCase):
    """The connector, served the core's requests and sending its events. Each request the tests serve
    as the core's, and each event the connector sends, is checked against the core's own schema
    (`fixtures/socket`); what is not as the core's types say is kept in `invalid`, which every test ends
    by finding empty."""

    async def asyncSetUp(self):
        self.hermes = FakeHermes()
        self.sent = []
        self.invalid = []
        self.folder = tempfile.TemporaryDirectory()
        self.sessions = Path(self.folder.name) / "sessions.json"
        serve = Connector.serve

        async def checked(connector, method, params):
            message = {"jsonrpc": "2.0", "id": 1, "method": method, "params": json.loads(json.dumps(params))}
            self.invalid += [f"a {method} as the core's: {p}" for p in problems("FromCore", message)]
            return await serve(connector, method, params)

        patch = mock.patch.object(Connector, "serve", checked)
        patch.start()
        self.addCleanup(patch.stop)
        self.connector = Connector(self.hermes, self.send, generations=self.sessions, hold=0.05)

    def send(self, event):
        self.invalid += [f"an event: {p}" for p in event_problems(json.loads(json.dumps(event)))]
        self.sent.append(event)

    async def asyncTearDown(self):
        self.folder.cleanup()
        self.assertEqual(self.invalid, [], "every message either way is as the core's types say")

    def feed(self, *lines, chat="c-1"):
        for line in lines:
            self.connector.record(line, chat)

    def resets(self, reply=phrase("gateway.reset.header_default")):
        """Hermes answers a typed /new as it does: a turn for the command, and its reply."""
        def answer(text, message_id):
            if text == "/new":
                self.feed(started(message_id, "/new"), said(reply, message_id, notify=True), complete(message_id))
        self.hermes.answer = answer

    def of(self, type_):
        return [e for e in self.sent if e["type"] == type_]

    def asks_to_confirm(self, typed, title, asking, confirm_id, session_key=KEY):
        """Hermes asks to confirm the command typed as `typed`, as `fixtures/recordings/hermes/new_session.jsonl`
        shows: it registers the confirmation, offers it as a card, then sends the same words as its reply."""
        self.hermes.waiting_confirm[session_key] = confirm_id
        self.feed(hook("send_slash_confirm", chat_id="c-1", title=title, message=asking, session_key=session_key,
                       confirm_id=confirm_id, metadata=None),
                  said(asking, typed, notify=True))

    async def test_a_prompt_is_typed_and_its_turn_names_it(self):
        await self.connector.serve("prompt", prompt("m-1"))
        self.assertEqual(self.hermes.typed, [("c-1", "u-1", "m-1", "hello")])
        self.feed(started("m-1"), hook("send_final_ledgered", event={}), said("Hi.", "m-1", notify=True),
                  complete("m-1"))
        turns = self.of("turn")
        self.assertEqual(turns[0]["prompts"], ["m-1"])
        self.assertEqual([t["state"] for t in turns], ["running", "done"])
        self.assertTrue(all("prompts" not in t for t in turns[1:]))
        message = self.of("message")[0]
        self.assertEqual((message["text"], message["chat"], message["turn"]), ("Hi.", "c-1", "m-1"))
        self.assertTrue(all("harness" not in e and "seq" not in e for e in self.sent))

    async def test_a_turn_reported_done_and_started_again_shows_no_end(self):
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), complete("m-1"))
        self.feed(started("m-1"), said("Hi.", "m-1", notify=True), complete("m-1"))
        await asyncio.sleep(0.1)
        self.assertEqual([t["state"] for t in self.of("turn")], ["running", "done"])

    async def test_the_session_item_does_not_release_a_held_end(self):
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), complete("m-1"),
                  {"dir": "out", "via": "call", "msg": {"method": "get_session_model_settings", "params": {
                      "source": {"chat_id": "c-1", "chat_type": "dm", "user_id": "u-1"}}}},
                  {"dir": "in", "via": "call", "msg": {"method": "get_session_model_settings", "result": {
                      "session_key": "agent:main:marmot:dm:c-1", "model": "m", "provider": "custom"}}},
                  started("m-1"), said("Hi.", "m-1", notify=True), complete("m-1"))
        await asyncio.sleep(0.1)
        self.assertEqual([t["state"] for t in self.of("turn")], ["running", "done"])
        self.assertEqual(self.of("session")[-1]["text"], "Model: m, Provider: custom")

    async def test_a_quiet_end_is_sent_once_nothing_follows(self):
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), complete("m-1"))
        self.assertEqual([t["state"] for t in self.of("turn")], ["running"])
        await asyncio.sleep(0.1)
        self.assertEqual([t["state"] for t in self.of("turn")], ["running", "done"])

    async def test_an_approval_is_answered_on_its_card_and_settled_by_its_edit(self):
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), card("rq", "rm -rf /work/x"))
        approval = self.of("approval")[0]
        self.assertEqual(approval["text"], "rm -rf /work/x")
        self.assertEqual([c["effect"] for c in approval["body"]["choices"]], ["allow", "allow", "allow", "deny"])
        await self.connector.serve("answer", {"request": "r-1", "item": approval["item"], "by": ALICE,
                                              "given": {"choice": "c0"}})
        self.assertEqual(self.hermes.cards, [("agent:main:marmot:dm:c-1", "once", "rq", "u-1")])
        answered = self.of("approval")[-1]
        self.assertEqual((answered["state"], answered["settles"]), ("done", ["r-1"]))
        with self.assertRaises(Refusal) as again:
            await self.connector.serve("answer", {"request": "r-2", "item": approval["item"], "by": ALICE,
                                                  "given": {"choice": "c3"}})
        self.assertEqual(again.exception.reason, "already_answered")

    async def test_an_approval_hermes_does_not_let_that_person_answer_still_waits(self):
        """D-151. Hermes's own check of who may answer the card refuses Bob, with Hermes's words, and the
        approval waits for someone it lets answer."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), card("rq", "rm -rf /work/x"))
        self.hermes.refuses = {"u-2": "⛔ /approve is admin-only here."}
        with self.assertRaises(Refusal) as refused:
            await self.connector.serve("answer", {"request": "r-1", "item": "approval:rq", "by": BOB,
                                                  "given": {"choice": "c0"}})
        self.assertEqual((refused.exception.reason, refused.exception.detail), ("not_supported", "⛔ /approve is admin-only here."))
        self.assertEqual((self.hermes.cards, self.of("approval")[-1]["state"]), ([], "waiting"))
        await self.connector.serve("answer", {"request": "r-2", "item": "approval:rq", "by": ALICE, "given": {"choice": "c0"}})
        self.assertEqual(self.of("approval")[-1]["settles"], ["r-2"])

    async def test_an_approval_hermes_no_longer_holds_is_refused(self):
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), card("rq", "rm -rf /work/x"))
        self.hermes.resolves = 0
        with self.assertRaises(Refusal) as refused:
            await self.connector.serve("answer", {"request": "r-1", "item": "approval:rq", "by": ALICE,
                                                  "given": {"choice": "c0"}})
        self.assertEqual(refused.exception.reason, "not_waiting")
        with self.assertRaises(Refusal) as unknown:
            await self.connector.serve("answer", {"request": "r-2", "item": "approval:other", "by": ALICE,
                                                  "given": {"choice": "c0"}})
        self.assertEqual(unknown.exception.reason, "unknown_item")

    async def test_an_interrupt_is_typed_as_stop_and_settled_by_the_turns_end(self):
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), card("rq", "rm -rf /work/x"))
        await self.connector.serve("control", {"request": "r-1", "by": ALICE,
                                               "action": {"type": "interrupt", "turn": "m-1"}})
        stop = self.hermes.typed[-1]
        self.assertEqual((stop[0], stop[1], stop[3]), ("c-1", "u-1", "/stop"))
        self.feed(hook("send", chat_id="c-1", content="⚡ Stopped. You can continue this session.", reply_to=stop[2],
                       metadata={"notify": True}), complete("m-1", "cancelled"))
        end = self.of("turn")[-1]
        self.assertEqual((end["state"], end["settles"]), ("cancelled", ["r-1"]))
        self.assertEqual(self.of("approval")[-1]["state"], "cancelled")
        self.assertEqual(self.of("notice")[-1]["body"], {"kind": "command_output"})
        with self.assertRaises(Refusal) as nothing:
            await self.connector.serve("control", {"request": "r-2", "by": ALICE,
                                                   "action": {"type": "interrupt", "turn": "m-1"}})
        self.assertEqual(nothing.exception.reason, "nothing_to_do")

    async def test_a_new_session_is_typed_and_settled_by_the_old_sessions_end(self):
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), said("Hi.", "m-1", notify=True), complete("m-1"))
        session = self.of("session")[0]["session"]
        self.resets()
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "new_session", "chat": CHAT, "session": session}})
        self.assertEqual(self.hermes.typed[-1][3], "/new")
        ended = [e for e in self.of("session") if e["state"] == "done"]
        self.assertEqual((ended[0]["session"], ended[0]["settles"]), (session, ["r-1"]))
        fresh = self.of("session")[-1]["session"]
        self.assertNotEqual(fresh, session)
        # A plugin started again names the new session as before: the count is kept.
        self.assertEqual(json.loads(self.sessions.read_text()), {"dm:c-1:u-1": 2})
        again = Connector(FakeHermes(), self.send, generations=self.sessions, hold=0)
        again.record(started("m-9"), "c-1")
        self.assertEqual(self.sent[-1]["session"], fresh)

    async def test_a_session_paused_by_a_lost_turn_is_resumed(self):
        await self.connector.serve("prompt", prompt("m-5"))
        self.feed(started("m-5"), said("⏸️ This conversation is paused: a previous turn did not finish when Hermes "
                                       "restarted, and only you know whether to send it again.", "m-5", notify=True))
        await asyncio.sleep(0)
        self.assertEqual(self.hermes.typed[-1][:2] + self.hermes.typed[-1][3:], ("c-1", "u-1", "/discard"))
        self.assertEqual(self.of("notice")[-1]["body"], {"kind": "warning"})

    async def test_ids_do_not_repeat_after_a_restart(self):
        first = Connector(FakeHermes(), self.send, hold=0)
        second = Connector(FakeHermes(), self.send, hold=0)
        for connector in (first, second):
            connector.record(started("m-1"), "c-1")
            connector.record(said("Hi."), "c-1")
        messages = self.of("message")
        self.assertNotEqual(messages[0]["item"], messages[1]["item"])

    async def asked(self):
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), hook("send_clarify", chat_id="c-1", question="Which?", choices=["Hello", "Hi"],
                                        clarify_id="q1", session_key="agent:main:marmot:dm:c-1"))
        return self.of("question")[0]["item"]

    def answer(self, request="r-1"):
        return self.connector.serve("answer", {"request": request, "item": "question:q1", "by": ALICE,
                                               "given": {"questions": [{"question": "q0", "options": ["o1"]}]}})

    async def test_a_question_is_answered_through_hermes_and_closed_once_taken(self):
        await self.asked()
        await self.answer()
        self.assertEqual(self.hermes.questions, [("q1", "Hi")])
        answered = self.of("question")[-1]
        self.assertEqual((answered["state"], answered["settles"]), ("done", ["r-1"]))
        self.assertEqual(answered["body"]["answer"], [{"question": "q0", "options": ["o1"]}])

    async def test_a_question_hermes_did_not_take_stays_open(self):
        await self.asked()
        self.hermes.question_answer = RuntimeError("Hermes failed")
        with self.assertRaises(RuntimeError):
            await self.answer()
        self.assertEqual(self.of("question")[-1]["state"], "waiting")
        # Nothing was taken, so the answer can be given again.
        self.hermes.question_answer = True
        await self.answer("r-2")
        self.assertEqual(self.of("question")[-1]["settles"], ["r-2"])

    async def test_a_question_hermes_does_not_let_that_person_answer_still_waits(self):
        """D-151. Hermes's own check of who may answer refuses Bob, with Hermes's words, and the question
        waits for someone it lets answer."""
        await self.asked()
        self.hermes.refuses = {"u-2": "Only Alice can answer this."}
        with self.assertRaises(Refusal) as refused:
            await self.connector.serve("answer", {"request": "r-1", "item": "question:q1", "by": BOB,
                                                  "given": {"questions": [{"question": "q0", "options": ["o1"]}]}})
        self.assertEqual((refused.exception.reason, refused.exception.detail), ("not_supported", "Only Alice can answer this."))
        self.assertEqual((self.hermes.questions, self.of("question")[-1]["state"]), ([], "waiting"))
        await self.answer("r-2")
        self.assertEqual(self.of("question")[-1]["settles"], ["r-2"])

    async def test_free_text_that_only_looks_like_a_number_is_free_text(self):
        await self.asked()
        await self.connector.serve("answer", {"request": "r-1", "item": "question:q1", "by": ALICE, "given": {
            "questions": [{"question": "q0", "options": [], "text": "²"}]}})
        self.assertEqual(self.hermes.questions, [("q1", "²")])
        answered = self.of("question")[-1]
        self.assertEqual((answered["state"], answered["settles"]), ("done", ["r-1"]))
        self.assertEqual(answered["body"]["answer"], [{"question": "q0", "text": "²"}])

    async def test_a_question_no_longer_waiting_in_hermes_is_refused(self):
        await self.asked()
        self.hermes.question_answer = False
        with self.assertRaises(Refusal) as refused:
            await self.answer()
        self.assertEqual(refused.exception.reason, "not_waiting")
        self.assertEqual(self.of("question")[-1]["state"], "waiting")

    async def test_stop_is_not_the_answer_to_a_question(self):
        await self.asked()
        await self.connector.serve("control", {"request": "r-1", "by": ALICE,
                                               "action": {"type": "interrupt", "turn": "m-1"}})
        self.assertEqual(self.of("question")[-1]["state"], "waiting")
        self.feed(complete("m-1", "cancelled"))
        closed = self.of("question")[-1]
        self.assertEqual(closed["state"], "cancelled")
        self.assertNotIn("answer", closed["body"])

    async def test_two_senders_turns_and_approvals_stay_apart(self):
        for message, sender in (("m-a", ALICE), ("m-b", BOB)):
            await self.connector.serve("prompt", prompt(message, chat=GROUP, sender=sender))
        self.feed(started("m-a", chat="c-2", user="u-1", chat_type="group"),
                  started("m-b", chat="c-2", user="u-2", chat_type="group"),
                  card("ra", "rm -rf /work/a", "agent:main:marmot:group:c-2:u-1", "c-2"),
                  card("rb", "rm -rf /work/b", "agent:main:marmot:group:c-2:u-2", "c-2"), chat="c-2")
        cards = {e["item"]: e for e in self.of("approval")}
        self.assertEqual((cards["approval:ra"]["turn"], cards["approval:ra"]["session"]), ("m-a", "group:c-2:u-1"))
        self.assertEqual((cards["approval:rb"]["turn"], cards["approval:rb"]["session"]), ("m-b", "group:c-2:u-2"))
        await self.connector.serve("answer", {"request": "r-1", "item": "approval:ra", "by": ALICE,
                                              "given": {"choice": "c0"}})
        self.assertEqual(self.hermes.cards[-1][:3], ("agent:main:marmot:group:c-2:u-1", "once", "ra"))
        answered = self.of("approval")[-1]
        self.assertEqual((answered["item"], answered["state"], answered["settles"], answered["turn"]),
                         ("approval:ra", "done", ["r-1"], "m-a"))
        self.feed(said("Done.", "m-a", notify=True, chat="c-2"), complete("m-a"), chat="c-2")
        self.assertEqual(self.of("message")[-1]["turn"], "m-a")
        ends = [(t["turn"], t["state"]) for t in self.of("turn") if t["state"] != "running"]
        self.assertEqual(ends, [("m-a", "done")])
        latest = {}
        for e in self.of("approval"):
            latest[e["item"]] = e["state"]
        self.assertEqual(latest, {"approval:ra": "done", "approval:rb": "waiting"})
        self.feed(complete("m-b", "cancelled"), chat="c-2")
        self.assertEqual(self.of("approval")[-1]["item"], "approval:rb")
        self.assertEqual(self.of("approval")[-1]["state"], "cancelled")
        self.assertEqual(self.of("turn")[-1]["turn"], "m-b")

    async def test_a_reset_after_a_restart_is_typed_by_the_sessions_own_sender(self):
        await self.connector.serve("prompt", prompt("m-a", chat=GROUP, sender=ALICE))
        self.feed(started("m-a", chat="c-2", user="u-1", chat_type="group"), complete("m-a"), chat="c-2")
        session = self.of("session")[0]["session"]
        # The plugin starts again, and Bob asks for a new session of Alice's before she says anything.
        self.connector = Connector(self.hermes, self.send, generations=self.sessions, hold=0)
        self.sent.clear()

        def answer(text, message_id, reply="⛔ /new is admin-only here."):
            if text == "/new":
                self.feed(started(message_id, "/new", "c-2", "u-1", "group"), said(reply, message_id, True, "c-2"),
                          complete(message_id), chat="c-2")
        self.hermes.answer = answer
        await self.connector.serve("control", {"request": "r-1", "by": BOB, "action": {
            "type": "new_session", "chat": GROUP, "session": session}})
        self.assertEqual(self.hermes.typed[-1][:2] + self.hermes.typed[-1][3:], ("c-2", "u-1", "/new"))
        # Hermes refused: its words settle the request, and no session ends.
        settled = [e for e in self.sent if "settles" in e]
        self.assertEqual([(e["type"], e["text"], e["settles"]) for e in settled],
                         [("notice", "⛔ /new is admin-only here.", ["r-1"])])
        self.hermes.answer = lambda text, message_id: answer(text, message_id, phrase("gateway.reset.header_default"))
        await self.connector.serve("control", {"request": "r-2", "by": BOB, "action": {
            "type": "new_session", "chat": GROUP, "session": session}})
        settled = [e for e in self.sent if "settles" in e][1:]
        self.assertEqual(len(settled), 1)
        self.assertEqual((settled[0]["type"], settled[0]["settles"]), ("session", ["r-2"]))
        self.assertEqual((settled[0]["session"], settled[0]["state"]), (session, "done"))
        self.assertEqual(self.of("session")[-1]["session"], f"{session}#2")
        with self.assertRaises(Refusal) as elsewhere:
            await self.connector.serve("control", {"request": "r-3", "by": BOB, "action": {
                "type": "new_session", "chat": GROUP, "session": "dm:c-1:u-1"}})
        self.assertEqual(elsewhere.exception.reason, "nothing_to_do")

    async def test_a_reset_reaches_the_session_of_the_sender_it_was_typed_as(self):
        for message, sender in (("m-a", ALICE), ("m-b", BOB)):
            await self.connector.serve("prompt", prompt(message, chat=GROUP, sender=sender))
        self.feed(started("m-a", chat="c-2", user="u-1", chat_type="group"),
                  started("m-b", chat="c-2", user="u-2", chat_type="group"), chat="c-2")

        # Hermes answers the /new typed as Alice while Bob's turn is the latest, and starts no turn for it.
        def answer(text, message_id):
            if text == "/new":
                self.feed(said(phrase("gateway.reset.header_default"), message_id, True, "c-2"), chat="c-2")
        self.hermes.answer = answer
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "new_session", "chat": GROUP, "session": "group:c-2:u-1"}})
        self.assertEqual(self.hermes.typed[-1][:2] + self.hermes.typed[-1][3:], ("c-2", "u-1", "/new"))
        settled = [e for e in self.sent if "settles" in e]
        self.assertEqual([(e["session"], e["state"], e["settles"]) for e in settled],
                         [("group:c-2:u-1", "done", ["r-1"])])
        self.assertEqual(json.loads(self.sessions.read_text()), {"group:c-2:u-1": 2, "group:c-2:u-2": 1})
        self.assertFalse([e for e in self.of("session") if e["session"].startswith("group:c-2:u-2#")
                          or (e["session"] == "group:c-2:u-2" and e["state"] == "done")])

    async def test_session_items_list_hermes_commands(self):
        self.connector.commands_read([{"name": "new", "description": "Start a new session", "args_hint": "",
                                       "gateway_available": True},
                                      {"name": "quit", "description": "Exit", "gateway_available": False}])
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"))
        commands = self.of("session")[-1]["body"]["commands"]
        self.assertEqual(commands, [{"name": "new", "description": "Start a new session", "hint": ""}])

    async def test_a_new_session_reads_its_model_again(self):
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"))
        self.assertTrue(self.connector.wants_model("dm", "c-1", "u-1"))
        self.feed(*model_settings(), complete("m-1"))
        self.assertFalse(self.connector.wants_model("dm", "c-1", "u-1"))
        self.resets()
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "new_session", "chat": CHAT, "session": "dm:c-1:u-1"}})
        await self.connector.serve("prompt", prompt("m-2"))
        self.feed(started("m-2"))
        self.assertTrue(self.connector.wants_model("dm", "c-1", "u-1"))
        self.feed(*model_settings())
        fresh = self.of("session")[-1]
        self.assertEqual((fresh["session"], fresh["text"]), ("dm:c-1:u-1#2", "Model: m, Provider: custom"))

    async def test_a_held_end_does_not_settle_a_later_stop(self):
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), complete("m-1"))
        await self.connector.serve("control", {"request": "r-1", "by": ALICE,
                                               "action": {"type": "interrupt", "turn": "m-1"}})
        self.assertEqual(self.hermes.typed[-1][3], "/stop")
        await asyncio.sleep(0.1)
        # Hermes had finished the turn before the stop: the core settles it as having no effect (D-087).
        end = self.of("turn")[-1]
        self.assertEqual((end["turn"], end["state"]), ("m-1", "done"))
        self.assertFalse([e for e in self.sent if "settles" in e])

    async def test_stop_while_an_end_is_held_settles_on_the_restarted_turns_end(self):
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), complete("m-1"))
        await self.connector.serve("control", {"request": "r-1", "by": ALICE,
                                               "action": {"type": "interrupt", "turn": "m-1"}})
        self.feed(started("m-1"), complete("m-1", "cancelled"))
        end = self.of("turn")[-1]
        self.assertEqual((end["turn"], end["state"], end["settles"]), ("m-1", "cancelled", ["r-1"]))

    def asks_in_text(self, message="m-1"):
        """Hermes asks for an approval in text, as it does without buttons: answered by typing."""
        self.feed(started(message), said("⚠️ **Hermes wants to run** `rm -rf /work/x`", message, notify=True))
        return self.of("approval")[-1]["item"]

    async def test_a_new_connection_starts_from_what_the_new_core_knows(self):
        """When the connection goes, the core closes every item open in the chat and settles every
        request it handed over (D-058, D-070). Hermes goes on, and keeps one connector across connections:
        what it says next of those items, a turn's end it held, and the requests the old core settled
        reach the next core not at all. What is new still does."""
        await self.connector.serve("prompt", prompt("m-1"))
        approval = self.asks_in_text("m-1")
        await self.connector.serve("answer", {"request": "r-1", "item": approval, "by": ALICE,
                                              "given": {"choice": "c0"}})
        await self.connector.serve("control", {"request": "r-2", "by": ALICE,
                                               "action": {"type": "interrupt", "turn": "m-1"}})
        # A quiet turn in another chat, whose end is held.
        await self.connector.serve("prompt", prompt("m-2", chat=GROUP, sender=BOB))
        self.feed(started("m-2", chat="c-2", user="u-2", chat_type="group"), complete("m-2"), chat="c-2")
        held = self.of("turn")[-1]["item"]
        turn = self.of("turn")[0]["item"]
        self.connector.core_gone()
        self.sent.clear()
        self.feed(said("✅ Command approved", "m-1"), said("Done.", "m-1", notify=True), complete("m-1", "cancelled"))
        await asyncio.sleep(0.1)
        self.assertEqual([e for e in self.sent if e["item"] in (approval, turn, held)], [])
        self.assertEqual([e for e in self.sent if "settles" in e], [])
        self.assertEqual([e["text"] for e in self.of("message")], ["Done."])
        with self.assertRaises(Refusal) as closed:
            await self.connector.serve("answer", {"request": "r-3", "item": approval, "by": ALICE,
                                                  "given": {"choice": "c0"}})
        self.assertEqual(closed.exception.reason, "not_waiting")
        await self.connector.serve("prompt", prompt("m-3"))
        self.feed(started("m-3"))
        self.assertEqual(self.of("turn")[-1]["prompts"], ["m-3"])

    async def test_an_answer_hermes_did_not_take_can_be_given_again(self):
        await self.connector.serve("prompt", prompt("m-1"))
        approval = self.asks_in_text("m-1")
        self.hermes.fails = OSError("Hermes did not take it")
        with self.assertRaises(Exception):
            await self.connector.serve("answer", {"request": "r-1", "item": approval, "by": ALICE,
                                                  "given": {"choice": "c0"}})
        self.hermes.fails = None
        await self.connector.serve("answer", {"request": "r-2", "item": approval, "by": ALICE,
                                              "given": {"choice": "c0"}})
        self.feed(said("✅ Command approved", "m-1"))
        answered = self.of("approval")[-1]
        self.assertEqual((answered["state"], answered["settles"]), ("done", ["r-2"]))

    async def test_a_new_session_is_taken_once_typed_and_settled_by_what_hermes_says(self):
        """The core waits 30 s for a request to be taken (`ANSWER_WITHIN`), and a request that waits holds
        up the next. A control is taken once Hermes has it; what Hermes says of it then settles it: the
        end of the session it replaced, or its words when it did not."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), said("Hi.", "m-1", notify=True), complete("m-1"))
        session = self.of("session")[0]["session"]
        await asyncio.wait_for(self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "new_session", "chat": CHAT, "session": session}}), 1)
        typed = self.hermes.typed[-1]
        self.assertEqual(typed[3], "/new")
        self.assertEqual([e for e in self.sent if "settles" in e], [])
        self.feed(said("⛔ /new is admin-only here.", typed[2], notify=True))
        refused = self.of("notice")[-1]
        self.assertEqual((refused["text"], refused["settles"]), ("⛔ /new is admin-only here.", ["r-1"]))
        await asyncio.wait_for(self.connector.serve("control", {"request": "r-2", "by": ALICE, "action": {
            "type": "new_session", "chat": CHAT, "session": session}}), 1)
        typed = self.hermes.typed[-1]
        self.feed(started(typed[2], "/new"), said(phrase("gateway.reset.header_default"), typed[2], notify=True),
                  complete(typed[2]))
        settled = [e for e in self.sent if "settles" in e]
        self.assertEqual([(e["type"], e["session"], e["state"], e["settles"]) for e in settled[1:]],
                         [("session", session, "done", ["r-2"])])

    async def test_a_model_is_taken_once_typed_and_settled_by_the_session_that_shows_it(self):
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), *model_settings(model="m"), complete("m-1"))
        session = self.of("session")[-1]["session"]
        self.hermes.read_model = lambda: self.feed(*model_settings(model="n"))
        await asyncio.wait_for(self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "set", "session": session, "setting": "model", "value": "n"}}), 1)
        typed = self.hermes.typed[-1]
        self.assertEqual(typed[3], "/model n")
        self.assertEqual(self.hermes.model_reads, [])
        self.feed(said(switched("n"), typed[2], notify=True))
        await asyncio.sleep(0)
        self.assertEqual(self.hermes.model_reads, [("c-1", "u-1")])
        shown = self.of("session")[-1]
        self.assertEqual((shown["text"], shown["settles"]), ("Model: n, Provider: custom", ["r-1"]))
        # A model Hermes does not switch to is settled by its words.
        await asyncio.wait_for(self.connector.serve("control", {"request": "r-2", "by": ALICE, "action": {
            "type": "set", "session": session, "setting": "model", "value": "x"}}), 1)
        self.feed(said(phrase("gateway.model.error_prefix", error="Unknown model `x`."), self.hermes.typed[-1][2], notify=True))
        self.assertEqual(self.of("notice")[-1]["settles"], ["r-2"])

    async def test_a_reset_hermes_asks_to_confirm_is_settled_by_the_session_it_replaced(self):
        """Hermes asks before a reset (`fixtures/recordings/hermes/new_session.jsonl`): a confirm card, then
        its words again as the reply to the command. That reply is a question, not the outcome. The answer
        goes to that confirmation by its id, and nothing is typed (D-146); the reset that follows the
        answer settles the request."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), said("Hi.", "m-1", notify=True), complete("m-1"))
        session = self.of("session")[0]["session"]
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "new_session", "chat": CHAT, "session": session}})
        typed = self.hermes.typed[-1]
        self.feed(started(typed[2], "/new"))
        self.asks_to_confirm(typed[2], "/new", "⚠️ **Confirm /new**\n\nThis starts a fresh session.", "k-1")
        self.feed(complete(typed[2]))
        self.assertEqual([e for e in self.sent if "settles" in e], [])
        self.hermes.confirmed = phrase("gateway.reset.header_default")
        before = list(self.hermes.typed)
        await self.connector.serve("answer", {"request": "r-2", "item": "confirm:k-1", "by": ALICE, "given": YES})
        self.assertEqual(self.hermes.confirms, [(KEY, "k-1", "once", "u-1")])
        self.assertEqual(self.hermes.typed, before, "nothing typed for the answer")
        settled = [(e["item"], e["state"], e["settles"]) for e in self.sent if "settles" in e]
        self.assertEqual(settled, [("confirm:k-1", "done", ["r-2"]), (f"session:{session}", "done", ["r-1"])])
        answered = next(e for e in self.sent if "settles" in e)
        self.assertEqual((answered["text"], answered["body"]["answer"]),
                         ("Answered: Approve once", [{"question": "q0", "options": ["o0"]}]))
        reply = self.of("notice")[-1]
        self.assertEqual((reply["title"], reply["text"]), ("Command output", phrase("gateway.reset.header_default")))
        self.assertNotEqual(self.of("session")[-1]["session"], session)

    async def test_an_answer_to_a_confirmation_resolves_that_one_or_nothing(self):
        """D-146. A person who may prompt answers Hermes's request to confirm a command (D-048). Between
        their answer and Hermes taking it, what Hermes waits on changes: the agent asks for an approval, and
        another command replaces the confirmation. The answer resolves nothing: it is not typed, so it
        cannot become `/approve` for the approval, nor confirm the other command. The confirmation that
        does wait is answered by its own id."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), said("Hi.", "m-1", notify=True), complete("m-1"))
        session = self.of("session")[0]["session"]
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "new_session", "chat": CHAT, "session": session}})
        typed = self.hermes.typed[-1]
        self.feed(started(typed[2], "/new"))
        self.asks_to_confirm(typed[2], "/new", "⚠️ **Confirm /new**\n\nThis starts a fresh session.", "k-1")
        self.feed(complete(typed[2]))
        await self.connector.serve("prompt", prompt("m-2", "delete it"))
        self.feed(started("m-2"), card("rq", "rm -rf /work/x"))
        self.hermes.waiting_confirm[KEY] = "k-2"
        before = list(self.hermes.typed)
        with self.assertRaises(Refusal) as stale:
            await self.connector.serve("answer", {"request": "r-2", "item": "confirm:k-1", "by": BOB, "given": YES})
        self.assertEqual(stale.exception.reason, "not_waiting")
        self.assertEqual(self.hermes.typed, before, "nothing typed, so nothing Hermes could take as /approve")
        self.assertEqual(self.hermes.cards, [], "the approval is not answered")
        self.assertEqual(self.hermes.waiting_confirm, {KEY: "k-2"}, "the other confirmation still waits")
        # Hermes no longer holding it says nothing of why: the question still shows waiting (D-155).
        self.assertEqual([e for e in self.sent if "settles" in e], [])
        # An answer shaped as a question's, on the approval, is not an approval's answer.
        with self.assertRaises(Refusal) as shaped:
            await self.connector.serve("answer", {"request": "r-3", "item": "approval:rq", "by": BOB, "given": YES})
        self.assertEqual(shaped.exception.reason, "invalid")
        self.assertEqual(self.hermes.cards, [])
        # The confirmation that waits, offered now, replaces the first, which shows as cancelled and settles the
        # `/new` it was for, and is answered by its id while it is live.
        self.asks_to_confirm(typed[2], "/new", "⚠️ **Confirm /new**\n\nThis starts a fresh session.", "k-2")
        dropped = [(e["item"], e["state"], e["settles"]) for e in self.sent if "settles" in e]
        self.assertEqual(dropped, [("confirm:k-1", "cancelled", ["r-1"])])
        await self.connector.serve("answer", {"request": "r-4", "item": "confirm:k-2", "by": BOB,
                                              "given": {"questions": [{"question": "q0", "options": ["o2"]}]}})
        self.assertEqual(self.hermes.confirms[-1], (KEY, "k-2", "cancel", "u-2"))
        self.assertEqual(self.hermes.typed, before)
        self.assertEqual(self.hermes.cards, [])
        self.assertEqual([(e["item"], e["settles"]) for e in self.sent if "settles" in e][1:], [("confirm:k-2", ["r-4"])])

    async def test_a_model_switch_hermes_asks_to_confirm_is_settled_by_the_model_it_shows(self):
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), *model_settings(model="m"), complete("m-1"))
        session = self.of("session")[-1]["session"]
        self.hermes.read_model = lambda: self.feed(*model_settings(model="n"))
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "set", "session": session, "setting": "model", "value": "n"}})
        typed = self.hermes.typed[-1]
        self.asks_to_confirm(typed[2], "/model", "⚠️ **Confirm /model**\n\nThis model costs more.", "k-2")
        self.assertEqual([e for e in self.sent if "settles" in e], [])
        self.hermes.confirmed = switched("n")
        before = list(self.hermes.typed)
        await self.connector.serve("answer", {"request": "r-2", "item": "confirm:k-2", "by": ALICE, "given": YES})
        self.assertEqual(self.hermes.typed, before)
        await asyncio.sleep(0)
        shown = self.of("session")[-1]
        self.assertEqual((shown["text"], shown["settles"]), ("Model: n, Provider: custom", ["r-1"]))

    async def test_a_confirmation_hermes_did_not_take_still_settles_the_command_once_answered_again(self):
        """The answer to Hermes's question fails on its way to Hermes: the next answer's reply is still the
        command's outcome."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), *model_settings(model="m"), complete("m-1"))
        session = self.of("session")[-1]["session"]
        self.hermes.read_model = lambda: self.feed(*model_settings(model="n"))
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "set", "session": session, "setting": "model", "value": "n"}})
        typed = self.hermes.typed[-1]
        self.asks_to_confirm(typed[2], "/model", "⚠️ **Confirm /model**\n\nThis model costs more.", "k-2")

        def busy(choice):
            raise ConnectionError("Hermes is busy")
        self.hermes.confirmed = busy
        self.hermes.waiting_confirm[KEY] = "k-2"
        with self.assertRaises(ConnectionError):
            await self.connector.serve("answer", {"request": "r-2", "item": "confirm:k-2", "by": ALICE, "given": YES})
        self.hermes.confirmed = switched("n")
        self.hermes.waiting_confirm[KEY] = "k-2"
        await self.connector.serve("answer", {"request": "r-3", "item": "confirm:k-2", "by": ALICE, "given": YES})
        await asyncio.sleep(0)
        self.assertEqual(self.hermes.model_reads, [("c-1", "u-1")])
        shown = self.of("session")[-1]
        self.assertEqual((shown["text"], shown["settles"]), ("Model: n, Provider: custom", ["r-1"]))
        self.assertEqual([e["settles"] for e in self.sent if e["item"] == "confirm:k-2" and "settles" in e], [["r-3"]])

    async def test_an_answer_hermes_does_not_let_that_person_give_leaves_the_confirmation_waiting(self):
        """D-146. The core lets Bob prompt, and so answer Hermes's request to confirm Alice's `/new` (D-048),
        but Hermes's own list of admins does not let him run `/new` here: Hermes refuses him, its words go
        to him, and the confirmation waits for someone Hermes lets answer it."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), said("Hi.", "m-1", notify=True), complete("m-1"))
        session = self.of("session")[0]["session"]
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "new_session", "chat": CHAT, "session": session}})
        typed = self.hermes.typed[-1]
        self.feed(started(typed[2], "/new"))
        self.asks_to_confirm(typed[2], "/new", "⚠️ **Confirm /new**\n\nThis starts a fresh session.", "k-1")
        self.feed(complete(typed[2]))
        self.hermes.refuses = {"u-2": "⛔ /new is admin-only here."}
        self.hermes.confirmed = phrase("gateway.reset.header_default")
        with self.assertRaises(Refusal) as refused:
            await self.connector.serve("answer", {"request": "r-2", "item": "confirm:k-1", "by": BOB, "given": YES})
        self.assertEqual((refused.exception.reason, refused.exception.detail), ("not_supported", "⛔ /new is admin-only here."))
        self.assertEqual(self.hermes.waiting_confirm, {KEY: "k-1"}, "still waiting")
        self.assertEqual([e for e in self.sent if "settles" in e], [])
        await self.connector.serve("answer", {"request": "r-3", "item": "confirm:k-1", "by": ALICE, "given": YES})
        self.assertEqual(self.hermes.confirms[-1], (KEY, "k-1", "once", "u-1"))
        settled = [(e["item"], e["settles"]) for e in self.sent if "settles" in e]
        self.assertEqual(settled, [("confirm:k-1", ["r-3"]), (f"session:{session}", ["r-1"])])

    async def test_a_confirmation_hermes_gave_no_reply_to_leaves_its_command_to_its_effect(self):
        """Hermes gave no reply to the answer: a notice says so. Hermes says the same when it ran nothing,
        having found the confirmation too old after all, so the notice never says the command ran, and
        settles nothing: the command's own effect settles it, as any other, or, failing one, the core
        (D-087, D-155)."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), said("Hi.", "m-1", notify=True), complete("m-1"))
        session = self.of("session")[0]["session"]
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "new_session", "chat": CHAT, "session": session}})
        typed = self.hermes.typed[-1]
        self.feed(started(typed[2], "/new"))
        self.asks_to_confirm(typed[2], "/new", "⚠️ **Confirm /new**\n\nThis starts a fresh session.", "k-1")
        self.feed(complete(typed[2]))
        await self.connector.serve("answer", {"request": "r-2", "item": "confirm:k-1", "by": ALICE, "given": YES})
        settled = [(e["item"], e["type"], e["settles"]) for e in self.sent if "settles" in e]
        self.assertEqual(settled, [("confirm:k-1", "question", ["r-2"])])
        notice = self.of("notice")[-1]
        self.assertEqual((notice["session"], notice.get("turn"), notice["title"]), (session, None, "Command output"))
        self.assertEqual(notice["text"], "Hermes gave no reply to /new.")

    async def test_a_failed_call_to_answer_a_confirmation_logs_only_what_kind_of_failure(self):
        """D-071: the failure may hold a path or the command's words, so the log names only its type."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), said("Hi.", "m-1", notify=True), complete("m-1"))
        session = self.of("session")[0]["session"]
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "new_session", "chat": CHAT, "session": session}})
        typed = self.hermes.typed[-1]
        self.feed(started(typed[2], "/new"))
        self.asks_to_confirm(typed[2], "/new", "⚠️ **Confirm /new**\n\nThis starts a fresh session.", "k-1")
        self.feed(complete(typed[2]))
        self.hermes.confirmed = OSError("cannot close /home/someone/secret.txt")
        with self.assertLogs("marmot_hermes") as logged:
            await self.connector.serve("answer", {"request": "r-2", "item": "confirm:k-1", "by": ALICE, "given": YES})
        self.assertEqual(logged.output, ["WARNING:marmot_hermes:Hermes's call to answer a confirmation failed: OSError"])

    async def slow_new_session_confirmed(self):
        """Alice asks for a new session, Hermes asks her to confirm it, and she does; the reset Hermes then
        runs pauses, as a `/new` closing the old session's tools can."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), said("Hi.", "m-1", notify=True), complete("m-1"))
        session = self.of("session")[0]["session"]
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "new_session", "chat": CHAT, "session": session}})
        typed = self.hermes.typed[-1]
        self.feed(started(typed[2], "/new"))
        self.asks_to_confirm(typed[2], "/new", "⚠️ **Confirm /new**\n\nThis starts a fresh session.", "k-1")
        self.feed(complete(typed[2]))
        self.hermes.confirmed = phrase("gateway.reset.header_default")
        self.hermes.pause = asyncio.Event()
        # Well inside the core's 30 seconds: the answer is taken while the reset still runs (D-058).
        await asyncio.wait_for(self.connector.serve("answer", {"request": "r-2", "item": "confirm:k-1", "by": ALICE,
                                                               "given": YES}), 1)
        return session

    async def test_a_confirmed_command_that_runs_long_does_not_hold_the_answer(self):
        """The core is answered once Hermes has taken the confirmation: the question shows the answer at
        once, and the command's outcome, which settles the command's request, when it finishes."""
        session = await self.slow_new_session_confirmed()
        self.assertEqual([(e["item"], e["settles"]) for e in self.sent if "settles" in e], [("confirm:k-1", ["r-2"])])
        self.hermes.pause.set()
        for _ in range(3):
            await asyncio.sleep(0)
        settled = [(e["item"], e["settles"]) for e in self.sent if "settles" in e]
        self.assertEqual(settled, [("confirm:k-1", ["r-2"]), (f"session:{session}", ["r-1"])])
        reply = self.of("notice")[-1]
        self.assertEqual((reply["text"], reply.get("turn")), (phrase("gateway.reset.header_default"), None))

    async def test_a_core_that_goes_away_does_not_cancel_a_confirmed_command(self):
        """The core goes away while the command Hermes took runs: the command runs to its end, and what
        Hermes said is still shown."""
        await self.slow_new_session_confirmed()
        self.connector.core_gone()
        self.hermes.pause.set()
        for _ in range(5):
            await asyncio.sleep(0)
        self.assertEqual(self.of("notice")[-1]["text"], phrase("gateway.reset.header_default"))
        self.assertEqual(self.connector.tasks, set(), "nothing left waiting")

    async def two_confirmations_in_a_group(self):
        """Alice asks for a new session and Bob for another model, in one group, where Hermes keeps a session
        for each: Hermes asks each of them to confirm, and both wait."""
        for message, sender in (("m-a", ALICE), ("m-b", BOB)):
            await self.connector.serve("prompt", prompt(message, chat=GROUP, sender=sender))
        self.feed(started("m-a", chat="c-2", user="u-1", chat_type="group"), complete("m-a"),
                  started("m-b", chat="c-2", user="u-2", chat_type="group"), complete("m-b"), chat="c-2")
        source = {"chat_id": "c-2", "chat_type": "group", "user_id": "u-2"}
        self.hermes.read_model = lambda: self.feed(
            {"dir": "out", "via": "call", "msg": {"method": "get_session_model_settings", "params": {"source": source}}},
            {"dir": "in", "via": "call", "msg": {"method": "get_session_model_settings", "result": {
                "session_key": "agent:main:marmot:group:c-2:u-2", "model": "n", "provider": "custom"}}}, chat="c-2")
        keys = {"u-1": "agent:main:marmot:group:c-2:u-1", "u-2": "agent:main:marmot:group:c-2:u-2"}
        await self.connector.serve("control", {"request": "r-a", "by": ALICE, "action": {
            "type": "new_session", "chat": GROUP, "session": "group:c-2:u-1"}})
        typed_a = self.hermes.typed[-1][2]
        self.feed(started(typed_a, "/new", "c-2", "u-1", "group"), chat="c-2")
        await self.connector.serve("control", {"request": "r-b", "by": BOB, "action": {
            "type": "set", "session": "group:c-2:u-2", "setting": "model", "value": "n"}})
        typed_b = self.hermes.typed[-1][2]
        for typed, title, asking, confirm_id, user in (
                (typed_a, "/new", "⚠️ **Confirm /new**\n\nThis starts a fresh session.", "k-a", "u-1"),
                (typed_b, "/model", "⚠️ **Confirm /model**\n\nThis model costs more.", "k-b", "u-2")):
            self.hermes.waiting_confirm[keys[user]] = confirm_id
            self.feed(hook("send_slash_confirm", chat_id="c-2", title=title, message=asking, session_key=keys[user],
                           confirm_id=confirm_id, metadata=None), said(asking, typed, True, "c-2"), chat="c-2")
        self.feed(complete(typed_a), chat="c-2")
        self.assertEqual([e for e in self.sent if "settles" in e], [])

    async def answer_alices(self):
        self.hermes.confirmed = phrase("gateway.reset.header_default")
        await self.connector.serve("answer", {"request": "r-a2", "item": "confirm:k-a", "by": ALICE, "given": YES})
        await asyncio.sleep(0)

    async def answer_bobs(self):
        self.hermes.confirmed = switched("n")
        await self.connector.serve("answer", {"request": "r-b2", "item": "confirm:k-b", "by": BOB, "given": YES})
        await asyncio.sleep(0)

    def settled(self):
        return [(e["item"], e["session"], e["settles"]) for e in self.sent if "settles" in e]

    async def test_two_confirmations_in_a_group_each_settle_their_own_command_alices_first(self):
        """Hermes's reply to Alice's answer is the outcome of her `/new`, and says nothing of Bob's `/model`,
        which still waits for his answer, whichever of them Hermes asked last."""
        await self.two_confirmations_in_a_group()
        await self.answer_alices()
        self.assertEqual(self.settled(), [("confirm:k-a", "group:c-2:u-1", ["r-a2"]),
                                          ("session:group:c-2:u-1", "group:c-2:u-1", ["r-a"])])
        self.assertEqual(self.hermes.model_reads, [], "Bob's model is not read: his switch did not happen")
        await self.answer_bobs()
        self.assertEqual(self.settled()[2:], [("confirm:k-b", "group:c-2:u-2", ["r-b2"]),
                                              ("session:group:c-2:u-2", "group:c-2:u-2", ["r-b"])])
        self.assertEqual(self.hermes.model_reads, [("c-2", "u-2")])
        replies = [(e["session"], e["text"]) for e in self.of("notice")]
        # Hermes's words on a reset are the new session's first.
        self.assertEqual(replies, [("group:c-2:u-1#2", phrase("gateway.reset.header_default")), ("group:c-2:u-2", switched("n"))])

    async def test_two_confirmations_in_a_group_each_settle_their_own_command_bobs_first(self):
        await self.two_confirmations_in_a_group()
        await self.answer_bobs()
        self.assertEqual(self.settled(), [("confirm:k-b", "group:c-2:u-2", ["r-b2"]),
                                          ("session:group:c-2:u-2", "group:c-2:u-2", ["r-b"])])
        await self.answer_alices()
        self.assertEqual(self.settled()[2:], [("confirm:k-a", "group:c-2:u-1", ["r-a2"]),
                                              ("session:group:c-2:u-1", "group:c-2:u-1", ["r-a"])])
        replies = [(e["session"], e["text"]) for e in self.of("notice")]
        self.assertEqual(replies, [("group:c-2:u-2", switched("n")), ("group:c-2:u-1#2", phrase("gateway.reset.header_default"))])

    # -- #183: each confirmation is Hermes's, kept by its session and id (D-155)

    def group_key(self, user):
        return f"agent:main:marmot:group:c-2:{user}"

    def last_of(self, item):
        return [e for e in self.sent if e["item"] == item][-1]

    async def test_a_typed_cancel_answers_only_the_senders_own_confirmation(self):
        """Both confirmations wait, Bob's asked last. Alice types `/cancel`: Hermes reads it in her own session,
        so it cancels her `/new` alone. Her question shows the answer and settles her `/new`, which will not
        run; Bob's still waits, and his answer still settles his `/model`."""
        await self.two_confirmations_in_a_group()
        await self.connector.serve("prompt", prompt("m-c", "/cancel", GROUP, ALICE))
        del self.hermes.waiting_confirm[self.group_key("u-1")]
        self.feed(started("m-c", "/cancel", "c-2", "u-1", "group"),
                  said("🟡 /new cancelled. Conversation unchanged.", "m-c", True, "c-2"), complete("m-c"), chat="c-2")
        alices = self.last_of("confirm:k-a")
        self.assertEqual((alices["state"], alices["text"], alices["body"]["answer"]),
                         ("done", "Answered: Cancel", [{"question": "q0", "options": ["o2"]}]))
        self.assertEqual(self.settled(), [("confirm:k-a", "group:c-2:u-1", ["r-a"])])
        self.assertEqual(self.last_of("confirm:k-b")["state"], "waiting", "Bob's still waits")
        await self.answer_bobs()
        self.assertEqual(self.settled()[1:], [("confirm:k-b", "group:c-2:u-2", ["r-b2"]),
                                              ("session:group:c-2:u-2", "group:c-2:u-2", ["r-b"])])

    async def test_typed_and_structured_answers_in_one_group_each_answer_their_own(self):
        """Alice types `/approve` while Bob answers his on the card: each answers the confirmation of their
        own session, and each command is settled by its own effect."""
        await self.two_confirmations_in_a_group()
        await self.connector.serve("prompt", prompt("m-y", "/approve", GROUP, ALICE))
        await self.answer_bobs()
        del self.hermes.waiting_confirm[self.group_key("u-1")]
        await self.connector.serve("prompt", prompt("m-h", "hello", GROUP, BOB))
        # Bob's turn starts while Alice's `/new` runs: Hermes's reply to her answer is hers, in her turn, and
        # Bob's reply to his message is his.
        self.feed(started("m-y", "/approve", "c-2", "u-1", "group"), started("m-h", "hello", "c-2", "u-2", "group"),
                  said(phrase("gateway.reset.header_default"), "m-y", True, "c-2"), complete("m-y"),
                  said("Hi Bob.", "m-h", True, "c-2"), complete("m-h"), chat="c-2")
        alices = self.last_of("confirm:k-a")
        self.assertEqual((alices["state"], alices["text"]), ("done", "Answered: Approve once"))
        self.assertEqual(self.last_of("confirm:k-b")["text"], "Answered: Approve once")
        self.assertEqual(self.settled(), [("confirm:k-b", "group:c-2:u-2", ["r-b2"]),
                                          ("session:group:c-2:u-2", "group:c-2:u-2", ["r-b"]),
                                          ("session:group:c-2:u-1", "group:c-2:u-1", ["r-a"])])
        reset = self.of("notice")[-1]
        self.assertEqual((reset["session"], reset["turn"], reset["text"]),
                         ("group:c-2:u-1#2", "m-y", phrase("gateway.reset.header_default")))
        bobs = self.of("message")[-1]
        self.assertEqual((bobs["session"], bobs["turn"], bobs["text"]), ("group:c-2:u-2", "m-h", "Hi Bob."))

    async def test_an_answer_typed_in_hermess_own_words_answers_the_confirmation(self):
        """Hermes reads more than `/approve` as the answer (`approval.inputs.confirm_once`): a `yes` from the
        sender whose confirmation waits answers it, as Hermes says when the plugin looks."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), said("Hi.", "m-1", notify=True), complete("m-1"))
        session = self.of("session")[0]["session"]
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "new_session", "chat": CHAT, "session": session}})
        typed = self.hermes.typed[-1]
        self.feed(started(typed[2], "/new"))
        self.asks_to_confirm(typed[2], "/new", "⚠️ **Confirm /new**\n\nThis starts a fresh session.", "k-1")
        self.feed(complete(typed[2]))
        await self.connector.serve("prompt", prompt("m-2", "yes"))
        del self.hermes.waiting_confirm[KEY]
        self.feed(started("m-2", "yes"), said(phrase("gateway.reset.header_default"), "m-2", notify=True),
                  complete("m-2"))
        answered = self.last_of("confirm:k-1")
        self.assertEqual((answered["state"], answered["text"]), ("done", "Answered: Approve once"))
        self.assertEqual([(e["item"], e["settles"]) for e in self.sent if "settles" in e], [(f"session:{session}", ["r-1"])])
        self.assertEqual(self.of("notice")[-1]["title"], "Command output")

    async def test_a_confirmation_hermes_replaced_is_closed_and_settles_its_command(self):
        """Hermes keeps one confirmation in a session, and a newer one replaces it: the older shows as
        cancelled, which settles the `/new` it was for, since that will not run, and an answer to it is
        refused. The newer is answered as any other."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), said("Hi.", "m-1", notify=True), complete("m-1"))
        session = self.of("session")[0]["session"]
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "new_session", "chat": CHAT, "session": session}})
        typed = self.hermes.typed[-1]
        self.feed(started(typed[2], "/new"))
        self.asks_to_confirm(typed[2], "/new", "⚠️ **Confirm /new**\n\nThis starts a fresh session.", "k-1")
        self.feed(complete(typed[2]))
        await self.connector.serve("prompt", prompt("m-2", "/reload-mcp"))
        self.feed(started("m-2", "/reload-mcp"))
        self.asks_to_confirm("m-2", "/reload-mcp", "⚠️ **Confirm /reload-mcp**\n\nThis reloads the tools.", "k-2")
        self.feed(complete("m-2"))
        older = self.last_of("confirm:k-1")
        self.assertEqual((older["state"], older.get("settles")), ("cancelled", ["r-1"]))
        self.assertEqual(self.last_of("confirm:k-2")["state"], "waiting")
        with self.assertRaises(Refusal) as refused:
            await self.connector.serve("answer", {"request": "r-2", "item": "confirm:k-1", "by": ALICE, "given": YES})
        self.assertEqual(refused.exception.reason, "not_waiting")
        self.hermes.confirmed = "🟡 /reload-mcp cancelled. MCP tools unchanged."
        await self.connector.serve("answer", {"request": "r-3", "item": "confirm:k-2", "by": ALICE,
                                              "given": {"questions": [{"question": "q0", "options": ["o2"]}]}})
        self.assertEqual([(e["item"], e["settles"]) for e in self.sent if "settles" in e],
                         [("confirm:k-1", ["r-1"]), ("confirm:k-2", ["r-3"])])

    async def test_another_senders_interrupted_turn_leaves_a_confirmation_waiting(self):
        """A confirmation waits in Hermes until it is answered, replaced or dropped, whatever becomes of a
        turn: Bob's turn is interrupted, and Alice's and his own confirmations still wait and are answered."""
        await self.two_confirmations_in_a_group()
        await self.connector.serve("prompt", prompt("m-b2", "hello", GROUP, BOB))
        self.feed(started("m-b2", "hello", "c-2", "u-2", "group"), complete("m-b2", "cancelled"), chat="c-2")
        self.assertEqual([self.last_of(i)["state"] for i in ("confirm:k-a", "confirm:k-b")], ["waiting", "waiting"])
        await self.answer_alices()
        await self.answer_bobs()
        self.assertEqual(self.settled(), [("confirm:k-a", "group:c-2:u-1", ["r-a2"]),
                                          ("session:group:c-2:u-1", "group:c-2:u-1", ["r-a"]),
                                          ("confirm:k-b", "group:c-2:u-2", ["r-b2"]),
                                          ("session:group:c-2:u-2", "group:c-2:u-2", ["r-b"])])

    async def test_a_confirmation_hermes_dropped_without_a_word_waits_until_its_limit(self):
        """Hermes drops a confirmation without a word, here when the session's next message comes: the plugin
        cannot tell why one stopped waiting, so the question still shows waiting until Hermes's own limit
        passes, and then shows as cancelled, and settles the `/new` it was for (D-155)."""
        with mock.patch("marmot_hermes.connector.EXPIRY_SECONDS", 0.05, create=True):
            await self.asked_to_confirm_a_new_session()
        await self.connector.serve("prompt", prompt("m-2", "hello again"))
        del self.hermes.waiting_confirm[KEY]
        self.feed(started("m-2", "hello again"), said("Hi again.", "m-2", notify=True), complete("m-2"))
        self.assertEqual(self.last_of("confirm:k-1")["state"], "waiting")
        self.assertEqual(self.of("message")[-1]["text"], "Hi again.")
        await asyncio.sleep(0.1)
        dropped = self.last_of("confirm:k-1")
        self.assertEqual((dropped["state"], dropped.get("settles")), ("cancelled", ["r-1"]))

    async def asked_to_confirm_a_new_session(self):
        """Alice asks for a new session in a direct chat, and Hermes asks her to confirm it, as `k-1`."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), said("Hi.", "m-1", notify=True), complete("m-1"))
        session = self.of("session")[0]["session"]
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "new_session", "chat": CHAT, "session": session}})
        typed = self.hermes.typed[-1]
        self.feed(started(typed[2], "/new"))
        self.asks_to_confirm(typed[2], "/new", "⚠️ **Confirm /new**\n\nThis starts a fresh session.", "k-1")
        self.feed(complete(typed[2]))
        return session

    async def test_a_typed_answer_hermes_did_not_take_leaves_the_confirmation_waiting(self):
        """Hermes still holds the confirmation after a typed answer, here because its own check of who may
        answer refused the sender (`_refused_slash_confirm`): the question still waits, and its command is
        not settled, whatever the word."""
        await self.asked_to_confirm_a_new_session()
        for n, word in enumerate(("/approve", "yes")):
            await self.connector.serve("prompt", prompt(f"m-{n + 2}", word))
            self.feed(started(f"m-{n + 2}", word), said("⛔ /new is admin-only here.", f"m-{n + 2}", notify=True),
                      complete(f"m-{n + 2}"))
            self.assertEqual(self.last_of("confirm:k-1")["state"], "waiting", word)
        self.assertEqual([e for e in self.sent if "settles" in e], [])

    async def test_a_typed_answer_refused_while_hermes_could_not_be_asked_leaves_the_confirmation_waiting(self):
        """When the plugin cannot read which confirmation Hermes holds, it does not guess: the question still
        waits, so the card can still be answered."""
        session = await self.asked_to_confirm_a_new_session()
        self.hermes.look_fails = RuntimeError("unreadable")
        await self.connector.serve("prompt", prompt("m-2", "/approve"))
        self.feed(started("m-2", "/approve"), said("⛔ /new is admin-only here.", "m-2", notify=True), complete("m-2"))
        self.assertEqual(self.last_of("confirm:k-1")["state"], "waiting")
        self.hermes.look_fails = None
        self.hermes.confirmed = phrase("gateway.reset.header_default")
        await self.connector.serve("answer", {"request": "r-2", "item": "confirm:k-1", "by": ALICE, "given": YES})
        await asyncio.sleep(0)
        self.assertEqual(self.last_of("confirm:k-1")["text"], "Answered: Approve once")
        self.assertEqual([(e["item"], e["settles"]) for e in self.sent if "settles" in e],
                         [("confirm:k-1", ["r-2"]), (f"session:{session}", ["r-1"])])

    async def test_a_typed_answer_to_a_confirmation_hermes_found_too_old_shows_it_cancelled(self):
        """Hermes takes a typed `/approve` to a confirmation older than its limit, runs nothing, and says
        nothing (`tools.slash_confirm.resolve`, `_hm_slash_confirm_reply`). With no reply, nothing shows the
        question answered: it shows as cancelled once Hermes's limit passes, and settles the `/new`, which will
        not run (round 1: it showed as approved)."""
        with mock.patch("marmot_hermes.connector.EXPIRY_SECONDS", 0.05, create=True):
            await self.asked_to_confirm_a_new_session()
        del self.hermes.waiting_confirm[KEY]
        await self.connector.serve("prompt", prompt("m-2", "/approve"))
        self.feed(started("m-2", "/approve"), complete("m-2"))
        self.assertEqual(self.last_of("confirm:k-1")["state"], "waiting")
        await asyncio.sleep(0.1)
        dropped = self.last_of("confirm:k-1")
        self.assertEqual((dropped["state"], dropped.get("settles")), ("cancelled", ["r-1"]))
        with self.assertRaises(Refusal) as refused:
            await self.connector.serve("answer", {"request": "r-2", "item": "confirm:k-1", "by": ALICE, "given": YES})
        self.assertEqual(refused.exception.reason, "not_waiting")

    async def test_a_confirmation_no_one_answers_closes_when_hermess_limit_passes(self):
        """Hermes never runs a confirmation older than its limit (`tools.slash_confirm`), and says nothing when
        it drops one: once the limit passes, the question shows as cancelled, and settles the `/new` it was
        for, which will not run (D-155)."""
        with mock.patch("marmot_hermes.connector.EXPIRY_SECONDS", 0.05, create=True):
            await self.asked_to_confirm_a_new_session()
        self.assertEqual(self.last_of("confirm:k-1")["state"], "waiting")
        await asyncio.sleep(0.1)
        dropped = self.last_of("confirm:k-1")
        self.assertEqual((dropped["state"], dropped.get("settles")), ("cancelled", ["r-1"]))
        self.assertEqual([(e["item"], e["settles"]) for e in self.sent if "settles" in e], [("confirm:k-1", ["r-1"])])

    async def test_a_message_while_an_approved_reset_runs_leaves_the_question_to_hermess_reply(self):
        """Round 2's blocker. Hermes takes a typed `/approve` before it runs the `/new` it confirms, which can
        take a while. A message Alice sends meanwhile finds no confirmation waiting in Hermes, which says
        nothing of why: the question still shows waiting, never cancelled, until Hermes's reply to the answer
        shows it answered, and the reset settles the `/new` by its effect (D-087)."""
        session = await self.asked_to_confirm_a_new_session()
        await self.connector.serve("prompt", prompt("m-2", "/approve"))
        self.feed(started("m-2", "/approve"))
        del self.hermes.waiting_confirm[KEY]  # Hermes took it, and runs the reset
        await self.connector.serve("prompt", prompt("m-3", "and then?"))
        self.assertEqual(self.last_of("confirm:k-1")["state"], "waiting")
        self.feed(said(phrase("gateway.reset.header_default"), "m-2", notify=True), complete("m-2"))
        self.assertEqual([e["state"] for e in self.sent if e["item"] == "confirm:k-1"], ["waiting", "done"])
        self.assertEqual(self.last_of("confirm:k-1")["text"], "Answered: Approve once")
        self.assertEqual([(e["item"], e["settles"]) for e in self.sent if "settles" in e], [(f"session:{session}", ["r-1"])])

    async def test_a_confirmation_asked_while_another_turn_of_its_session_runs_waits_for_its_answer(self):
        """Hermes asks to confirm a typed `/new` while the session's turn still runs, so the `/new` has no
        turn of its own: the question Hermes sends in reply to the `/new` names it, and the answer, not
        those words, decides what becomes of it."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"))
        session = self.of("session")[0]["session"]
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "new_session", "chat": CHAT, "session": session}})
        typed = self.hermes.typed[-1]
        self.asks_to_confirm(typed[2], "/new", "⚠️ **Confirm /new**\n\nThis starts a fresh session.", "k-1")
        self.feed(said("Hi.", "m-1", notify=True), complete("m-1"))
        self.assertEqual([e for e in self.sent if "settles" in e], [])
        self.hermes.confirmed = phrase("gateway.reset.header_default")
        await self.connector.serve("answer", {"request": "r-2", "item": "confirm:k-1", "by": ALICE, "given": YES})
        await asyncio.sleep(0)
        self.assertEqual([(e["item"], e["settles"]) for e in self.sent if "settles" in e],
                         [("confirm:k-1", ["r-2"]), (f"session:{session}", ["r-1"])])

    async def test_a_second_reset_while_one_is_on_its_way_is_refused(self):
        """One reset settles one request: a second `/new` for the same session waits for the first."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), said("Hi.", "m-1", notify=True), complete("m-1"))
        session = self.of("session")[0]["session"]
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "new_session", "chat": CHAT, "session": session}})
        with self.assertRaises(Refusal) as busy:
            await self.connector.serve("control", {"request": "r-2", "by": ALICE, "action": {
                "type": "new_session", "chat": CHAT, "session": session}})
        self.assertEqual(busy.exception.reason, "busy")
        self.assertEqual([t[3] for t in self.hermes.typed].count("/new"), 1)

    async def test_a_model_reset_is_settled_by_the_model_hermes_then_shows(self):
        """`/model reset` succeeds with other words (`fixtures/recordings/hermes/model_switch.jsonl`), and
        a model Hermes names after a switch may be spelt otherwise than asked: the model read after either
        settles the request."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), *model_settings(model="n"), complete("m-1"))
        session = self.of("session")[-1]["session"]
        self.hermes.read_model = lambda: self.feed(*model_settings(model="m"))
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "set", "session": session, "setting": "model", "value": "reset"}})
        self.feed(said(phrase("gateway.model.reset_done", model="m"), self.hermes.typed[-1][2], notify=True))
        await asyncio.sleep(0)
        self.assertEqual(self.hermes.model_reads, [("c-1", "u-1")])
        shown = self.of("session")[-1]
        self.assertEqual((shown["text"], shown["settles"]), ("Model: m, Provider: custom", ["r-1"]))
        self.hermes.read_model = lambda: self.feed(*model_settings(model="vendor/n"))
        await self.connector.serve("control", {"request": "r-2", "by": ALICE, "action": {
            "type": "set", "session": session, "setting": "model", "value": "n"}})
        self.feed(said(switched("vendor/n"), self.hermes.typed[-1][2], notify=True))
        await asyncio.sleep(0)
        self.assertEqual(self.of("session")[-1]["settles"], ["r-2"])

    async def test_a_model_reset_to_the_model_shown_is_settled_by_the_read_after_it(self):
        """A reset that leaves the model as it was shows nothing new: the read after it settles the
        request all the same, without the core waiting for the effect it would not see."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), *model_settings(model="m"), complete("m-1"))
        session = self.of("session")[-1]["session"]
        self.hermes.read_model = lambda: self.feed(*model_settings(model="m"))
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "set", "session": session, "setting": "model", "value": "reset"}})
        self.feed(said(phrase("gateway.model.reset_done", model="m"), self.hermes.typed[-1][2], notify=True))
        await asyncio.sleep(0)
        shown = self.of("session")[-1]
        self.assertEqual((shown["text"], shown.get("settles")), ("Model: m, Provider: custom", ["r-1"]))

    async def test_a_model_read_before_a_switch_neither_settles_it_nor_hides_the_read_after(self):
        """A read of the model started before Hermes took a `/model` may answer after it, with the model
        before: it settles nothing and shows nothing, and the read started after the switch still does."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"))
        earlier_out, earlier_in = model_settings(model="m", read="read-1")
        later_out, later_in = model_settings(model="n", read="read-2")
        self.feed(earlier_out)
        session = self.of("session")[-1]["session"]
        self.hermes.read_model = lambda: self.feed(later_out)
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "set", "session": session, "setting": "model", "value": "n"}})
        self.feed(said(switched("n"), self.hermes.typed[-1][2], notify=True))
        await asyncio.sleep(0)
        before = self.of("session")
        self.feed(earlier_in)
        self.assertEqual([e for e in self.sent if "settles" in e], [])
        self.assertEqual(self.of("session"), before)
        self.feed(later_in)
        shown = self.of("session")[-1]
        self.assertEqual((shown["text"], shown.get("settles")), ("Model: n, Provider: custom", ["r-1"]))

    async def test_a_second_model_switch_while_one_is_waiting_for_its_read_is_refused(self):
        """One model read settles one switch: a second `/model` for a session whose switch is still
        waiting for the read that shows it is refused as busy, so the first is never settled by an
        event showing the second's model (D-028)."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), *model_settings(model="m"), complete("m-1"))
        session = self.of("session")[-1]["session"]
        read_out, read_in = model_settings(model="n", read="read-1")
        self.hermes.read_model = lambda: self.feed(read_out)
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "set", "session": session, "setting": "model", "value": "n"}})
        self.feed(said(switched("n"), self.hermes.typed[-1][2], notify=True))
        await asyncio.sleep(0)
        # The read after the switch to `n` is still under way.
        with self.assertRaises(Refusal) as busy:
            await self.connector.serve("control", {"request": "r-2", "by": ALICE, "action": {
                "type": "set", "session": session, "setting": "model", "value": "p"}})
        self.assertEqual(busy.exception.reason, "busy")
        self.assertEqual([t[3] for t in self.hermes.typed], ["hello", "/model n"])
        self.feed(read_in)
        shown = self.of("session")[-1]
        self.assertEqual((shown["text"], shown.get("settles")), ("Model: n, Provider: custom", ["r-1"]))
        # Once it is settled, the next switch is taken; and a switch left waiting longer than a control
        # waits for its effect holds up no other.
        await self.connector.serve("control", {"request": "r-3", "by": ALICE, "action": {
            "type": "set", "session": session, "setting": "model", "value": "p"}})
        with mock.patch.object(asyncio.get_running_loop(), "time",
                               return_value=asyncio.get_running_loop().time() + EFFECT_SECONDS + 1):
            await self.connector.serve("control", {"request": "r-4", "by": ALICE, "action": {
                "type": "set", "session": session, "setting": "model", "value": "q"}})
        self.assertEqual([t[3] for t in self.hermes.typed], ["hello", "/model n", "/model p", "/model q"])

    async def test_a_session_showing_the_model_before_hermes_takes_the_switch_does_not_settle_it(self):
        """A switch is settled by the read Hermes answers after taking it, never by a session event that
        happens to show the model asked for before Hermes said anything: the switch still waits, holds
        off a second one, and Hermes's refusal settles it (D-028)."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), *model_settings(model="m"), complete("m-1"))
        session = self.of("session")[-1]["session"]
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "set", "session": session, "setting": "model", "value": "n"}})
        typed = self.hermes.typed[-1]
        # A read begun before Hermes answers the command shows the model asked for.
        self.feed(*model_settings(model="n"))
        self.assertEqual(self.of("session")[-1]["text"], "Model: n, Provider: custom")
        self.assertEqual([e for e in self.sent if "settles" in e], [])
        with self.assertRaises(Refusal) as busy:
            await self.connector.serve("control", {"request": "r-2", "by": ALICE, "action": {
                "type": "set", "session": session, "setting": "model", "value": "p"}})
        self.assertEqual(busy.exception.reason, "busy")
        refusal = phrase("gateway.model.error_prefix", error="Unknown model `n`.")
        self.feed(said(refusal, typed[2], notify=True))
        refused = self.of("notice")[-1]
        self.assertEqual((refused["text"], refused["settles"]), (refusal, ["r-1"]))
        self.assertEqual(self.hermes.model_reads, [])

    async def busy_at_once(self, params):
        """Serves a control that must be refused as busy before anything is handed to Hermes."""
        task = asyncio.ensure_future(self.connector.serve("control", params))
        for _ in range(3):
            await asyncio.sleep(0)
        if not task.done():
            task.cancel()
            self.fail(f"{params['request']} was handed to Hermes, not refused")
        with self.assertRaises(Refusal) as busy:
            task.result()
        self.assertEqual(busy.exception.reason, "busy")

    async def test_a_slow_handoff_holds_off_a_second_control_until_its_effect_window_ends(self):
        """The core waits for a control's effect from when the plugin hands it over (D-087). The plugin
        holds the session from before the handoff, however long Hermes takes to take it, and for as long
        after as the core waits, so no second control is let through while the first can still settle."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), *model_settings(model="m"), complete("m-1"))
        session = self.of("session")[-1]["session"]
        loop = asyncio.get_running_loop()
        clock = [loop.time()]
        actions = [{"type": "set", "session": session, "setting": "model", "value": "n"},
                   {"type": "new_session", "chat": CHAT, "session": session}]
        for n, action in enumerate(actions):
            gate = asyncio.Event()
            inject = self.hermes.inject

            async def slow(*args, gate=gate, inject=inject):
                await gate.wait()
                await inject(*args)
            with mock.patch.object(self.hermes, "inject", slow), mock.patch.object(loop, "time", lambda: clock[0]):
                first = asyncio.ensure_future(self.connector.serve("control", {
                    "request": f"r-{n}a", "by": ALICE, "action": action}))
                await asyncio.sleep(0)
                clock[0] += EFFECT_SECONDS + 1
                await self.busy_at_once({"request": f"r-{n}b", "by": ALICE, "action": action})
                gate.set()
                await first
                clock[0] += EFFECT_SECONDS - 1
                await self.busy_at_once({"request": f"r-{n}c", "by": ALICE, "action": action})
                clock[0] += 2
                await self.connector.serve("control", {"request": f"r-{n}d", "by": ALICE, "action": action})

    async def test_every_reset_hermes_takes_is_settled_by_the_read_after_it(self):
        """`/model reset` is taken in two ways (`gateway/slash_commands_model.py`, `_handle_model_reset`):
        the session's own model cleared (`reset_done`), or none to clear (`reset_none`). Each is settled by
        the model read after it, which shows the session even when nothing changed."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), *model_settings(model="m"), complete("m-1"))
        session = self.of("session")[-1]["session"]
        self.hermes.read_model = lambda: self.feed(*model_settings(model="m"))
        for n, key in enumerate(("gateway.model.reset_none", "gateway.model.reset_done")):
            await self.connector.serve("control", {"request": f"r-{n}", "by": ALICE, "action": {
                "type": "set", "session": session, "setting": "model", "value": "reset"}})
            self.feed(said(phrase(key, model="m"), self.hermes.typed[-1][2], notify=True))
            await asyncio.sleep(0)
            self.assertEqual(len(self.hermes.model_reads), n + 1)
            shown = self.of("session")[-1]
            self.assertEqual((shown["text"], shown.get("settles")), ("Model: m, Provider: custom", [f"r-{n}"]))
            self.assertEqual([e for e in self.of("notice") if "settles" in e], [])

    async def test_a_read_with_no_model_neither_shows_nor_settles(self):
        """Hermes's getter can name the session and give a blank model (`gateway/platforms/session_model.py`,
        with no default configured). That says nothing of the session: the model shown stays, and the
        switch waits for a read that has one."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), *model_settings(model="m"), complete("m-1"))
        session = self.of("session")[-1]["session"]
        blank_out, blank_in = model_settings(model="", read="read-1")
        later_out, later_in = model_settings(model="n", read="read-2")
        reads = iter([blank_out, later_out])
        self.hermes.read_model = lambda: self.feed(next(reads))
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
            "type": "set", "session": session, "setting": "model", "value": "n"}})
        self.feed(said(switched("n"), self.hermes.typed[-1][2], notify=True))
        await asyncio.sleep(0)
        before = list(self.sent)
        self.feed(blank_in)
        self.assertEqual(self.sent, before)
        self.assertEqual(self.connector.chats["c-1"].translator.reads, {})
        await self.connector.hermes.read_model_settings(CHAT, ALICE)
        self.feed(later_in)
        shown = self.of("session")[-1]
        self.assertEqual((shown["text"], shown.get("settles")), ("Model: n, Provider: custom", ["r-1"]))

    async def test_hermes_is_read_in_the_language_it_is_set_to(self):
        """The plugin knows Hermes's reply by its key in Hermes's catalog, whatever the language; and
        when Hermes knows none of the keys, the control is refused rather than guessed at."""
        await self.connector.serve("prompt", prompt("m-1"))
        self.feed(started("m-1"), *model_settings(model="m"), complete("m-1"))
        session = self.of("session")[-1]["session"]
        other = {"gateway.model.switched": "Modell gewechselt zu `{model}`"}
        self.hermes.phrase = lambda key, **fields: other.get(key, key).format(**fields)
        self.hermes.read_model = lambda: self.feed(*model_settings(model="n"))
        set_n = {"type": "set", "session": session, "setting": "model", "value": "n"}
        await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": set_n})
        self.feed(said("Modell gewechselt zu `n`\nAnbieter: Custom", self.hermes.typed[-1][2], notify=True))
        await asyncio.sleep(0)
        self.assertEqual(self.of("session")[-1].get("settles"), ["r-1"])
        self.hermes.phrase = lambda key, **fields: key
        for action in (set_n, {"type": "new_session", "chat": CHAT, "session": session}):
            with self.assertRaises(Refusal) as refused:
                await self.connector.serve("control", {"request": "r-2", "by": ALICE, "action": action})
            self.assertEqual(refused.exception.reason, "not_supported")

    async def test_what_is_not_offered_is_refused(self):
        with self.assertRaises(Refusal) as refused:
            await self.connector.serve("control", {"request": "r-1", "by": ALICE, "action": {
                "type": "set", "session": "s", "setting": "yolo", "value": "on"}})
        self.assertEqual(refused.exception.reason, "not_supported")
        with self.assertRaises(Refusal) as invalid:
            await self.connector.serve("prompt", {"message": "m-1"})
        self.assertEqual(invalid.exception.reason, "invalid")
        # That prompt is not one the core sends, on purpose.
        self.assertEqual(len(self.invalid), 4)
        self.invalid.clear()


    async def test_the_plugin_reads_nothing_from_the_cores_requests_that_the_core_does_not_send(self):
        """Every request of the core's in `fixtures/socket/examples.json`, served with every field read
        recorded: each must be one the core sends."""
        order = ["prompt", "answer", "control"]
        requests = sorted(core_requests(), key=lambda r: order.index(r[1]))
        self.assertEqual({r[1] for r in requests}, set(order))
        for n, (name, method, params) in enumerate(requests):
            if method == "answer":
                # Hermes asks, and the example answers what it asked: the item is the plugin's own id.
                if "questions" in params["given"]:
                    self.feed(hook("send_clarify", chat_id="c-1", question="Which?", choices=["Hello", "Hi"],
                                   clarify_id=f"q{n}", session_key="agent:main:marmot:dm:c-1"))
                    params["item"] = self.of("question")[-1]["item"]
                else:
                    self.feed(card(f"rq{n}", "rm -rf /work/x"))
                    params["item"] = self.of("approval")[-1]["item"]
            reads = Reads()
            try:
                await asyncio.wait_for(self.connector.serve(method, reads.track(params)), 1)
            except (Refusal, asyncio.TimeoutError):
                pass
            definition = method[0].upper() + method[1:] + "Params"
            with self.subTest(example=name):
                self.assertTrue(reads.paths, "nothing was read")
                self.assertEqual(unknown_reads(definition, reads), [], "read, and never sent by the core")


class Socket(unittest.IsolatedAsyncioTestCase):
    """The connection, against a stand-in for the core on a real Unix socket."""

    async def test_hello_requests_and_events(self):
        with tempfile.TemporaryDirectory() as folder:
            path = str(Path(folder) / "core.sock")
            seen = asyncio.Queue()

            async def core(reader, writer):
                async def line():
                    return json.loads(await reader.readline())

                hello = await line()
                await seen.put(hello)
                writer.write(json.dumps({"jsonrpc": "2.0", "id": hello["id"], "result": {
                    "core": {"version": "0"}, "agent": {"name": "Hermes"}}}).encode() + b"\n")
                for n, method in enumerate(("prompt", "control"), start=7):
                    writer.write(json.dumps({"jsonrpc": "2.0", "id": n, "method": method, "params": {}}).encode() + b"\n")
                for _ in range(3):
                    await seen.put(await line())
                writer.close()

            server = await asyncio.start_unix_server(core, path)
            connection = await CoreConnection.open(path)

            async def serve(method, params):
                if method == "control":
                    raise Refusal("not_supported", "Not here.")

            welcome = await connection.hello({"token": "t", "capabilities": {}}, serve)
            self.assertEqual(welcome["agent"]["name"], "Hermes")
            hello = await seen.get()
            self.assertEqual((hello["method"], hello["params"]["protocol"], hello["params"]["token"]),
                             ("hello", PROTOCOL, "t"))
            self.assertTrue(connection.notify("event", {"item": "i"}))
            got = [await seen.get() for _ in range(3)]
            by_id = {m.get("id"): m for m in got}
            self.assertEqual(by_id[7]["result"], {})
            self.assertEqual(by_id[8]["error"]["data"], {"reason": "not_supported", "detail": "Not here."})
            self.assertEqual(by_id[None]["method"], "event")
            await asyncio.wait_for(connection.done(), 5)
            server.close()

    async def test_the_plugins_own_requests_are_answered_by_their_ids(self):
        """`send_file` (D-090): numbered after the hello, answered once the core has done it or with its
        refusal, and failed when the connection closes before the answer."""
        with tempfile.TemporaryDirectory() as folder:
            path = str(Path(folder) / "core.sock")
            asked = asyncio.Queue()

            async def core(reader, writer):
                async def line():
                    return json.loads(await reader.readline())

                hello = await line()
                writer.write(json.dumps({"jsonrpc": "2.0", "id": hello["id"], "result": {}}).encode() + b"\n")
                first, second, third = await line(), await line(), await line()
                for m in (first, second, third):
                    await asked.put(m)
                # Answered out of order: the second first.
                writer.write(json.dumps({"jsonrpc": "2.0", "id": second["id"], "error": {
                    "code": -32000, "message": "not_allowed", "data": {
                        "reason": "not_allowed", "detail": "The file is outside the folders."}}}).encode() + b"\n")
                writer.write(json.dumps({"jsonrpc": "2.0", "id": first["id"], "result": {}}).encode() + b"\n")
                await writer.drain()
                writer.close()

            server = await asyncio.start_unix_server(core, path)
            connection = await CoreConnection.open(path)

            async def serve(method, params):
                pass

            await connection.hello({"token": "t", "capabilities": {}}, serve)
            sent = asyncio.ensure_future(connection.request("send_file", {"chat": "c-1", "path": "/w/a.txt"}))
            refused = asyncio.ensure_future(connection.request("send_file", {"chat": "c-1", "path": "/etc/b"}))
            cut = asyncio.ensure_future(connection.request("send_file", {"chat": "c-1", "path": "/w/c.txt"}))
            got = [await asked.get() for _ in range(3)]
            self.assertEqual([m["method"] for m in got], ["send_file"] * 3)
            self.assertEqual(got[0]["params"], {"chat": "c-1", "path": "/w/a.txt"})
            ids = [m["id"] for m in got]
            self.assertEqual(len(set(ids)), 3)
            self.assertNotIn(1, ids)  # the hello's
            self.assertIsNone(await asyncio.wait_for(sent, 5))
            with self.assertRaises(Refused) as no:
                await asyncio.wait_for(refused, 5)
            self.assertEqual((no.exception.reason, no.exception.detail), ("not_allowed", "The file is outside the folders."))
            with self.assertRaises(Refused) as gone:
                await asyncio.wait_for(cut, 5)
            self.assertEqual(gone.exception.reason, "failed")
            # Once closed, a request fails at once.
            with self.assertRaises(Refused):
                await asyncio.wait_for(connection.request("send_file", {}), 5)
            server.close()


if __name__ == "__main__":
    unittest.main()
