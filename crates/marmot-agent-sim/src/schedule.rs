//! What a run does: its name, the messages people post, and the steps, each named as the
//! shared vocabulary names it (`docs/design/verification.md`), so a model's trace can later drive
//! a run. A schedule is plain data: it is written to a file before it runs, kept in a failure
//! capsule, and cut down by the shrinker. Any list of steps that names only the chats and
//! messages the schedule has is a valid schedule: a step that makes no sense where it falls, such
//! as a restart while the core runs, does nothing. One that names what the schedule lacks fails
//! the run ([`Schedule::check`]).

use crate::rng::{Rng, hash, mix};
use serde::{Deserialize, Serialize};
use std::fmt;

/// A family of generated runs: which faults its schedules hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    /// No faults: people post, the driver works, time passes.
    Calm,
    /// Crashes between any two effects, failed saves, clean stops.
    Crash,
    /// Sends MDK is busy for, whose outcome is not known, that it refuses, or that it takes and
    /// drops before they go out.
    Sends,
    /// A live stream that lags, live messages that come late, and reads of what arrived that fail
    /// for a moment.
    Live,
    /// A driver that refuses, is slow, hangs, never settles, or stops.
    Driver,
    /// All of the above, less often each.
    Mixed,
    /// Beyond MDK's contract: a live message heard before it is stored. MDK announces a message
    /// only after storing it (its `MessageReceived`, since MDK #3606), so this family is not part
    /// of the gate: it shows what the core would do. That MDK may announce messages out of
    /// storage order, or again, is within its contract, and the gate's `live` family does both.
    Disorder,
    /// A send whose outcome is not known goes out long after, while the core tries it again.
    /// Beyond what the core once assumed, when it looked for such a send in what MDK stored and
    /// found a duplicate here; now that MDK keeps each send's token, it finds none, and the
    /// family is part of the gate.
    Late,
}

impl Family {
    /// Every family.
    pub const ALL: [Family; 8] = [
        Family::Calm,
        Family::Crash,
        Family::Sends,
        Family::Live,
        Family::Driver,
        Family::Mixed,
        Family::Disorder,
        Family::Late,
    ];

    /// The families every pull request runs, which must pass.
    pub const GATE: [Family; 7] = [
        Family::Calm,
        Family::Crash,
        Family::Sends,
        Family::Live,
        Family::Driver,
        Family::Mixed,
        Family::Late,
    ];

    /// The family's name, as a run's name gives it.
    pub fn name(self) -> &'static str {
        match self {
            Family::Calm => "calm",
            Family::Crash => "crash",
            Family::Sends => "sends",
            Family::Live => "live",
            Family::Driver => "driver",
            Family::Mixed => "mixed",
            Family::Disorder => "disorder",
            Family::Late => "late",
        }
    }

    /// The family named `name`.
    pub fn named(name: &str) -> Option<Family> {
        Family::ALL.into_iter().find(|f| f.name() == name)
    }

    /// The generator's version for the family. A change to what a seed generates raises it, so
    /// an old name never stands for a new schedule. Version 2 of `live`, `driver` and `mixed`:
    /// a message heard live takes the ones stored before it off the list still to be heard, and
    /// a `Hang` goes ahead of the moves still queued. Version 3 of `live` and `mixed`: reads that
    /// fail for a moment (`ReadFails`). Version 2 of `sends` and 4 of `mixed`: sends MDK drops
    /// (`Rejected`) in place of lookups that fail, which the core no longer makes. Version 4 of
    /// `live` and 5 of `mixed`, MDK's contract since #3606: a message heard live no longer has
    /// those stored before it heard with it, the ones still to be heard are heard in any order,
    /// MDK announces a stored message again (`Reannounce`), and no read of a live message's place
    /// fails, since the core makes none.
    pub fn version(self) -> u32 {
        match self {
            Family::Mixed => 5,
            Family::Live => 4,
            Family::Driver | Family::Sends => 2,
            _ => 1,
        }
    }

    /// Whether the simulated MDK keeps its contract: a message is heard only once it is stored.
    pub fn within_contract(self) -> bool {
        self != Family::Disorder
    }

    /// How often each kind of step is generated.
    fn mix(self) -> Mix {
        let none = Mix::default();
        match self {
            Family::Calm => none,
            Family::Crash => Mix {
                crash: 6,
                save: 2,
                stop: 1,
                crash_after_send: 2,
                ..none
            },
            Family::Sends => Mix {
                busy: 5,
                ambiguous: 6,
                refused: 1,
                rejected: 3,
                crash: 2,
                crash_after_send: 2,
                ..none
            },
            Family::Live => Mix {
                lag: 4,
                late_hear: 6,
                reannounce: 3,
                crash: 1,
                read: 4,
                ..none
            },
            Family::Driver => Mix {
                refuse: 3,
                slow: 3,
                hang: 1,
                ignore: 3,
                driver_stops: 1,
                ..none
            },
            Family::Mixed => Mix {
                crash: 2,
                save: 1,
                stop: 1,
                crash_after_send: 1,
                busy: 2,
                ambiguous: 2,
                refused: 1,
                rejected: 1,
                lag: 1,
                late_hear: 2,
                reannounce: 1,
                read: 1,
                refuse: 1,
                slow: 1,
                hang: 1,
                ignore: 1,
                driver_stops: 1,
                ..none
            },
            Family::Disorder => Mix {
                disorder: 6,
                late_hear: 3,
                crash: 1,
                ..none
            },
            Family::Late => Mix {
                late: 5,
                ambiguous: 2,
                crash_after_send: 2,
                crash: 1,
                ..none
            },
        }
    }
}

impl fmt::Display for Family {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A run's name: everything that made its schedule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunName {
    /// The family.
    pub family: Family,
    /// The family generator's version.
    pub version: u32,
    /// The campaign's seed.
    pub seed: u64,
    /// The run's place in the campaign.
    pub index: u32,
}

impl RunName {
    /// The run `index` of `family`'s campaign `seed`, at the family's current version.
    pub fn new(family: Family, seed: u64, index: u32) -> Self {
        Self {
            family,
            version: family.version(),
            seed,
            index,
        }
    }

    /// The seed the run's schedule is made from.
    fn mixed(&self) -> u64 {
        let family = hash(self.family.name());
        mix(&[
            family,
            u64::from(self.version),
            self.seed,
            u64::from(self.index),
        ])
    }

    /// The name as a file name: `crash-v1-7-12`.
    pub fn file_stem(&self) -> String {
        format!(
            "{}-v{}-{}-{}",
            self.family, self.version, self.seed, self.index
        )
    }
}

impl fmt::Display for RunName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/v{}/{}/{}",
            self.family, self.version, self.seed, self.index
        )
    }
}

/// What a person posts to a chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "what", rename_all = "snake_case")]
pub enum What {
    /// A prompt; with `approval`, the driver asks for an approval in the turn it starts.
    Prompt {
        /// Whether the turn asks for an approval.
        approval: bool,
    },
    /// An answer to the chat's latest approval, as the person's app has it when posting.
    Answer,
    /// Stop the running turn.
    Interrupt,
    /// End the chat's session and start another.
    NewSession,
    /// Change a setting of the chat's session.
    Set,
}

/// A message a person posts: to which chat, and what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posted {
    /// The chat, from 0.
    pub chat: u8,
    /// What it is.
    pub what: What,
}

impl Posted {
    /// Whether it is an answer or a control (kind 1202), not a chat message.
    pub fn is_request(&self) -> bool {
        !matches!(self.what, What::Prompt { .. })
    }
}

/// What a send meets in the simulated MDK.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// MDK is busy and does not take it.
    Busy,
    /// The outcome is not known, and it did not go out.
    Ambiguous,
    /// The outcome is not known, and it went out.
    AmbiguousLanded,
    /// MDK refuses it, and will again.
    Refused,
    /// It goes out, and the core crashes before it learns so.
    CrashAfter,
    /// MDK takes it and drops it before it goes out: nothing of it went, and MDK refuses its
    /// token from then on.
    Rejected,
    /// The outcome is not known, and it goes out `ms` later, while the core tries it again
    /// ([`Family::Late`]).
    Late {
        /// How much later.
        ms: u64,
    },
}

/// Which read of what arrived fails, as a passing failure of MDK.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadStep {
    /// Where a message's group is, as when its state cannot be read for a moment.
    Place,
    /// What was stored after the read mark, in a catch-up.
    Stored,
    /// Which groups the agent is in, as a catch-up begins.
    Groups,
}

/// One step of a run. The names are the shared vocabulary's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum Action {
    /// The chat's owner invites the agent to it.
    Invite {
        /// The chat.
        chat: u8,
    },
    /// MDK stores a message (`Store(msg)`).
    Store {
        /// The message, by its place in [`Schedule::messages`].
        msg: usize,
    },
    /// A chat message arrives live (`Hear(msg)`).
    Hear {
        /// The message.
        msg: usize,
    },
    /// An answer or a control arrives live (`Ask(req)`).
    Ask {
        /// The message.
        msg: usize,
    },
    /// MDK announces a message it stored again, as its recovery may (`Reannounce(msg)`, MDK
    /// #3606); nothing when it is not stored.
    Reannounce {
        /// The message.
        msg: usize,
    },
    /// The live stream lags: what was not yet delivered is lost, and the core is told.
    Lag,
    /// The driver emits the next event of one of its turns or effects (`Emit(item, settles)`).
    Emit {
        /// Which, and in which variant.
        pick: u32,
        /// Whether an approval no one answered is given up on.
        force: bool,
    },
    /// The next request the driver gets is refused (`Refuse(req)`).
    Refuse,
    /// The next request the driver gets is taken after `ms` (`Take(req)`, late).
    Slow {
        /// How long it takes.
        ms: u64,
    },
    /// The next request the driver gets is never answered.
    Hang,
    /// The next request the driver gets is taken and its effect never shown.
    Ignore,
    /// The driver ends (`DriverStops`).
    DriverStops,
    /// The next send meets `outcome` (`Send(send)`).
    Send {
        /// What it meets.
        outcome: Outcome,
    },
    /// The next read of this kind fails, for a moment (`ReadFails`).
    ReadFails {
        /// Which read.
        at: ReadStep,
    },
    /// The next save fails (`SaveFails`); the core stops.
    SaveFails,
    /// The process dies (`Crash`): at once when `after` is 0, else just before the `after`-th
    /// effect from here (a save, a send, a request to the driver, an invitation accepted).
    Crash {
        /// Which effect it comes before.
        after: u32,
    },
    /// The clock advances (`Tick`).
    Tick {
        /// By how much.
        ms: u64,
    },
    /// The core starts again from its saved state, with a new driver (`Restart`).
    Restart,
    /// A clean stop (`Stop`).
    Stop,
}

/// Bugs to put back, for the tests that show the simulator finds each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BugSet {
    /// A send is saved only after it went.
    #[serde(default)]
    pub save_after_send: bool,
    /// A send whose outcome is not known is tried again under a new token.
    #[serde(default)]
    pub retry_ambiguous_with_new_token: bool,
    /// What the agent's words settle is ignored.
    #[serde(default)]
    pub settles_only_from_items: bool,
    /// A live message moves the read mark to its place, as before MDK #3606.
    #[serde(default)]
    pub mark_from_live: bool,
}

impl BugSet {
    /// Whether any is put back.
    pub fn any(&self) -> bool {
        self.save_after_send
            || self.retry_ambiguous_with_new_token
            || self.settles_only_from_items
            || self.mark_from_live
    }
}

/// Faults beyond what a run assumes, made by hand and never generated: they show that the
/// runner's own checks fail a run, as the bug switches show the properties do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Faults {
    /// The core's task panics as it ends after a clean stop, once everything is closed and sent.
    #[serde(default)]
    pub panic_when_stopped: bool,
    /// The harness emits nothing more once the fair part begins, so its work never ends.
    #[serde(default)]
    pub harness_stalls: bool,
}

impl Faults {
    /// Whether none is made.
    pub fn none(&self) -> bool {
        *self == Faults::default()
    }
}

/// A whole run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    /// What made it.
    pub name: RunName,
    /// How many chats the agent is in: 1 or 2.
    pub chats: u8,
    /// Bugs put back.
    #[serde(default)]
    pub bugs: BugSet,
    /// Faults made by hand.
    #[serde(default, skip_serializing_if = "Faults::none")]
    pub faults: Faults,
    /// The messages people post, by number.
    pub messages: Vec<Posted>,
    /// The steps, in order.
    pub steps: Vec<Action>,
}

impl Schedule {
    /// Whether every chat and message the schedule names is one it has, and each step that hears
    /// a message names it as the vocabulary does (`Hear` for a prompt, `Ask` for an answer or a
    /// control). A run of a schedule that fails this fails at once, naming the step, so one written
    /// by hand with a wrong number cannot pass without doing what it was written to check. A step
    /// that makes no sense where it falls is still valid: this checks only what it names.
    pub fn check(&self) -> Result<(), BadReference> {
        let chats = usize::from(self.chats);
        for (msg, posted) in self.messages.iter().enumerate() {
            if usize::from(posted.chat) >= chats {
                return Err(BadReference {
                    step: None,
                    detail: format!(
                        "m{msg} is posted to c{}, and the schedule has {chats} chats",
                        posted.chat
                    ),
                });
            }
        }
        for (step, action) in self.steps.iter().enumerate() {
            let bad = |detail: String| {
                Err(BadReference {
                    step: Some(step),
                    detail: format!("step {step}, {action:?}: {detail}"),
                })
            };
            match *action {
                Action::Invite { chat } if usize::from(chat) >= chats => {
                    return bad(format!("the schedule has {chats} chats, so no c{chat}"));
                }
                Action::Store { msg }
                | Action::Hear { msg }
                | Action::Ask { msg }
                | Action::Reannounce { msg }
                    if msg >= self.messages.len() =>
                {
                    return bad(format!(
                        "the schedule has {} messages, so no m{msg}",
                        self.messages.len()
                    ));
                }
                Action::Hear { msg } if self.messages[msg].is_request() => {
                    return bad(format!("m{msg} is an answer or a control: Ask hears it"));
                }
                Action::Ask { msg } if !self.messages[msg].is_request() => {
                    return bad(format!("m{msg} is a prompt: Hear hears it"));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// A chat or message a schedule names and does not have ([`Schedule::check`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadReference {
    /// The step that names it; `None` for a message posted to a chat the schedule lacks.
    pub step: Option<usize>,
    /// What is wrong, naming the step, the chat or the message.
    pub detail: String,
}

impl fmt::Display for BadReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

/// How often each kind of fault step is generated, against the weights of ordinary steps.
#[derive(Clone, Copy, Default)]
struct Mix {
    crash: u32,
    save: u32,
    stop: u32,
    crash_after_send: u32,
    busy: u32,
    ambiguous: u32,
    refused: u32,
    rejected: u32,
    read: u32,
    lag: u32,
    late_hear: u32,
    reannounce: u32,
    refuse: u32,
    slow: u32,
    hang: u32,
    ignore: u32,
    driver_stops: u32,
    disorder: u32,
    late: u32,
}

/// Makes the schedule a run's name stands for.
pub fn generate(name: &RunName) -> Schedule {
    let mut rng = Rng::new(name.mixed());
    let mix = name.family.mix();
    let chats: u8 = if rng.one_in(3) { 2 } else { 1 };
    let mut messages: Vec<Posted> = Vec::new();
    let mut steps: Vec<Action> = (0..chats).map(|chat| Action::Invite { chat }).collect();
    // For each chat, the messages stored and not yet heard, oldest first.
    let mut unheard: Vec<Vec<usize>> = vec![Vec::new(); usize::from(chats)];
    // Steps until a restart, after a fault that may stop the core.
    let mut restart_in: Option<u32> = None;
    let length = 30 + rng.below(90) as usize;
    let hear = |msg: usize, messages: &[Posted]| {
        if messages[msg].is_request() {
            Action::Ask { msg }
        } else {
            Action::Hear { msg }
        }
    };
    while steps.len() < length {
        if let Some(n) = restart_in {
            if n == 0 {
                steps.push(Action::Restart);
                restart_in = None;
                continue;
            }
            restart_in = Some(n - 1);
        }
        let weights = [
            30, // post
            if unheard.iter().any(|u| !u.is_empty()) {
                10
            } else {
                0
            },
            30, // emit
            10, // tick
            mix.crash,
            mix.save,
            mix.stop,
            mix.crash_after_send,
            mix.busy,
            mix.ambiguous,
            mix.refused,
            mix.rejected,
            mix.lag,
            mix.refuse,
            mix.slow,
            mix.hang,
            mix.ignore,
            mix.driver_stops,
            mix.late,
            mix.read,
            if messages.is_empty() {
                0
            } else {
                mix.reannounce
            },
            2, // a restart, which does nothing while the core runs
        ];
        let mut down = |rng: &mut Rng| restart_in = Some(rng.below(4) as u32);
        match rng.weighted(&weights) {
            0 => {
                let chat = rng.below(u64::from(chats)) as u8;
                let what = match rng.weighted(&[55, 20, 8, 5, 12]) {
                    0 => What::Prompt {
                        approval: rng.one_in(3),
                    },
                    1 => What::Answer,
                    2 => What::Interrupt,
                    3 => What::NewSession,
                    _ => What::Set,
                };
                let msg = messages.len();
                messages.push(Posted { chat, what });
                if mix.disorder > 0 && rng.below(10) < u64::from(mix.disorder) {
                    // Heard before it is stored, which MDK does not do.
                    steps.push(hear(msg, &messages));
                    unheard[usize::from(chat)].push(msg);
                    continue;
                }
                steps.push(Action::Store { msg });
                if mix.late_hear > 0 && rng.below(10) < u64::from(mix.late_hear) {
                    unheard[usize::from(chat)].push(msg);
                } else {
                    steps.push(hear(msg, &messages));
                }
            }
            1 => {
                let chats_waiting: Vec<usize> = (0..unheard.len())
                    .filter(|c| !unheard[*c].is_empty())
                    .collect();
                let chat = rng.pick(&chats_waiting);
                let msg = if mix.disorder > 0 {
                    if rng.one_in(2) {
                        unheard[chat].pop().expect("not empty")
                    } else {
                        unheard[chat].remove(0)
                    }
                } else {
                    // Any of them: MDK need not announce them in storage order.
                    let at = rng.below(unheard[chat].len() as u64) as usize;
                    unheard[chat].remove(at)
                };
                if mix.disorder > 0 && !steps.contains(&Action::Store { msg }) {
                    steps.push(Action::Store { msg });
                } else {
                    steps.push(hear(msg, &messages));
                }
            }
            2 => steps.push(Action::Emit {
                pick: rng.next_u64() as u32,
                force: rng.one_in(5),
            }),
            3 => steps.push(Action::Tick {
                ms: rng.pick(&[100, 1_000, 5_000, 15_000, 30_000, 130_000]),
            }),
            4 => {
                steps.push(Action::Crash {
                    after: rng.below(9) as u32,
                });
                down(&mut rng);
            }
            5 => {
                steps.push(Action::SaveFails);
                down(&mut rng);
            }
            6 => {
                steps.push(Action::Stop);
                down(&mut rng);
            }
            7 => {
                steps.push(Action::Send {
                    outcome: Outcome::CrashAfter,
                });
                down(&mut rng);
            }
            8 => steps.push(Action::Send {
                outcome: Outcome::Busy,
            }),
            9 => steps.push(Action::Send {
                outcome: if rng.one_in(2) {
                    Outcome::Ambiguous
                } else {
                    Outcome::AmbiguousLanded
                },
            }),
            10 => steps.push(Action::Send {
                outcome: Outcome::Refused,
            }),
            11 => steps.push(Action::Send {
                outcome: Outcome::Rejected,
            }),
            12 => steps.push(Action::Lag),
            13 => steps.push(Action::Refuse),
            14 => steps.push(Action::Slow {
                ms: rng.pick(&[200, 2_000, 8_000]),
            }),
            15 => {
                // A harness that hangs holds the core until it is killed: a prompt reaches it
                // (the `Hang` goes ahead of any move still queued).
                steps.push(Action::Hang);
                let chat = rng.below(u64::from(chats)) as u8;
                let msg = messages.len();
                messages.push(Posted {
                    chat,
                    what: What::Prompt { approval: false },
                });
                steps.push(Action::Store { msg });
                steps.push(Action::Hear { msg });
                for _ in 0..rng.below(3) {
                    steps.push(Action::Emit {
                        pick: rng.next_u64() as u32,
                        force: false,
                    });
                }
                steps.push(Action::Crash { after: 0 });
                restart_in = Some(0);
            }
            16 => steps.push(Action::Ignore),
            17 => {
                steps.push(Action::DriverStops);
                down(&mut rng);
            }
            18 => steps.push(Action::Send {
                outcome: Outcome::Late {
                    ms: rng.pick(&[20_000, 40_000, 90_000]),
                },
            }),
            19 => steps.push(Action::ReadFails {
                at: match rng.weighted(&[2, 1, 1]) {
                    0 => ReadStep::Place,
                    1 => ReadStep::Stored,
                    _ => ReadStep::Groups,
                },
            }),
            20 => steps.push(Action::Reannounce {
                msg: rng.below(messages.len() as u64) as usize,
            }),
            _ => steps.push(Action::Restart),
        }
    }
    // What is left unheard is heard at the end, in the fair part of the run.
    Schedule {
        name: name.clone(),
        chats,
        bugs: BugSet::default(),
        faults: Faults::default(),
        messages,
        steps,
    }
}
