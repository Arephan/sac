import { homedir } from "node:os";
import { resolve } from "node:path";
import { expandTilde } from "./paths.js";

export interface Io {
  out: (s: string) => void;
  err: (s: string) => void;
}

export const stdio: Io = { out: (s) => process.stdout.write(s), err: (s) => process.stderr.write(s) };

/** Shows a path with the home folder as ~. */
export const pretty = (p: string) => {
  const h = homedir();
  return p === h ? "~" : p.startsWith(`${h}/`) ? `~${p.slice(h.length)}` : p;
};

export const absolute = (p: string) => resolve(expandTilde(p, homedir()));

export const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** Two columns, the first padded to its widest entry. */
export function table(rows: Array<[string, string]>, indent = "  "): string {
  const w = Math.max(0, ...rows.map(([a]) => a.length));
  return rows.map(([a, b]) => `${indent}${a.padEnd(w)}  ${b}`.trimEnd()).join("\n") + (rows.length ? "\n" : "");
}
