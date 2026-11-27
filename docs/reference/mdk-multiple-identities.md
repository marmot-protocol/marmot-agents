# Reference: several identities in one program, as MDK stands

This page is detailed on purpose. It records what MDK supports today for running more than one Marmot account in
one process, so that the choice between "one program for all of an owner's agents" and "one program per agent"
([Q-08](../open-questions.md#q-08-how-do-several-agent-identities-run-together)) can be made on facts.

- **Read at:** MDK commit `a2d3486ac`, on 2026-11-17. That revision, the merge of MDK pull request 3484, is no
  longer on MDK's history, which was rewritten on 2026-11-24 (noted on 2026-11-27).
- **How:** a research agent read the code and reported with file and line references. The points marked
  "checked" were then confirmed directly. The rest were not re-checked. Points marked "inferred" are conclusions,
  not statements found in the code.
- **Paths** are relative to the MDK repository root. Line numbers will drift.

## The short answer

One process can already hold and run several accounts. MDK's app runtime calls itself "the multi-account app
runtime" (checked: `crates/marmot-app/README.md`), runs one worker per account (checked:
`crates/marmot-app/src/runtime/mod.rs`), and today's `wn-agent` daemon takes an account id on every request.

Each account's keys and storage are kept apart **as files and as data**. They are not kept apart **as a security
boundary**: every running account's key is in the same process memory, and nothing separates accounts by
operating-system user, sandbox or process.

## What each account has to itself

| Thing | Detail |
| --- | --- |
| A folder | `accounts/<label>/` under the home directory |
| A session database | `accounts/<label>/session.sqlite`, encrypted with SQLCipher. Checked |
| A directory cache | `accounts/<label>/app-cache.sqlite3`, encrypted. Checked |
| A database key | Derived from that account's own signing secret, its label and public key, and a salt stored per database (`crates/storage-sqlite`, `sqlcipher.rs`) |
| A signing key | Stored by the account's secret store; see below |
| A media folder | Under the account's folder |
| A worker task | One per account, restarted on its own if it dies |
| Relay connections | Each account gets its own clients, signed in with its own key |
| An invite allow-list and policy | One file per account id in `wn-agent` |

## What all accounts in a home share

| Thing | Detail |
| --- | --- |
| The process, and its memory | Every running account's signing key is loaded and held while its worker runs (inferred from the worker holding the signer) |
| One lock for the whole home | `.marmot-runtime.lock`, an exclusive lock taken by the runtime. Checked. A second process that opens the same home is refused |
| `shared.sqlite3` | One file for the installation, readable only by the owner and **not encrypted**. It holds a mirror of public directory data, telemetry and audit-log preferences, and a telemetry installation id. Checked |
| The secret store | One backend for the home |
| The proxy setting | One SOCKS5 setting for the process |
| Parts of the relay plane | The router task, and an anonymous client used for public directory lookups, which carries no account's credentials |

## Where a signing key is kept

MDK has two secret stores (`crates/marmot-account/src/secret_store.rs`):

- **A file store.** The key is written to a file in the account's own folder, as a hex string in JSON (checked:
  the stored record has a `secret_key_hex` field). `wn-agent` uses this one (checked:
  `crates/agent-connector/src/lib.rs` opens the home with the default store). So an agent's key sits in the same
  folder as the databases whose keys are derived from it.
- **A keychain store.** The key goes to the operating system's keychain, under one service name with an entry
  per account.

## The lock decides the supported shapes

Because the lock is on the home, not on an account:

| Shape | Supported today? |
| --- | --- |
| One program, one home, several accounts | Yes. This is what the runtime is built for |
| Several programs, each with its own home | Yes. This is today's default for connectors |
| Several programs sharing one home | No. The second is refused |
| One program holding several homes | Not documented. Each home would need its own runtime, and some process-wide settings would still be shared. It would need testing (inferred) |

## When one account fails

- Each account's worker is its own task. A panic ends only that task (inferred from the build settings), and a
  dead worker is started again the next time it is needed, backing off from one second to a minute.
- A worker that hangs on shutdown is stopped after a timeout.
- **A caveat.** Starting the runtime reports an error if any one account fails to start, although the other
  accounts keep running. A program that hosts several agents has to handle a partial start.
- Shared things are shared points of failure: `shared.sqlite3`, the relay plane and the runtime as a whole.

## What `wn-agent` does today

- One daemon can serve several accounts. Every request names the account.
- An inbound subscription with no account named receives events for every local account.
- The rule that an integration "fails closed when several signing accounts are available" lives in the client's
  start-up, not in the daemon.
- By default each connector is installed with its own home and its own daemon, so in practice one daemon serves
  one account.

## What "kept apart" does and does not give you

| Risk | One program, several accounts | One program per account |
| --- | --- | --- |
| A bug or break-in in the program | Exposes every agent's key | Exposes one agent's key |
| One agent's storage read by another | Not by design; separate encrypted files | Not by design, and separate homes |
| Agents linked by their network traffic | Same address, same proxy, same timing (inferred) | Same machine and address unless routed separately |
| One agent's failure affecting the others | Shared runtime and shared file | None |
| Running and setting up | One service, one place to add an agent | One service per agent |

Neither shape hides from a relay that several agents run on one machine, unless each is routed differently.

## To decide, or to change

1. **Which shape.** One home with several accounts works today. One program per account also works today and
   isolates more.
2. **Where keys live.** The file store keeps a plain key beside the data. The keychain store, or a store of our
   own, is the alternative. This matters more when one program holds every agent's key.
3. **The unencrypted shared file.** Whether an installation-wide file with an installation id is acceptable when
   several identities share a home.
4. **Partial start.** What the program does when one agent's account will not open.
5. **Always name the account.** With several accounts, nothing may fall back to "the only account".
6. **Network separation between agents**, if it is wanted. Today's proxy setting is for the whole process.
7. **One program with several homes**, if we want one service and still separate homes. It is untested.
