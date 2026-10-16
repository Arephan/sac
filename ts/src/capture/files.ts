import { existsSync, lstatSync, readFileSync, realpathSync, statSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import fg from "fast-glob";
import { expandTilde } from "../paths.js";
import type { SavedFile, Target, Warning } from "../types.js";

/** Folders under the Claude folder that are saved whole. */
export const DIR_ITEMS = ["rules", "skills", "agents", "commands", "output-styles"];

/** Never saved from inside those folders. */
export const IGNORE = ["**/node_modules/**", "**/.git/**", "**/.venv/**", "**/__pycache__/**", "**/.cache/**", "**/.DS_Store", "**/*.log"];

/** Claude Code's own state: never saved, even when a setting points into it. */
export const NEVER = ["plugins", "projects", "sessions", "todos", "shell-snapshots", "statsig", "ide", "local", "cache", "debug", "file-history", "logs", "telemetry", "paste-cache", "history.jsonl", ".credentials.json", "settings.local.json"];

const MAX_BYTES = 5 * 1024 * 1024;

export function isText(data: Buffer): boolean {
  if (data.includes(0)) return false;
  try {
    new TextDecoder("utf-8", { fatal: true }).decode(data);
    return true;
  } catch {
    return false;
  }
}

/** Rewrites the path of every `@path` import that sits outside inline code and fenced blocks. */
export function mapImports(markdown: string, f: (path: string) => string): string {
  let fenced = false;
  return markdown
    .split("\n")
    .map((line) => {
      if (/^\s*(```|~~~)/.test(line)) {
        fenced = !fenced;
        return line;
      }
      if (fenced) return line;
      return line
        .split(/(`[^`]*`)/)
        .map((part, i) =>
          i % 2 === 1
            ? part
            : part.replace(/(^|\s)@((?:\\ |[^\s])+)/g, (_m, pre: string, raw: string) => {
                const path = raw.replace(/\\ /g, " ");
                return `${pre}@${f(path).replace(/ /g, "\\ ")}`;
              }),
        )
        .join("");
    })
    .join("\n");
}

export function findImports(markdown: string): string[] {
  const found: string[] = [];
  mapImports(markdown, (p) => {
    found.push(p);
    return p;
  });
  return found;
}

const inside = (child: string, parent: string) => {
  const r = relative(parent, child);
  return r !== "" && !r.startsWith("..") && !r.startsWith(sep);
};

/** Script paths named in a shell command: absolute, ~/, $HOME/ or ${HOME}/, quotes stripped. */
function scriptPaths(command: string, home: string): string[] {
  return command
    .split(/\s+|&&|\|\||;|\|/)
    .map((tok) => tok.replace(/^["']|["']$/g, ""))
    .map((tok) => tok.replace(/^\$\{?HOME\}?(?=\/)/, home))
    .map((tok) => expandTilde(tok, home))
    .filter((tok) => tok.startsWith("/"));
}

export async function readFiles(t: Target): Promise<{ files: SavedFile[]; warnings: Warning[] }> {
  const files = new Map<string, SavedFile>();
  const warnings: Warning[] = [];
  if (!existsSync(t.claudeDir)) return { files: [], warnings };
  const root = realpathSync(t.claudeDir);
  const rel = (abs: string) => relative(t.claudeDir, abs).split(sep).join("/");
  const dep = (message: string) => warnings.push({ kind: "dependency", message });
  const add = (abs: string) => {
    const st = statSync(abs);
    if (st.size > MAX_BYTES) {
      warnings.push({ kind: "other", message: `${rel(abs)} is larger than 5 MB; not saved` });
      return null;
    }
    const f: SavedFile = { path: rel(abs), data: readFileSync(abs), mode: st.mode & 0o777 };
    files.set(f.path, f);
    return f;
  };

  const visit = (abs: string, from: string, depth: number) => {
    if (depth > 4 || !existsSync(abs) || !statSync(abs).isFile()) return;
    if (!inside(abs, t.claudeDir)) {
      dep(`${from} imports ${abs}, which is outside the Claude folder; not saved`);
      return;
    }
    if (files.has(rel(abs))) return;
    if (lstatSync(abs).isSymbolicLink()) {
      dep(`${rel(abs)} is a symlink; not saved`);
      return;
    }
    const f = add(abs);
    if (f && abs.endsWith(".md") && isText(f.data)) {
      for (const imp of findImports(f.data.toString("utf8"))) visit(resolve(dirname(abs), expandTilde(imp, t.home)), f.path, depth + 1);
    }
  };
  visit(join(t.claudeDir, "CLAUDE.md"), "CLAUDE.md", 0);

  for (const name of ["settings.json", "keybindings.json"]) {
    const abs = join(t.claudeDir, name);
    if (existsSync(abs) && statSync(abs).isFile()) add(abs);
  }

  for (const dir of DIR_ITEMS) {
    const base = join(t.claudeDir, dir);
    if (!existsSync(base)) continue;
    if (lstatSync(base).isSymbolicLink()) {
      dep(`${dir} is a symlink; not saved`);
      continue;
    }
    const entries = await fg("**/*", { cwd: base, dot: true, onlyFiles: false, followSymbolicLinks: false, objectMode: true, ignore: IGNORE });
    for (const e of entries) {
      const abs = join(base, e.path);
      if (e.dirent.isSymbolicLink()) dep(`${rel(abs)} is a symlink; not saved`);
      else if (e.dirent.isFile()) add(abs);
    }
  }

  const settings = files.get("settings.json");
  if (settings) {
    let s: { hooks?: Record<string, Array<{ hooks?: Array<{ command?: unknown }> }>>; statusLine?: { command?: unknown } } = {};
    try {
      s = JSON.parse(settings.data.toString("utf8"));
    } catch {
      warnings.push({ kind: "other", message: "settings.json is not valid JSON; the scripts it runs were not looked for" });
    }
    const commands: string[] = [];
    for (const groups of Object.values(s.hooks ?? {})) {
      for (const g of Array.isArray(groups) ? groups : []) for (const h of g?.hooks ?? []) if (typeof h?.command === "string") commands.push(h.command);
    }
    if (typeof s.statusLine?.command === "string") commands.push(s.statusLine.command);
    for (const cmd of commands) {
      for (const abs of scriptPaths(cmd, t.home)) {
        if (!existsSync(abs) || !statSync(abs).isFile() || files.has(rel(abs))) continue;
        if (inside(realpathSync(abs), root)) {
          if (!NEVER.includes(relative(root, realpathSync(abs)).split(sep)[0])) add(abs);
        }
        else dep(`settings.json runs ${abs}, which is outside the Claude folder; not saved`);
      }
    }
  }

  return { files: [...files.values()].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0)), warnings };
}
