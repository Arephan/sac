import { existsSync, mkdirSync, realpathSync, rmSync } from "node:fs";
import { homedir } from "node:os";
import { createHash } from "node:crypto";
import { join, posix, resolve } from "node:path";
import deepmerge from "deepmerge";
import { execa } from "execa";
import { mapImports } from "./capture/files.js";
import { expandTilde } from "./paths.js";
import { readMeta, readRepo } from "./repo.js";
import type { SavedFile, Setup, Warning } from "./types.js";

export interface Layer {
  name: string;
  source: string;
  dir: string;
  setup: Setup;
}

export const MAX_DEPTH = 5;

export function isGitUrl(s: string): boolean {
  return /^(https?:\/\/|ssh:\/\/|git@|file:\/\/)/.test(s) || s.endsWith(".git");
}

const firstLine = (s: unknown) => String(s ?? "").trim().split("\n")[0];

async function fetchGit(url: string, cacheDir: string): Promise<string> {
  if (url.startsWith("-")) throw new Error(`not a git URL: ${url}`);
  const readable = url.replace(/^[a-z]+:\/\//, "").replace(/[^A-Za-z0-9._-]+/g, "_").slice(-60);
  const dir = join(cacheDir, `${readable}-${createHash("sha256").update(url).digest("hex").slice(0, 12)}`);
  if (existsSync(join(dir, ".git"))) {
    const origin = await execa("git", ["-C", dir, "remote", "get-url", "origin"], { reject: false });
    const pull = String(origin.stdout).trim() !== url ? null : await execa("git", ["-C", dir, "pull", "-q", "--ff-only"], { reject: false });
    if (pull?.exitCode === 0) return dir;
    rmSync(dir, { recursive: true, force: true });
  }
  mkdirSync(cacheDir, { recursive: true });
  const clone = await execa("git", ["clone", "-q", "--depth", "1", "--", url, dir], { reject: false });
  if (clone.exitCode !== 0) throw new Error(`could not clone ${url}: ${firstLine(clone.stderr)}`);
  return dir;
}

/** Loads setup repos and everything they extend, bases first, each once. */
export async function loadLayers(sources: string[], opts: { cacheDir: string; cwd?: string }): Promise<Layer[]> {
  const layers: Layer[] = [];
  const seen = new Set<string>();
  const visit = async (source: string, from: string, stack: Array<{ key: string; name: string }>) => {
    const dir = isGitUrl(source) && !existsSync(resolve(from, source)) ? await fetchGit(source, opts.cacheDir) : resolve(from, expandTilde(source, homedir()));
    const meta = readMeta(dir);
    const key = realpathSync(dir);
    if (stack.some((s) => s.key === key)) throw new Error(`extends cycle: ${[...stack.map((s) => s.name), meta.name].join(" -> ")}`);
    if (seen.has(key)) return;
    if (stack.length > MAX_DEPTH) throw new Error(`extends goes more than ${MAX_DEPTH} levels deep: ${[...stack.map((s) => s.name), meta.name].join(" -> ")}`);
    for (const e of meta.extends ?? []) await visit(e, dir, [...stack, { key, name: meta.name }]);
    seen.add(key);
    const setup = readRepo(dir);
    let name = setup.name;
    for (let i = 2; layers.some((l) => l.name === name); i++) name = `${setup.name}-${i}`;
    layers.push({ name, source, dir, setup });
  };
  for (const s of sources) await visit(s, opts.cwd ?? process.cwd(), []);
  return layers;
}

const mergeable = (v: unknown) => Array.isArray(v) || (!!v && typeof v === "object" && Object.getPrototypeOf(v) === Object.prototype);

function union(a: unknown[], b: unknown[]): unknown[] {
  const seen = new Set(a.map((x) => JSON.stringify(x)));
  const out = [...a];
  for (const x of b) {
    const k = JSON.stringify(x);
    if (!seen.has(k)) {
      seen.add(k);
      out.push(x);
    }
  }
  return out;
}

/** Settings merge: lists are combined without duplicates, objects merge, other values from `over` win. */
export function mergeSettings<T extends Record<string, unknown>>(base: T, over: Record<string, unknown>): T {
  return deepmerge(base, over, { arrayMerge: union, isMergeableObject: mergeable }) as T;
}

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/** Merges layers per the spec: the top layer's CLAUDE.md stays, lower ones move to rules/sac-<name>.md. */
export function mergeLayers(layers: Layer[]): { setup: Setup; warnings: Warning[] } {
  if (!layers.length) throw new Error("no setup to restore");
  const warnings: Warning[] = [];
  const clash = (what: string, by: string, over: string) => warnings.push({ kind: "clash", message: `${what}: ${by} replaces ${over}'s version` });
  const files = new Map<string, { file: SavedFile; from: string }>();
  let settings: Record<string, unknown> | null = null;
  const keyed = <T>(map: Map<string, { v: T; from: string }>, what: string, key: string, v: T, from: string) => {
    const prev = map.get(key);
    if (prev && !same(prev.v, v)) clash(`${what} ${key}`, from, prev.from);
    map.set(key, { v, from });
  };
  const mcp = new Map<string, { v: Setup["mcp"][string]; from: string }>();
  const marketplaces = new Map<string, { v: Setup["marketplaces"][number]; from: string }>();
  const plugins = new Map<string, { v: Setup["plugins"][number]; from: string }>();
  const jobs = new Map<string, { v: Setup["jobs"][number]; from: string }>();

  layers.forEach((layer, i) => {
    const top = i === layers.length - 1;
    const known = new Set(layer.setup.files.map((f) => f.path));
    for (const f of layer.setup.files) {
      if (f.path === "settings.json") {
        try {
          const s = JSON.parse(f.data.toString("utf8"));
          settings = settings ? mergeSettings(settings, s) : s;
        } catch {
          warnings.push({ kind: "other", message: `${layer.name}: settings.json is not valid JSON; skipped` });
        }
        continue;
      }
      let file = f;
      if (f.path === "CLAUDE.md" && !top) {
        const text = mapImports(f.data.toString("utf8"), (p) => (!/^[~/]/.test(p) && known.has(posix.normalize(p)) ? `../${p}` : p));
        file = { ...f, path: `rules/sac-${layer.name}.md`, data: Buffer.from(text) };
      }
      const prev = files.get(file.path);
      if (prev && !prev.file.data.equals(file.data)) clash(file.path, layer.name, prev.from);
      files.set(file.path, { file, from: layer.name });
    }
    for (const [k, v] of Object.entries(layer.setup.mcp)) keyed(mcp, "MCP server", k, v, layer.name);
    for (const m of layer.setup.marketplaces) keyed(marketplaces, "marketplace", m.name, m, layer.name);
    for (const p of layer.setup.plugins) keyed(plugins, "plugin", p.id, p, layer.name);
    for (const j of layer.setup.jobs) keyed(jobs, "job", j.label, j, layer.name);
  });

  const out = [...files.values()].map((x) => x.file);
  if (settings) out.push({ path: "settings.json", data: Buffer.from(`${JSON.stringify(settings, null, 2)}\n`), mode: 0o644 });
  const top = layers[layers.length - 1];
  return {
    setup: {
      name: top.name,
      captured: top.setup.captured,
      extends: [],
      files: out.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0)),
      mcp: Object.fromEntries([...mcp].map(([k, x]) => [k, x.v])),
      marketplaces: [...marketplaces.values()].map((x) => x.v),
      plugins: [...plugins.values()].map((x) => x.v),
      jobs: [...jobs.values()].map((x) => x.v),
    },
    warnings,
  };
}
