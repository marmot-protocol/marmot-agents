/**
 * The connector's side of the core's local socket (`docs/design/socket-protocol.md`, "The messages, as
 * built"): JSON-RPC 2.0 over a Unix socket, one message per line, no line over 4 MB either way (D-033).
 *
 * The connector opens with a hello. Then the core sends `prompt`, `answer` and `control` requests, which
 * the connector answers with `{}` once it has handed them to Claude Code, or with a refusal; and the
 * connector sends the core `event` notifications (D-034, D-058).
 *
 * Nothing here writes what a message holds to a log (D-071).
 */

import { connect, type Socket } from "node:net";

/** The protocol the core speaks (D-020). */
export const PROTOCOL = "marmot.agent-control.v3";
/** The largest line either side may send (D-033). */
export const MAX_MESSAGE = 4 * 1024 * 1024;

/** The size of a notification's line in bytes, without its newline, as {@link CoreConnection.notify} writes it. */
export function notificationSize(method: string, params: unknown): number {
  return Buffer.byteLength(JSON.stringify({ jsonrpc: "2.0", method, params }));
}

/** The eight reasons a request may be refused (`crates/marmot-agent-driver`, `Reason`). */
export type Reason =
  | "not_supported"
  | "busy"
  | "unknown_item"
  | "not_waiting"
  | "already_answered"
  | "nothing_to_do"
  | "invalid"
  | "failed";

/** A request not taken. The detail is shown in the chat, to every member. */
export class Refusal extends Error {
  readonly reason: Reason;
  readonly detail: string | undefined;
  constructor(reason: Reason, detail?: string) {
    super(reason);
    this.reason = reason;
    this.detail = detail;
  }
}

/** The seven capability flags, every one, every time (D-058). */
export interface Capabilities {
  interrupt: boolean;
  mid_turn: "fold" | "restart" | "queue";
  sessions: "core" | "own";
  resume: boolean;
  approvals: boolean;
  files_in: "none" | "images" | "any";
  approval_commands: { command: string; counts: "start" | "anywhere" }[];
}

export interface Hello {
  connector: { name: string; version: string };
  harness: { name: string; version?: string };
  token: string;
  capabilities: Capabilities;
}

/** What the core answered to the hello. */
export interface Welcome {
  core?: { version?: string };
  agent?: { name?: string };
}

/** Serves one request from the core. It resolves once the request is taken, and throws a {@link Refusal}. */
export type Serve = (method: string, params: Record<string, unknown>) => Promise<void>;

/** The hello was refused, or the connection went before the core answered it. */
export class HelloRefused extends Error {}

/**
 * One connection to the core. Requests from the core are served one at a time, in the order they came,
 * so a request is never taken before the one sent ahead of it.
 */
export class CoreConnection {
  private readonly socket: Socket;
  private buffer: Buffer[] = [];
  private buffered = 0;
  private tooLong = false;
  private serve: Serve = async () => {
    throw new Refusal("not_supported");
  };
  private queue: Promise<void> = Promise.resolve();
  private nextId = 0;
  private readonly waiting = new Map<number, { resolve: (r: unknown) => void; reject: (e: Error) => void }>();
  private readonly closedPromise: Promise<void>;
  private closed = false;

  private constructor(socket: Socket) {
    this.socket = socket;
    this.closedPromise = new Promise((resolve) => {
      socket.on("close", () => {
        this.closed = true;
        for (const w of this.waiting.values()) w.reject(new HelloRefused("the connection closed"));
        this.waiting.clear();
        resolve();
      });
    });
    socket.on("error", () => socket.destroy());
    socket.on("data", (chunk: Buffer) => this.read(chunk));
  }

  /** Connects to the socket at `path`. */
  static open(path: string): Promise<CoreConnection> {
    return new Promise((resolve, reject) => {
      const socket = connect(path);
      const failed = (e: Error) => reject(e);
      socket.once("error", failed);
      socket.once("connect", () => {
        socket.off("error", failed);
        resolve(new CoreConnection(socket));
      });
    });
  }

  /** Resolves once the connection has closed, from either side. */
  get done(): Promise<void> {
    return this.closedPromise;
  }

  /** Gives the hello, and sets what serves the core's requests once it is taken. */
  async hello(hello: Hello, serve: Serve): Promise<Welcome> {
    this.serve = serve;
    try {
      return (await this.request("hello", { protocol: PROTOCOL, ...hello })) as Welcome;
    } catch (e) {
      throw e instanceof HelloRefused ? e : new HelloRefused("the core refused the hello");
    }
  }

  /** Sends a notification. One over the limit is not sent, and false says so. */
  notify(method: string, params: unknown): boolean {
    return this.write({ jsonrpc: "2.0", method, params });
  }

  close(): void {
    this.socket.end();
  }

  private request(method: string, params: unknown): Promise<unknown> {
    this.nextId += 1;
    const id = this.nextId;
    return new Promise((resolve, reject) => {
      this.waiting.set(id, { resolve, reject });
      if (!this.write({ jsonrpc: "2.0", id, method, params })) {
        this.waiting.delete(id);
        reject(new Error("the request could not be sent"));
      }
    });
  }

  private write(message: unknown): boolean {
    if (this.closed) return false;
    const line = JSON.stringify(message);
    if (Buffer.byteLength(line) > MAX_MESSAGE) return false;
    this.socket.write(line + "\n");
    return true;
  }

  /** Splits what arrives into lines. A line over the limit is passed over as it arrives, unheld. */
  private read(chunk: Buffer): void {
    let rest = chunk;
    for (;;) {
      const at = rest.indexOf(0x0a);
      const part = at === -1 ? rest : rest.subarray(0, at);
      if (!this.tooLong) {
        if (this.buffered + part.length > MAX_MESSAGE) {
          this.tooLong = true;
          this.buffer = [];
          this.buffered = 0;
        } else {
          this.buffer.push(part);
          this.buffered += part.length;
        }
      }
      if (at === -1) return;
      const line = Buffer.concat(this.buffer).toString("utf8");
      const skip = this.tooLong;
      this.buffer = [];
      this.buffered = 0;
      this.tooLong = false;
      if (!skip) this.message(line);
      rest = rest.subarray(at + 1);
    }
  }

  private message(line: string): void {
    if (line.trim() === "") return;
    let m: Record<string, unknown>;
    try {
      const parsed: unknown = JSON.parse(line);
      if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return;
      m = parsed as Record<string, unknown>;
    } catch {
      return;
    }
    if (m["jsonrpc"] !== "2.0") return;
    const id = m["id"];
    const method = m["method"];
    if (typeof method === "string") {
      if (id === undefined || id === null) return; // The core sends the connector no notifications.
      const params = m["params"];
      const p = typeof params === "object" && params !== null && !Array.isArray(params) ? params : {};
      this.queue = this.queue.then(() => this.answer(id, method, p as Record<string, unknown>));
      return;
    }
    if (typeof id === "number") {
      const w = this.waiting.get(id);
      if (w === undefined) return;
      this.waiting.delete(id);
      if ("result" in m && !("error" in m)) w.resolve(m["result"]);
      else w.reject(new HelloRefused("refused"));
    }
  }

  private async answer(id: unknown, method: string, params: Record<string, unknown>): Promise<void> {
    try {
      await this.serve(method, params);
      this.write({ jsonrpc: "2.0", id, result: {} });
    } catch (e) {
      const refusal = e instanceof Refusal ? e : new Refusal("failed");
      const data: Record<string, unknown> = { reason: refusal.reason };
      if (refusal.detail !== undefined) data["detail"] = refusal.detail;
      this.write({ jsonrpc: "2.0", id, error: { code: -32000, message: refusal.reason, data } });
    }
  }
}
