import { existsSync, readFileSync } from "node:fs";
import { basename, join } from "node:path";
import { isText, readFiles } from "./capture/files.js";
import { readJobs } from "./capture/jobs.js";
import { readUserMcp } from "./capture/mcp.js";
import { readPlugins } from "./capture/plugins.js";
import { claudeCli, claudeVersion, type ClaudeCli } from "./claude-cli.js";
import { deepMapStrings, tokenize } from "./paths.js";
import { commitRepo, diffRepo, readMeta, readRepoFiles, repoChanges, setupToRepo, writeRepo, type RepoChanges, type RepoFiles } from "./repo.js";
import { findSecretSpans, ph, redact, scanText, SecretVault } from "./secrets.js";
import type { SavedFile, SecretRef, Setup, Target, Warning } from "./types.js";

export interface Captured {
  setup: Setup;
  secrets: SecretRef[];
  /** placeholder name -> the value it replaced; used only to double-check the output */
  values: Record<string, string>;
  warnings: Warning[];
}

/** Reads the machine's setup with secrets replaced by placeholders and paths by tokens. */
export async function capture(t: Target, opts: { jobs?: string[]; cli?: ClaudeCli; now?: Date } = {}): Promise<Captured> {
  const cli = opts.cli ?? claudeCli(t);
  const vault = new SecretVault();
  const tok = <T>(v: T): T => deepMapStrings(v, (s) => tokenize(s, t));
  const { files: raw, warnings } = await readFiles(t);

  const files: SavedFile[] = [];
  for (const f of raw) {
    if (f.path === "settings.json") {
      try {
        const s = JSON.parse(f.data.toString("utf8"));
        delete s.enabledPlugins;
        files.push({ ...f, data: Buffer.from(`${JSON.stringify(tok(await redact(s, ["settings"], vault)), null, 2)}\n`) });
        continue;
      } catch {
        // not JSON: saved like any other text file
      }
    }
    if (!isText(f.data)) {
      files.push(f);
      continue;
    }
    const text = f.data.toString("utf8");
    const spans = await findSecretSpans(text);
    const rules = spans.length ? await scanText(text) : [];
    const named = spans.map(([a, b]) => {
      const line = text.slice(0, a).split("\n").length;
      return { a, b, line, name: vault.name(["files", f.path], text.slice(a, b)), rule: rules.find((r) => r.line === line)?.rule ?? "secret" };
    });
    let out = text;
    for (const s of [...named].reverse()) out = out.slice(0, s.a) + ph(s.name) + out.slice(s.b);
    for (const s of named) warnings.push({ kind: "secret-in-file", message: `${f.path} line ${s.line} holds a ${s.rule} credential; saved as ${ph(s.name)}` });
    files.push({ ...f, data: Buffer.from(tokenize(out, t)) });
  }

  const mcp = tok(await redact(readUserMcp(t), ["mcp"], vault));

  const captured = await readPlugins(cli);
  const plugins = [];
  for (const p of captured.plugins) {
    const settings = await redact(p.settings, ["plugins", p.id], vault, { sensitiveKeys: new Set(captured.sensitive[p.id] ?? []), byKey: "all" });
    plugins.push({ ...p, settings: tok(settings) });
  }
  const marketplaces = tok(captured.marketplaces);

  const found = await readJobs(t, { labels: opts.jobs });
  warnings.push(...found.warnings);
  const jobs = [];
  for (const j of found.jobs) {
    const env = await redact(j.env, ["jobs", j.label, "env"], vault);
    const program = await redact(j.program, ["jobs", j.label, "program"], vault);
    const extra = j.extra ? await redact(j.extra, ["jobs", j.label, "extra"], vault) : undefined;
    jobs.push(tok({ ...j, env, program, ...(extra ? { extra } : {}) }));
  }

  const setup: Setup = {
    name: "",
    captured: { at: (opts.now ?? new Date()).toISOString().replace(/\.\d{3}Z$/, "Z"), claude: await claudeVersion(cli), os: t.platform },
    extends: [],
    files,
    mcp,
    marketplaces,
    plugins,
    jobs,
  };
  return { setup, secrets: vault.refs(), values: vault.values(), warnings };
}

/** Throws if any text file still holds a detectable credential or one of the known values. */
export async function assertNoSecrets(files: RepoFiles, values: string[]): Promise<void> {
  for (const [p, f] of files) {
    if (!isText(f.data) || p === ".env.example") continue;
    const text = f.data.toString("utf8");
    const found = await scanText(text);
    if (found.length) throw new Error(`refusing to save: ${p} line ${found[0].line} still holds a ${found[0].rule} credential`);
    if (values.some((v) => v.length >= 4 && text.includes(v))) throw new Error(`refusing to save: ${p} still holds a secret value`);
  }
}

export interface SaveOptions {
  dir: string;
  target: Target;
  dryRun?: boolean;
  jobs?: string[];
  now?: Date;
}

export interface SaveResult {
  dir: string;
  dryRun: boolean;
  committed: boolean;
  diff: string;
  changes: RepoChanges;
  setup: Setup;
  secrets: SecretRef[];
  warnings: Warning[];
}

export async function save(opts: SaveOptions): Promise<SaveResult> {
  const { dir } = opts;
  const meta = existsSync(join(dir, "sac.yaml")) ? readMeta(dir) : null;
  const cap = await capture(opts.target, { jobs: opts.jobs, now: opts.now });
  const setup = { ...cap.setup, name: meta?.name ?? basename(dir), extends: meta?.extends ?? [] };
  const files = setupToRepo(setup, cap.secrets);
  await assertNoSecrets(files, Object.values(cap.values));

  if (meta) {
    const disk = readRepoFiles(dir);
    const same = [...files].every(([p, f]) => p === "sac.yaml" || (disk.get(p)?.data.equals(f.data) && disk.get(p)?.mode === f.mode));
    const noneRemoved = repoChanges(dir, files).removed.length === 0;
    if (same && noneRemoved && disk.has("sac.yaml")) files.set("sac.yaml", { data: readFileSync(join(dir, "sac.yaml")), mode: 0o644 });
  }

  if (opts.dryRun) {
    return { dir, dryRun: true, committed: false, diff: diffRepo(dir, files), changes: repoChanges(dir, files), setup, secrets: cap.secrets, warnings: cap.warnings };
  }
  const changes = writeRepo(dir, files);
  const committed = await commitRepo(dir, `sac save ${(opts.now ?? new Date()).toISOString().slice(0, 16).replace("T", " ")}`);
  return { dir, dryRun: false, committed, diff: "", changes, setup, secrets: cap.secrets, warnings: cap.warnings };
}
