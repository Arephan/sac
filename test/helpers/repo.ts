import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

/** Every file in a folder (skipping .git), as relative path -> bytes. */
export function treeOf(dir: string): Map<string, Buffer> {
  const out = new Map<string, Buffer>();
  const walk = (d: string) => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      if (e.name === ".git") continue;
      const p = join(d, e.name);
      if (e.isDirectory()) walk(p);
      else out.set(relative(dir, p), readFileSync(p));
    }
  };
  walk(dir);
  return new Map([...out.entries()].sort(([a], [b]) => (a < b ? -1 : 1)));
}

export const modeOf = (p: string) => statSync(p).mode & 0o777;
