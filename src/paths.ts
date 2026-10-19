import { homedir } from "node:os";
import { join } from "node:path";
import type { Target } from "./types.js";

export function targetFor(opts: { configDir?: string | null; home?: string; platform?: "darwin" | "linux" } = {}): Target {
  const home = opts.home ?? homedir();
  const configDir = opts.configDir ?? null;
  return {
    home,
    claudeDir: configDir ?? join(home, ".claude"),
    claudeJson: configDir ? join(configDir, ".claude.json") : join(home, ".claude.json"),
    configDir,
    platform: opts.platform ?? (process.platform === "darwin" ? "darwin" : "linux"),
  };
}

/** "~" and a leading "~/" become absolute; anything else is unchanged. */
export function expandTilde(s: string, home: string): string {
  if (s === "~") return home;
  return s.startsWith("~/") ? home + s.slice(1) : s;
}

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** Matches `path` only as a whole path prefix: not inside another path, not a longer name. */
const prefix = (path: string) => new RegExp(`(?<![A-Za-z0-9._~-])${escape(path)}(?![A-Za-z0-9._-])`, "g");

/** Machine paths to portable tokens: the Claude folder becomes {{claude}}, then home becomes {{home}}. */
export function tokenize(s: string, t: Pick<Target, "home" | "claudeDir">): string {
  return s.replace(prefix(t.claudeDir), "{{claude}}").replace(prefix(t.home), "{{home}}");
}

export function detokenize(s: string, t: Pick<Target, "home" | "claudeDir">): string {
  return s.split("{{claude}}").join(t.claudeDir).split("{{home}}").join(t.home);
}

export function deepMapStrings<T>(v: T, f: (s: string, path: string[]) => string, path: string[] = []): T {
  if (typeof v === "string") return f(v, path) as T;
  if (Array.isArray(v)) return v.map((x, i) => deepMapStrings(x, f, [...path, String(i)])) as T;
  if (v && typeof v === "object" && !Buffer.isBuffer(v)) {
    return Object.fromEntries(Object.entries(v as Record<string, unknown>).map(([k, x]) => [k, deepMapStrings(x, f, [...path, k])])) as T;
  }
  return v;
}
