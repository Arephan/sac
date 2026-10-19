import { existsSync, readFileSync, statSync } from "node:fs";
import { isAbsolute, join, posix } from "node:path";
import { createTwoFilesPatch } from "diff";
import { build } from "plist";
import { isText } from "./capture/files.js";
import { cronLine, plistFor, readPlistFile } from "./capture/jobs.js";
import { readUserMcp } from "./capture/mcp.js";
import { listMarketplaces, listPlugins, pluginOptions, type ClaudeCli, type InstalledPlugin, type KnownMarketplace } from "./claude-cli.js";
import { mergeSettings } from "./layers.js";
import { deepMapStrings, detokenize } from "./paths.js";
import { extractSecrets, fillPlaceholders, fillText, findSecretSpans, placeholdersIn, scrub } from "./secrets.js";
import type { JobDef, McpServer, Setup, Target, Warning } from "./types.js";

export type StepKind = "file" | "settings" | "marketplace" | "plugin" | "plugin-settings" | "mcp" | "job";
export type StepOp = "add" | "update" | "unchanged" | "install" | "enable" | "disable" | "set";

export interface Step {
  kind: StepKind;
  key: string;
  op: StepOp;
  /** Why the step will not run; the op is what it would have done. */
  skip?: string;
  /** Secret names whose absence caused the skip (only those). */
  needs?: string[];
  diff?: string;
  /** Absolute file the step writes (files, settings, jobs). */
  path?: string;
  data?: Buffer;
  mode?: number;
  /** Whether `path` exists now (it is backed up before it is overwritten). */
  exists?: boolean;
  source?: string;
  enabled?: boolean;
  values?: Record<string, string>;
  config?: McpServer;
  job?: JobDef;
}

export interface Plan {
  steps: Step[];
  warnings: Warning[];
  /** Secrets referenced by the setup that no source supplied. */
  missing: string[];
}

/** What the target machine has now, read once and shared by secret learning, planning and doctor. */
export interface Machine {
  settingsText: string | null;
  mcp: Record<string, McpServer>;
  marketplaces: KnownMarketplace[];
  plugins: InstalledPlugin[];
  pluginInputs: Record<string, Record<string, string>>;
}

export const sha = (v: unknown): string => JSON.stringify(sorted(v));
const sorted = (v: unknown): unknown => {
  if (Array.isArray(v)) return v.map(sorted);
  if (v && typeof v === "object" && !Buffer.isBuffer(v)) return Object.fromEntries(Object.keys(v).sort().map((k) => [k, sorted((v as Record<string, unknown>)[k])]));
  return v;
};
const same = (a: unknown, b: unknown) => sha(a) === sha(b);

const inputString = (v: unknown) => (typeof v === "string" ? v : JSON.stringify(v));

export async function readMachine(setup: Setup, t: Target, cli: ClaudeCli): Promise<Machine> {
  const sp = join(t.claudeDir, "settings.json");
  const plugins = (await listPlugins(cli)).filter((p) => p.scope === "user");
  const pluginInputs: Record<string, Record<string, string>> = {};
  for (const p of setup.plugins) {
    if (!Object.keys(p.settings).length || !plugins.some((x) => x.id === p.id)) continue;
    const o = await pluginOptions(cli, p.id);
    pluginInputs[p.id] = Object.fromEntries(Object.entries(o.inputs).filter(([, v]) => v !== undefined && v !== null).map(([k, v]) => [k, inputString(v)]));
  }
  return {
    settingsText: existsSync(sp) ? readFileSync(sp, "utf8") : null,
    mcp: readUserMcp(t),
    marketplaces: await listMarketplaces(cli),
    plugins,
    pluginInputs,
  };
}

const parseJson = (s: string | null): Record<string, unknown> | null => {
  if (s === null) return null;
  try {
    const v = JSON.parse(s);
    return v && typeof v === "object" && !Array.isArray(v) ? v : null;
  } catch {
    return null;
  }
};

const tokens = <T>(v: T, t: Target): T => deepMapStrings(v, (s) => detokenize(s, t));

/** A path inside the Claude folder that cannot climb out of it. */
const safeRel = (p: string) => !isAbsolute(p) && !posix.normalize(p).startsWith("..") && !p.split("/").includes("..");

export function jobPath(job: JobDef, t: Target, sacHome: string): string {
  const file = job.label.replace(/[/\\]/g, "_");
  if (job.kind === "cron") return join(sacHome, "jobs", `${file}.cron`);
  return t.configDir ? join(t.configDir, "sac-jobs", `${file}.plist`) : join(t.home, "Library", "LaunchAgents", `${file}.plist`);
}

const readPlist = (p: string): Record<string, unknown> | null => {
  try {
    return existsSync(p) ? readPlistFile(p) : null;
  } catch {
    return null;
  }
};

/** Secret values the machine already holds, read from the same places the setup puts them. */
export function learnSecrets(setup: Setup, t: Target, m: Machine, sacHome: string): Record<string, string> {
  const found: Record<string, string> = {};
  const take = (o: Record<string, string>) => {
    for (const [k, v] of Object.entries(o)) found[k] ??= v;
  };
  for (const f of setup.files) {
    if (f.path === "settings.json" || !isText(f.data) || !safeRel(f.path)) continue;
    const text = f.data.toString("utf8");
    if (!placeholdersIn(text).length) continue;
    const p = join(t.claudeDir, f.path);
    if (existsSync(p)) take(extractSecrets(detokenize(text, t), readFileSync(p, "utf8")));
  }
  const settings = setup.files.find((f) => f.path === "settings.json");
  const current = parseJson(m.settingsText);
  if (settings && current) {
    const desired = parseJson(settings.data.toString("utf8"));
    if (desired) take(extractSecrets(tokens(desired, t), current));
  }
  for (const [name, s] of Object.entries(setup.mcp)) if (m.mcp[name]) take(extractSecrets(tokens(s, t), m.mcp[name]));
  for (const p of setup.plugins) if (m.pluginInputs[p.id]) take(extractSecrets(p.settings, m.pluginInputs[p.id]));
  for (const j of setup.jobs) {
    if (j.kind !== "launchd") continue;
    const cur = readPlist(jobPath(j, t, sacHome));
    if (cur) take(extractSecrets(plistFor(tokens(j, t)), cur));
  }
  return found;
}

/** Text safe to show: known secret values become ***, and anything that looks like a credential is masked. */
export async function safeText(text: string, values: Record<string, string>): Promise<string> {
  const s = scrub(text, Object.values(values));
  const spans = await findSecretSpans(s);
  let out = s;
  for (const [a, b] of spans.reverse()) out = `${out.slice(0, a)}***${out.slice(b)}`;
  return out;
}

const needs = (names: string[]) => `needs secret ${names.join(", ")}`;

export async function makePlan(setup: Setup, t: Target, m: Machine, values: Record<string, string>, opts: { sacHome: string }): Promise<Plan> {
  const steps: Step[] = [];
  const warnings: Warning[] = [];
  const missing = new Set<string>();
  const skipSecret = (s: Step, names: string[]) => {
    names.forEach((n) => missing.add(n));
    return { ...s, skip: needs(names), needs: names };
  };
  const diffOf = async (name: string, a: string, b: string) => safeText(createTwoFilesPatch(`a/${name}`, `b/${name}`, a, b, "", "", { context: 2 }).split("\n").slice(2).join("\n"), values);

  // Files other than settings.json.
  for (const f of setup.files) {
    if (f.path === "settings.json") continue;
    const path = join(t.claudeDir, f.path);
    const base: Step = { kind: "file", key: f.path, op: "add", path, mode: f.mode };
    if (!safeRel(f.path)) {
      steps.push({ ...base, skip: "path leaves the Claude folder" });
      continue;
    }
    let data = f.data;
    if (isText(f.data)) {
      const lost = new Set<string>();
      const text = fillText(detokenize(f.data.toString("utf8"), t), values, lost);
      if (lost.size) {
        steps.push(skipSecret(base, [...lost].sort()));
        continue;
      }
      data = Buffer.from(text);
    }
    const exists = existsSync(path);
    if (!exists) {
      steps.push({ ...base, data, exists });
      continue;
    }
    const cur = readFileSync(path);
    const curMode = statSync(path).mode & 0o777;
    if (cur.equals(data) && curMode === f.mode) {
      steps.push({ ...base, op: "unchanged", data, exists });
      continue;
    }
    const diff = isText(cur) && isText(data) && !cur.equals(data) ? await diffOf(f.path, cur.toString("utf8"), data.toString("utf8")) : undefined;
    steps.push({ ...base, op: "update", data, exists, diff });
  }

  // settings.json: merged into what the machine has.
  const sf = setup.files.find((f) => f.path === "settings.json");
  if (sf) {
    const path = join(t.claudeDir, "settings.json");
    const base: Step = { kind: "settings", key: "settings.json", op: "add", path, mode: 0o644 };
    const parsed = parseJson(sf.data.toString("utf8"));
    const current = parseJson(m.settingsText);
    if (!parsed) steps.push({ ...base, skip: "the saved settings.json is not valid JSON" });
    else if (m.settingsText !== null && !current) steps.push({ ...base, op: "update", exists: true, skip: "settings.json on this machine is not valid JSON; fix it first" });
    else {
      const desired = tokens(parsed, t);
      const env = desired.env && typeof desired.env === "object" ? { ...(desired.env as Record<string, unknown>) } : null;
      if (env) {
        for (const [k, v] of Object.entries(env)) {
          const lost = placeholdersIn(v).filter((n) => !Object.hasOwn(values, n));
          if (lost.length) {
            lost.forEach((n) => missing.add(n));
            delete env[k];
            warnings.push({ kind: "skipped", message: `settings.json: env ${k} left out, ${needs(lost)}` });
          }
        }
        desired.env = env;
      }
      const { value, missing: lost } = fillPlaceholders(desired, values);
      if (lost.length) steps.push(skipSecret({ ...base, exists: current !== null }, lost));
      else {
        const merged = mergeSettings(current ?? {}, value);
        const text = `${JSON.stringify(merged, null, 2)}\n`;
        const data = Buffer.from(text);
        if (current && same(current, merged)) steps.push({ ...base, op: "unchanged", data, exists: true });
        else if (!current) steps.push({ ...base, data, exists: false });
        else steps.push({ ...base, op: "update", data, exists: true, diff: await diffOf("settings.json", `${JSON.stringify(current, null, 2)}\n`, text) });
      }
    }
  }

  // Marketplaces.
  const adding = new Set<string>();
  for (const mk of setup.marketplaces) {
    const source = detokenize(mk.source, t);
    const base: Step = { kind: "marketplace", key: mk.name, op: "add", source };
    if (m.marketplaces.some((x) => x.name === mk.name)) steps.push({ ...base, op: "unchanged" });
    else if (mk.kind === "directory" && !existsSync(source)) steps.push({ ...base, skip: `folder ${source} is not on this machine` });
    else {
      adding.add(mk.name);
      steps.push(base);
    }
  }

  // Plugins and their settings.
  for (const p of setup.plugins) {
    const mk = p.id.split("@")[1] ?? "";
    const have = m.plugins.find((x) => x.id === p.id);
    const op: StepOp = !have ? "install" : have.enabled === p.enabled ? "unchanged" : p.enabled ? "enable" : "disable";
    const base: Step = { kind: "plugin", key: p.id, op, enabled: p.enabled };
    if (!have && !adding.has(mk) && !m.marketplaces.some((x) => x.name === mk)) steps.push({ ...base, skip: `marketplace ${mk || "(none)"} is not available` });
    else steps.push(base);

    if (!Object.keys(p.settings).length) continue;
    const { value, missing: lost } = fillPlaceholders(tokens(p.settings, t), values);
    const sb: Step = { kind: "plugin-settings", key: p.id, op: "set", values: value };
    const pluginSkip = steps[steps.length - 1].skip;
    if (pluginSkip) steps.push({ ...sb, skip: `plugin ${p.id} is not installed` });
    else if (lost.length) steps.push(skipSecret(sb, lost));
    else {
      const cur = m.pluginInputs[p.id] ?? {};
      steps.push(Object.entries(value).every(([k, v]) => cur[k] === v) ? { ...sb, op: "unchanged" } : sb);
    }
  }

  // MCP servers.
  for (const [name, s] of Object.entries(setup.mcp)) {
    const { value, missing: lost } = fillPlaceholders(tokens(s, t), values);
    const cur = m.mcp[name];
    const base: Step = { kind: "mcp", key: name, op: !cur ? "add" : same(cur, value) ? "unchanged" : "update", config: value };
    steps.push(lost.length ? skipSecret(base, lost) : base);
  }

  // Jobs.
  for (const j of setup.jobs) {
    const path = jobPath(j, t, opts.sacHome);
    const { value: job, missing: lost } = fillPlaceholders(tokens(j, t), values);
    const base: Step = { kind: "job", key: j.label, op: "add", path, job, mode: 0o644 };
    if (j.kind === "launchd" && t.platform !== "darwin") {
      steps.push({ ...base, skip: "launchd jobs run only on macOS" });
      continue;
    }
    const gone = job.dependencies.filter((d) => !existsSync(d));
    if (gone.length) {
      steps.push({ ...base, skip: `needs ${gone.join(", ")}, which is not on this machine` });
      continue;
    }
    if (lost.length) {
      steps.push(skipSecret(base, lost));
      continue;
    }
    const exists = existsSync(path);
    if (j.kind === "cron") {
      const data = Buffer.from(`${cronLine(job)}\n`);
      const op = !exists ? "add" : readFileSync(path).equals(data) ? "unchanged" : "update";
      steps.push({ ...base, op, data, exists });
      continue;
    }
    const plist = plistFor(job);
    const data = Buffer.from(build(plist));
    const cur = readPlist(path);
    steps.push({ ...base, op: !exists ? "add" : cur && same(cur, plist) ? "unchanged" : "update", data, exists });
  }

  return { steps, warnings, missing: [...missing].sort() };
}

const KIND_TITLE: Record<StepKind, string> = {
  file: "Files",
  settings: "Settings",
  marketplace: "Marketplaces",
  plugin: "Plugins",
  "plugin-settings": "Plugin settings",
  mcp: "MCP servers",
  job: "Jobs",
};

const KIND_NAME: Record<StepKind, string> = {
  file: "file",
  settings: "settings",
  marketplace: "marketplace",
  plugin: "plugin",
  "plugin-settings": "plugin settings for",
  mcp: "MCP server",
  job: "job",
};

const MARK: Record<StepOp, string> = { add: "+", install: "+", update: "~", set: "~", enable: "~", disable: "~", unchanged: "=" };
const VERB: Partial<Record<StepOp, string>> = { install: "install", enable: "enable", disable: "disable", set: "set" };

export const changes = (p: Plan) => p.steps.filter((s) => !s.skip && s.op !== "unchanged");
export const skips = (p: Plan) => p.steps.filter((s) => s.skip);

export const describe = (s: Step) => `${KIND_NAME[s.kind]} ${s.key}`;

/** The plan for people: changes grouped by kind, diffs, skips with reasons, a count of what already matches. */
export function renderPlan(plan: Plan): string {
  const lines: string[] = [];
  for (const kind of Object.keys(KIND_TITLE) as StepKind[]) {
    const group = plan.steps.filter((s) => s.kind === kind && !s.skip && s.op !== "unchanged");
    if (!group.length) continue;
    lines.push(KIND_TITLE[kind]);
    for (const s of group) {
      const verb = VERB[s.op];
      lines.push(`  ${MARK[s.op]} ${verb && kind !== "plugin-settings" ? `${verb} ` : ""}${s.key}${kind === "plugin-settings" ? ` (${Object.keys(s.values ?? {}).join(", ")})` : ""}`);
      if (s.diff) lines.push(...s.diff.trimEnd().split("\n").map((l) => `      ${l}`));
    }
  }
  const skipped = skips(plan);
  if (skipped.length) {
    lines.push("Skipped");
    for (const s of skipped) lines.push(`  - ${describe(s)}: ${s.skip}`);
  }
  if (plan.warnings.length) {
    lines.push("Notes");
    for (const w of plan.warnings) lines.push(`  ${w.message}`);
  }
  const same = plan.steps.filter((s) => !s.skip && s.op === "unchanged").length;
  if (same) lines.push(`${same} already match${same === 1 ? "es" : ""}.`);
  return lines.length ? `${lines.join("\n")}\n` : "";
}
