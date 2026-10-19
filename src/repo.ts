import { chmodSync, existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, realpathSync, rmdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { basename, dirname, join, relative, sep } from "node:path";
import { createTwoFilesPatch } from "diff";
import { execa } from "execa";
import fg from "fast-glob";
import YAML from "yaml";
import { isText } from "./capture/files.js";
import type { JobDef, Marketplace, PluginEntry, SavedFile, SecretRef, Setup } from "./types.js";

/** Repo-relative path -> contents. */
export type RepoFiles = Map<string, { data: Buffer; mode: number }>;

/** Paths sac owns inside a setup repo. Anything else in the folder is the user's. */
export const MANAGED = ["sac.yaml", "claude", "mcp.json", "plugins.json", "jobs", ".env.example", ".gitignore"];
const MIRRORED = ["claude", "jobs"];

export interface RepoMeta {
  version: 1;
  name: string;
  captured?: Setup["captured"];
  extends?: string[];
}

const json = (v: unknown) => Buffer.from(`${JSON.stringify(v, null, 2)}\n`);
const jobFile = (label: string) => `jobs/${label.replace(/[/\\]/g, "_")}.yaml`;

export function envExample(secrets: SecretRef[]): string {
  const lines = [
    "# Secrets this setup needs. Their values are never saved here.",
    "# Copy this file to .env (never commit it) or export the variables before `sac restore`.",
    "",
  ];
  for (const s of secrets) lines.push(`# used by ${s.where}`, `${s.name}=`, "");
  return lines.join("\n");
}

export function setupToRepo(setup: Setup, secrets: SecretRef[]): RepoFiles {
  const files: RepoFiles = new Map();
  const meta: RepoMeta = { version: 1, name: setup.name, ...(setup.captured ? { captured: setup.captured } : {}) };
  if (setup.extends.length) meta.extends = setup.extends;
  files.set("sac.yaml", { data: Buffer.from(YAML.stringify(meta)), mode: 0o644 });
  for (const f of setup.files) files.set(`claude/${f.path}`, { data: f.data, mode: f.mode });
  files.set("mcp.json", { data: json({ mcpServers: setup.mcp }), mode: 0o644 });
  files.set("plugins.json", { data: json({ marketplaces: setup.marketplaces, plugins: setup.plugins }), mode: 0o644 });
  for (const j of setup.jobs) files.set(jobFile(j.label), { data: Buffer.from(YAML.stringify(j)), mode: 0o644 });
  files.set(".env.example", { data: Buffer.from(envExample(secrets)), mode: 0o644 });
  return files;
}

export function readMeta(dir: string): RepoMeta {
  const path = join(dir, "sac.yaml");
  if (!existsSync(path)) throw new Error(`${dir} is not a setup repo (no sac.yaml)`);
  if (!isPlain(path)) throw new Error(`${path} is not a regular file`);
  const meta = YAML.parse(readFileSync(path, "utf8")) ?? {};
  if (meta.version !== 1) throw new Error(`${path}: unsupported version ${JSON.stringify(meta.version)}`);
  return { version: 1, name: typeof meta.name === "string" && meta.name ? meta.name : basename(dir), captured: meta.captured, extends: Array.isArray(meta.extends) ? meta.extends.map(String) : [] };
}

/**
 * Regular files under `root` matching `pattern`, never through a symlink: a repo cloned from a
 * git URL could otherwise point a setup file at anything the user can read.
 */
function plainFiles(root: string, pattern: string): string[] {
  if (!existsSync(root) || lstatSync(root).isSymbolicLink()) return [];
  const real = realpathSync(root);
  return fg
    .sync(pattern, { cwd: root, dot: true, onlyFiles: true, followSymbolicLinks: false })
    .filter((p) => {
      const abs = join(root, p);
      if (lstatSync(abs).isSymbolicLink() || !lstatSync(abs).isFile()) return false;
      const r = relative(real, realpathSync(abs));
      return !r.startsWith("..") && !r.startsWith(sep);
    })
    .sort();
}

const isPlain = (path: string) => existsSync(path) && !lstatSync(path).isSymbolicLink() && lstatSync(path).isFile();

const readJson = (path: string): any => {
  if (!existsSync(path)) return {};
  if (!isPlain(path)) throw new Error(`${path} is not a regular file`);
  try {
    return JSON.parse(readFileSync(path, "utf8")) ?? {};
  } catch (e) {
    throw new Error(`${path}: ${(e as Error).message}`);
  }
};

export function readRepo(dir: string): Setup {
  const meta = readMeta(dir);
  const files: SavedFile[] = plainFiles(join(dir, "claude"), "**/*").map((p) => {
        const abs = join(dir, "claude", p);
        return { path: p.split(sep).join("/"), data: readFileSync(abs), mode: statSync(abs).mode & 0o777 };
      });
  const plugins = readJson(join(dir, "plugins.json"));
  const jobs: JobDef[] = plainFiles(join(dir, "jobs"), "*.yaml").map((p) => {
        const j = YAML.parse(readFileSync(join(dir, "jobs", p), "utf8")) ?? {};
        return { ...j, label: String(j.label ?? p.replace(/\.yaml$/, "")), kind: j.kind === "cron" ? "cron" : "launchd", program: (j.program ?? []).map(String), schedule: j.schedule ?? {}, env: j.env ?? {}, dependencies: j.dependencies ?? [] };
      });
  return {
    name: meta.name,
    captured: meta.captured,
    extends: meta.extends ?? [],
    files,
    mcp: readJson(join(dir, "mcp.json")).mcpServers ?? {},
    marketplaces: ((plugins.marketplaces ?? []) as Marketplace[]).map((m) => ({ name: m.name, source: m.source, kind: m.kind ?? "other" })),
    plugins: ((plugins.plugins ?? []) as PluginEntry[]).map((p) => ({ id: p.id, enabled: p.enabled !== false, settings: p.settings ?? {} })),
    jobs,
  };
}

/** The managed files currently on disk (without .gitignore, which sac only appends to). */
export function readRepoFiles(dir: string): RepoFiles {
  const out: RepoFiles = new Map();
  if (!existsSync(dir)) return out;
  const paths = fg.sync(["sac.yaml", "mcp.json", "plugins.json", ".env.example", "claude/**/*", "jobs/**/*"], { cwd: dir, dot: true, onlyFiles: true });
  for (const p of paths.sort()) out.set(p, { data: readFileSync(join(dir, p)), mode: statSync(join(dir, p)).mode & 0o777 });
  return out;
}

export interface RepoChanges {
  added: string[];
  changed: string[];
  removed: string[];
}

export function repoChanges(dir: string, files: RepoFiles): RepoChanges {
  const disk = readRepoFiles(dir);
  const c: RepoChanges = { added: [], changed: [], removed: [] };
  for (const [p, f] of files) {
    const d = disk.get(p);
    if (!d) c.added.push(p);
    else if (!d.data.equals(f.data) || d.mode !== f.mode) c.changed.push(p);
  }
  for (const p of disk.keys()) if (!files.has(p) && MIRRORED.some((m) => p.startsWith(`${m}/`))) c.removed.push(p);
  for (const list of [c.added, c.changed, c.removed]) list.sort();
  return c;
}

export function diffRepo(dir: string, files: RepoFiles): string {
  const disk = readRepoFiles(dir);
  const c = repoChanges(dir, files);
  const out: string[] = [];
  for (const p of [...c.added, ...c.changed, ...c.removed].sort()) {
    const a = disk.get(p);
    const b = files.get(p);
    if ((a && !isText(a.data)) || (b && !isText(b.data))) {
      out.push(`Binary file ${p} ${!a ? "added" : !b ? "removed" : "differs"}\n`);
      continue;
    }
    if (a && b && a.data.equals(b.data)) {
      out.push(`mode ${p}: ${a.mode.toString(8)} -> ${b.mode.toString(8)}\n`);
      continue;
    }
    out.push(createTwoFilesPatch(a ? `a/${p}` : "/dev/null", b ? `b/${p}` : "/dev/null", a?.data.toString("utf8") ?? "", b?.data.toString("utf8") ?? ""));
  }
  return out.join("");
}

function pruneEmpty(dir: string, root: string): void {
  if (!existsSync(dir)) return;
  for (const e of readdirSync(dir, { withFileTypes: true })) if (e.isDirectory()) pruneEmpty(join(dir, e.name), root);
  if (dir !== root && readdirSync(dir).length === 0) rmdirSync(dir);
}

/** Mirrors `files` into the repo folder: writes what differs, removes stale claude/ and jobs/ files. */
export function writeRepo(dir: string, files: RepoFiles): RepoChanges {
  const c = repoChanges(dir, files);
  for (const p of [...c.added, ...c.changed]) {
    const abs = join(dir, p);
    mkdirSync(dirname(abs), { recursive: true });
    writeFileSync(abs, files.get(p)!.data);
    chmodSync(abs, files.get(p)!.mode);
  }
  for (const p of c.removed) rmSync(join(dir, p), { force: true });
  for (const m of MIRRORED) pruneEmpty(join(dir, m), join(dir, m));
  const gi = join(dir, ".gitignore");
  const current = existsSync(gi) ? readFileSync(gi, "utf8") : "";
  if (!/^\.env$/m.test(current)) writeFileSync(gi, `${current}${current && !current.endsWith("\n") ? "\n" : ""}.env\n`);
  return c;
}

/** Commits the managed paths. Returns false when there was nothing to commit. Never pushes. */
export async function commitRepo(dir: string, message: string): Promise<boolean> {
  const git = (args: string[]) => execa("git", ["-C", dir, ...args], { reject: false });
  if (!existsSync(join(dir, ".git"))) {
    const init = await git(["init", "-q", "-b", "main"]);
    if (init.exitCode !== 0) throw new Error(`git init failed: ${String(init.stderr).split("\n")[0]}`);
  }
  await git(["add", "-A", "--", ...MANAGED.filter((p) => existsSync(join(dir, p)))]);
  if ((await git(["diff", "--cached", "--quiet"])).exitCode === 0) return false;
  const who = (await git(["config", "user.email"])).stdout ? [] : ["-c", "user.name=sac", "-c", "user.email=sac@localhost"];
  const r = await git([...who, "commit", "-q", "-m", message]);
  if (r.exitCode !== 0) throw new Error(`git commit failed: ${String(r.stderr).split("\n")[0]}`);
  return true;
}
