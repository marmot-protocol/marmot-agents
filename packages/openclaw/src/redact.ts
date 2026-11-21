/**
 * What never leaves the plugin: the whole value of a secret OpenClaw may put in its own replies, such as
 * the model key. The masked form OpenClaw shows in `/model status`, which keeps a few of the key's
 * characters, is let through (D-088); it never holds the whole key.
 *
 * Secrets are replaced in the strings of an event before it is serialized, so a key holding a quote or
 * a backslash, which serialization escapes, is found as it is. Every value is replaced, however short:
 * a short secret is still a secret, and the ones configured are long keys.
 */

export const SECRET = "<secret>";

/** The values of the environment variables named in `MARMOT_SECRET_ENV`. */
export function secretsFrom(env: Record<string, string | undefined>): string[] {
  return (env["MARMOT_SECRET_ENV"] ?? "")
    .split(",")
    .map((name) => (name.trim() === "" ? "" : (env[name.trim()] ?? "")))
    .filter((v) => v.length > 0);
}

/**
 * Gives a function that copies a JSON value with every secret in its strings, keys included, replaced
 * by {@link SECRET}; or null if there are none.
 */
export function redactor(secrets: readonly string[]): (<T>(value: T) => T) | null {
  // The longest first, so a secret that holds another is replaced whole.
  const values = [...new Set(secrets.filter((v) => v.length > 0))].sort((a, b) => b.length - a.length);
  if (values.length === 0) return null;
  const line = (s: string): string => {
    for (const v of values) if (s.includes(v)) s = s.split(v).join(SECRET);
    return s;
  };
  const walk = (value: unknown): unknown => {
    if (typeof value === "string") return line(value);
    if (Array.isArray(value)) return value.map(walk);
    if (typeof value === "object" && value !== null) {
      const out: Record<string, unknown> = {};
      for (const [k, x] of Object.entries(value)) out[line(k)] = walk(x);
      return out;
    }
    return value;
  };
  return <T>(value: T) => walk(value) as T;
}
