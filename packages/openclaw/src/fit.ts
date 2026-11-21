/**
 * An event cut to fit one line of the socket (D-033).
 *
 * Copied from `fitEvent` in `packages/claude-code/src/connector.ts`, made to take any event: each package
 * stands alone. Keep the two the same.
 */

type Json = Record<string, unknown>;

/** What of an event can be cut. */
export interface Fittable {
  title: string;
  text: string;
  body: Json;
  native?: unknown;
}

/** What is put where text was cut to fit the socket. */
const CUT = "\n[… cut to fit …]\n";
const CUT_BYTES = Buffer.byteLength(JSON.stringify(CUT)) - 2;

/** Whether the UTF-16 unit at `i` starts or ends a pair, which a cut must not split. */
function high(s: string, i: number): boolean {
  const c = s.charCodeAt(i);
  return c >= 0xd800 && c <= 0xdbff;
}
function low(s: string, i: number): boolean {
  const c = s.charCodeAt(i);
  return c >= 0xdc00 && c <= 0xdfff;
}

/** Where a string is: the object or list that holds it, and its key there. */
interface Spot {
  owner: Record<string | number, unknown>;
  key: string | number;
  length: number;
}

/** The longest string in `value`, or in `best` if that is longer. */
function longest(value: unknown, at: Spot | null = null): Spot | null {
  let best = at;
  const visit = (owner: Record<string | number, unknown>, key: string | number, v: unknown) => {
    if (typeof v === "string") {
      if (best === null || v.length > best.length) best = { owner, key, length: v.length };
    } else if (typeof v === "object" && v !== null) {
      best = longest(v, best);
    }
  };
  if (typeof value === "object" && value !== null) {
    for (const [k, v] of Object.entries(value)) visit(value as Record<string, unknown>, Array.isArray(value) ? Number(k) : k, v);
  }
  return best;
}

/**
 * An event as it fits on one line of the socket (D-033). One over the limit goes without its native
 * message; if it is still over, its longest texts are cut in the middle, keeping their start and end
 * with a mark between, until it fits. Only `title`, `text` and `body` are cut: the event's ids, state,
 * `prompts` and `settles` stay as they are. This is the socket's limit, not the item's 64 KB, which the
 * core applies (D-057). Says what it did, or returns null when nothing could make it fit.
 */
export function fitEvent<E extends Fittable>(event: E, size: (e: E) => number, limit: number): { event: E; did: string | null } | null {
  if (size(event) <= limit) return { event, did: null };
  const { native: _native, ...rest } = event;
  let out = rest as E;
  if (size(out) <= limit) return { event: out, did: "sent without its native message" };
  out = structuredClone(out);
  const parts: Json = { title: out.title, text: out.text, body: out.body };
  for (;;) {
    const over = size(out) - limit;
    if (over <= 0) return { event: out, did: "cut to fit" };
    const at = longest(parts);
    if (at === null || at.length <= 2 * CUT.length) return null;
    const s = at.owner[at.key] as string;
    // Removing a UTF-16 unit removes at least a byte, so this many go, at least.
    const keep = Math.max(0, s.length - over - CUT_BYTES);
    let head = Math.ceil(keep / 2);
    let tail = s.length - (keep - head);
    if (head > 0 && high(s, head - 1)) head -= 1;
    if (tail < s.length && low(s, tail)) tail += 1;
    at.owner[at.key] = s.slice(0, head) + CUT + s.slice(tail);
    out.title = parts["title"] as string;
    out.text = parts["text"] as string;
  }
}
