/**
 * The OpenClaw plugin for marmot-agent: a channel named `marmot` that runs inside OpenClaw and connects
 * it to the core over the local socket (D-005, `docs/design/drivers.md`).
 *
 * This is the one file that touches OpenClaw's plugin SDK. It is modelled on the recorder in
 * `tools/record/openclaw/plugin/index.js`, whose calls the recordings hold. The rest is in
 * `connector.ts`, which OpenClaw reaches through a {@link Port}.
 *
 * The agent's home is `MARMOT_AGENT_HOME`, or the channel's `home` setting: the socket is
 * `socket/marmot-agent.sock` there and the token `connector.token` (`Endpoint::in_home` in
 * `crates/marmot-agent/src/socket.rs`). The plugin keeps how many sessions each OpenClaw session key has
 * named in `marmot-sessions.json`, in OpenClaw's state folder. It logs errors by kind only: never a
 * message, a prompt, a key or a path (D-071).
 *
 * A file OpenClaw delivers to a Marmot chat, with a reply or through the channel's outbound media send, is
 * handed to the core to send (`connector.ts`, D-090). The channel says it takes media, so OpenClaw hands
 * it over rather than leaving it out.
 */

import { readFileSync, writeFileSync, renameSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { randomBytes } from "node:crypto";

import { createChatChannelPlugin, defineChannelPluginEntry } from "openclaw/plugin-sdk/channel-core";
import { defineChannelMessageAdapter } from "openclaw/plugin-sdk/channel-outbound";
import { hasControlCommand } from "openclaw/plugin-sdk/command-detection";
import { resolveApprovalOverGateway } from "openclaw/plugin-sdk/approval-gateway-runtime";
import { questionGatewayRuntime } from "openclaw/plugin-sdk/question-gateway-runtime";
import { getSessionEntryAsync } from "openclaw/plugin-sdk/session-store-runtime";
import { listChatCommands, resolveCommandArgChoices } from "openclaw/plugin-sdk/native-command-registry";
import { buildModelsProviderData } from "openclaw/plugin-sdk/models-provider-runtime";
import { resolveAgentWorkspaceDir, resolveDefaultAgentId } from "openclaw/plugin-sdk/agent-runtime";

import { CAPABILITIES, Connector, type CoreEvent, type Port } from "./connector.ts";
import { fitEvent } from "./fit.ts";
import { redactor, secretsFrom } from "./redact.ts";
import { CoreConnection, MAX_MESSAGE, notificationSize, type Hello } from "./socket.ts";

const ID = "marmot";
const PLUGIN_VERSION = "0.0.0";

type Json = Record<string, unknown>;

function say(kind: string): void {
  console.error(`marmot-agent-openclaw: ${kind}`);
}

function object(value: unknown): Json {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Json) : {};
}

function section(cfg: unknown): Json {
  return object(object(object(cfg)["channels"])[ID]);
}

/** OpenClaw's release, from the plugin API once OpenClaw has registered the plugin. */
let openclawVersion = "unknown";

/** The one connector, while connected: OpenClaw's adapter calls go to it. */
let current: Connector | null = null;

// -- the session counts, kept across restarts

function generationsFile(): string {
  return join(process.env["OPENCLAW_HOME"] ?? homedir(), ".openclaw", "marmot-sessions.json");
}

function loadGenerations(): [string, number][] {
  try {
    const read = object(JSON.parse(readFileSync(generationsFile(), "utf8")));
    return Object.entries(read).filter((e): e is [string, number] => typeof e[1] === "number");
  } catch {
    return [];
  }
}

function saveGenerations(generations: [string, number][]): void {
  try {
    const file = generationsFile();
    writeFileSync(`${file}.tmp`, JSON.stringify(Object.fromEntries(generations)) + "\n", { mode: 0o600 });
    renameSync(`${file}.tmp`, file);
  } catch {
    say("the session counts could not be saved");
  }
}

// -- the connection to the core

interface Gateway {
  cfg: Json;
  accountId: string;
  rt: Json;
  abortSignal: AbortSignal;
}

function port(gw: Gateway): Port {
  const { cfg, accountId, rt } = gw;
  /** A function of the channel runtime, by its path, such as `inbound.run`. */
  const fn = (path: string) => {
    let owner: Json = rt;
    let at: unknown = rt;
    for (const key of path.split(".")) {
      owner = object(at);
      at = owner[key];
    }
    if (typeof at !== "function") throw new Error(`OpenClaw's channel runtime has no ${path}`);
    // Called on what holds it, as `rt.inbound.run(…)` would be.
    return (at as (...args: unknown[]) => unknown).bind(owner);
  };
  return {
    route: async (peer) => object(await fn("routing.resolveAgentRoute")({ cfg, channel: ID, accountId, peer })),
    ingress: async (params) =>
      object(await fn("inbound.ingress.resolveStable")({ ...params, cfg, accessGroups: cfg["accessGroups"] })),
    context: async (params, ingress) =>
      fn("inbound.buildContext")({ ...params, channelIngress: ingress, resolveSupplementalMedia: true, suppressSelfQuoteBody: false }),
    run: async (t) => {
      const route = t.route;
      const storePath = fn("session.resolveStorePath")(object(cfg["session"])["store"], { agentId: route["agentId"] });
      return fn("inbound.run")({
        channel: ID,
        accountId,
        raw: { messageId: t.messageId },
        log: t.log,
        adapter: {
          ingest: () => ({ id: t.messageId, rawText: t.text, textForAgent: t.text }),
          resolveTurn: () => ({
            channel: ID,
            accountId,
            routeSessionKey: route["sessionKey"],
            storePath,
            ctxPayload: t.ctx,
            recordInboundSession: fn("session.recordInboundSession"),
            record: { groupResolution: { key: `${ID}:group:${t.chat}`, channel: ID, id: t.chat, chatType: "group" } },
            runDispatchLifecycle: { turnAdoptionLifecycle: undefined, onDispatchSkipped: async () => undefined },
            runDispatch: () =>
              fn("reply.dispatchReplyWithBufferedBlockDispatcher")({
                ctx: t.ctx,
                cfg,
                dispatcherOptions: t.dispatcherOptions,
                replyOptions: t.replyOptions,
              }),
          }),
        },
      });
    },
    resolveApproval: (params) => resolveApprovalOverGateway({ ...params, cfg }),
    resolveQuestion: (params) => questionGatewayRuntime.resolveOption({ ...params, cfg }),
    sessionEntry: (params) => getSessionEntryAsync(params),
    hasControlCommand: (text) => hasControlCommand(text, cfg),
    // As the recorder's `sdk` step asks: a command whose choices depend on the model gets them for the
    // configured one.
    commands: async () => {
      const [provider, ...rest] = String(object(object(object(cfg["agents"])["defaults"])["model"])["primary"] ?? "").split("/");
      const model = rest.join("/");
      return listChatCommands().map((command) => {
        const args = command["args"];
        if (!Array.isArray(args)) return command;
        return {
          ...command,
          args: args.map((arg: Json) =>
            typeof arg["choices"] === "function"
              ? { ...arg, choices: "<function>", resolvedChoices: resolveCommandArgChoices({ cfg, provider, model, command, arg }) }
              : arg,
          ),
        };
      });
    },
    models: (agentId) => buildModelsProviderData(cfg, agentId),
  };
}

/** Connects to the core, serves it until the socket closes, and gives back whether to try again. */
async function connectOnce(gw: Gateway, home: string, generations: [string, number][]): Promise<void> {
  const token = readFileSync(join(home, "connector.token"), "utf8").trim();
  const core = await CoreConnection.open(join(home, "socket", "marmot-agent.sock"));
  // The whole model key never leaves; its masked form may (D-088).
  const scrub = redactor(secretsFrom(process.env));
  const connector = new Connector({
    port: port(gw),
    channel: ID,
    accountId: gw.accountId,
    version: openclawVersion,
    boot: randomBytes(4).toString("hex"),
    generations,
    saveGenerations,
    send: (event: CoreEvent) => {
      const out: CoreEvent = scrub === null ? event : scrub(event);
      // Too large for a line of the socket: it goes without its native message, and cut if need be (D-033).
      const fitted = fitEvent(out, (e) => notificationSize("event", e), MAX_MESSAGE);
      if (fitted?.did) say(`an event over the size limit was ${fitted.did} (${event.type})`);
      if (fitted === null || !core.notify("event", fitted.event)) say(`an event could not be sent (${event.type})`);
    },
    warn: say,
    sendFile: (chat, path) => core.request("send_file", { chat, path }),
    workspace: (agentId) => {
      try {
        return resolveAgentWorkspaceDir(gw.cfg, agentId ?? resolveDefaultAgentId(gw.cfg));
      } catch {
        return undefined;
      }
    },
  });
  const hello: Hello = {
    connector: { name: "marmot-agent-openclaw", version: PLUGIN_VERSION },
    harness: { name: "openclaw", version: openclawVersion },
    token,
    capabilities: CAPABILITIES,
  };
  const stop = () => core.close();
  gw.abortSignal.addEventListener("abort", stop, { once: true });
  try {
    await core.hello(hello, (method, params) => connector.serve(method, params));
    current = connector;
    say("connected to the core");
    await core.done;
  } finally {
    gw.abortSignal.removeEventListener("abort", stop);
    if (current === connector) current = null;
    // The core ended this connection's turns. Its runs end too, before another connection takes requests:
    // until then the plugin does not connect again, so the core sends it nothing.
    await connector.stopped(10_000, !gw.abortSignal.aborted);
  }
}

async function startAccount(ctx: Json): Promise<void> {
  const gw: Gateway = {
    cfg: object(ctx["cfg"]),
    accountId: typeof ctx["accountId"] === "string" ? ctx["accountId"] : "default",
    rt: object(ctx["channelRuntime"]),
    abortSignal: ctx["abortSignal"] as AbortSignal,
  };
  const setStatus = ctx["setStatus"] as ((s: Json) => void) | undefined;
  const getStatus = ctx["getStatus"] as (() => Json) | undefined;
  setStatus?.({ ...(getStatus?.() ?? {}), accountId: gw.accountId, running: true, lastStartAt: Date.now() });
  const home = process.env["MARMOT_AGENT_HOME"] ?? (section(gw.cfg)["home"] as string | undefined);
  if (!home) throw new Error("marmot: set MARMOT_AGENT_HOME, or channels.marmot.home, to the agent's home");
  // OpenClaw keeps a channel account alive for as long as this promise is pending. The plugin connects,
  // and connects again whenever the core goes, until OpenClaw stops the account.
  while (!gw.abortSignal.aborted) {
    try {
      await connectOnce(gw, home, loadGenerations());
      say("the core closed the socket");
    } catch {
      say("the core could not be reached");
    }
    if (gw.abortSignal.aborted) break;
    await new Promise((resolve) => {
      const timer = setTimeout(resolve, 2000);
      gw.abortSignal.addEventListener("abort", () => (clearTimeout(timer), resolve(undefined)), { once: true });
    });
  }
}

// -- the channel

/** A call on one of the plugin's adapters: written down for the chat it names, and answered. */
function adapter<T>(method: string, answer: (params: unknown) => T): (params: unknown) => T {
  return (params) => {
    const out = answer(params);
    current?.adapterCall(method, params, out instanceof Promise ? "<promise>" : out);
    return out;
  };
}

let outCount = 0;
function receipt(kind: string): Json {
  outCount += 1;
  const id = `marmot-out-${outCount}`;
  return {
    receipt: { primaryPlatformMessageId: id, platformMessageIds: [id], parts: [{ platformMessageId: id, kind, index: 0 }], sentAt: Date.now() },
  };
}

function buildPlugin(): unknown {
  // Nothing is ever posted by these: what OpenClaw sends reaches the chat as the events it becomes, and a
  // file through the core.
  const message = defineChannelMessageAdapter({
    id: ID,
    durableFinal: { capabilities: { text: true, media: true, payload: true, replyTo: true } },
    send: {
      text: adapter("message.send.text", async () => receipt("text")),
      // Its caption goes as the agent's words, then its file to the core, as a file in a reply does. The
      // receipt waits for the core: a refusal, or no core, is a failed send.
      media: async (params: unknown) => {
        const connector = current;
        if (connector === null) throw new Error("marmot: not connected to the core");
        return connector.sendMedia(params, () => receipt("media"));
      },
      payload: adapter("message.send.payload", async () => receipt("text")),
    },
    receive: { defaultAckPolicy: "after_agent_dispatch", supportedAckPolicies: ["after_agent_dispatch", "manual"] },
  });
  const base = {
    id: ID,
    meta: {
      id: ID,
      label: "Marmot",
      selectionLabel: "Marmot",
      docsPath: "channels/marmot",
      blurb: "Marmot group chats, through marmot-agent.",
      markdownCapable: true,
    },
    capabilities: { chatTypes: ["group"], reply: true, media: true, reactions: false, edit: false, unsend: false, blockStreaming: true },
    config: {
      listAccountIds: () => ["default"],
      resolveAccount: (_cfg: unknown, accountId?: string) => ({
        accountId: accountId ?? "default",
        dmPolicy: "open",
        groupPolicy: "open",
        allowFrom: ["*"],
        groupAllowFrom: ["*"],
        requireMention: false,
      }),
      inspectAccount: () => ({ enabled: true, configured: true }),
    },
    message,
    messaging: {
      targetPrefixes: [ID],
      normalizeTarget: (raw: unknown) => String(raw).trim().replace(/^marmot:/, "") || undefined,
      inferTargetChatType: () => "group",
      targetResolver: {
        hint: "<chat id>",
        looksLikeId: () => true,
        resolveTarget: async ({ input }: { input: unknown }) => {
          const to = String(input).trim().replace(/^marmot:/, "");
          return to ? { to, kind: "group", source: "normalized" } : null;
        },
      },
    },
    agentPrompt: { messageToolHints: () => [] },
    // The agent's message tool is not offered actions in a Marmot chat until the socket's chat section
    // is designed (`docs/design/socket-protocol.md`).
    actions: {
      describeMessageTool: () => ({ actions: [] }),
      supportsAction: () => false,
    },
    groups: { resolveRequireMention: () => false },
    heartbeat: { sendTyping: async () => undefined, clearTyping: async () => undefined },
    gateway: { startAccount },
    // The plugin claims OpenClaw's approval capability: it gets each request with its fields, and is
    // asked who may approve, which the core decides (D-010).
    approvalCapability: {
      authorizeActorAction: (params: unknown) => {
        const p = object(params);
        return current?.mayApprove(p["senderId"]) ?? { authorized: false, reason: "Not connected to the core." };
      },
      getActionAvailabilityState: () => ({ kind: "enabled" }),
      render: {
        exec: {
          buildPendingPayload: adapter("approvalCapability.render.exec.buildPendingPayload", () => null),
          buildResolvedPayload: adapter("approvalCapability.render.exec.buildResolvedPayload", () => null),
        },
        plugin: { buildPendingPayload: () => null, buildResolvedPayload: () => null },
      },
    },
  };
  return createChatChannelPlugin({
    base,
    security: {
      dm: { channelKey: ID, resolvePolicy: () => "open", resolveAllowFrom: () => ["*"], defaultPolicy: "open" },
    },
    threading: { topLevelReplyToMode: "reply" },
  });
}

export default defineChannelPluginEntry({
  id: ID,
  name: "Marmot",
  description: "Marmot group chats, through marmot-agent.",
  plugin: buildPlugin(),
  registerFull(api: unknown) {
    const version = object(object(api)["runtime"])["version"];
    if (typeof version === "string" && version !== "") openclawVersion = version;
  },
});
