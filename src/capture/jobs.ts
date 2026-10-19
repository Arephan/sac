import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { isAbsolute, join, relative, resolve, sep } from "node:path";
import { parse, parseBinary, type PlistValue } from "plist";
import { tool } from "../claude-cli.js";
import { expandTilde } from "../paths.js";
import type { JobDef, Target, Warning } from "../types.js";
import { isText } from "./files.js";

/** `claude` (or `claude-code`) as a command word or a path's last part, not inside another name. */
export const CLAUDE_CMD = /(^|[\s;&|('"`/=])claude(-code)?(?=$|[\s;&|)'"`])/;

export type ReadText = (path: string) => string | null;

export const SCHEDULE_KEYS = ["StartCalendarInterval", "StartInterval", "KeepAlive", "RunAtLoad", "WatchPaths", "QueueDirectories", "StartOnMount"];
const MODELLED = new Set(["Label", "Program", "ProgramArguments", "EnvironmentVariables", "WorkingDirectory", "StandardOutPath", "StandardErrorPath", ...SCHEDULE_KEYS]);

/** Reads a small text file, or null. */
export const readSmallText: ReadText = (p) => {
  try {
    if (!existsSync(p) || !statSync(p).isFile() || statSync(p).size > 256 * 1024) return null;
    const data = readFileSync(p);
    return isText(data) ? data.toString("utf8") : null;
  } catch {
    return null;
  }
};

const words = (s: string) => s.split(/\s+|&&|\|\||;|\|/).map((w) => w.replace(/^["']|["']$/g, "")).filter(Boolean);

/** True when an argument runs claude, or names a script with a non-comment line that does. */
export function runsClaude(args: string[], readText: ReadText): boolean {
  if (args.some((a) => CLAUDE_CMD.test(a))) return true;
  return args.some((a) => {
    if (!isAbsolute(a)) return false;
    const text = readText(a);
    return text !== null && text.split("\n").some((l) => !/^\s*(#|\/\/)/.test(l) && CLAUDE_CMD.test(l));
  });
}

/** Files under the home folder (but outside the Claude folder) that a job's command uses. */
function dependencies(args: string[], home: string, claudeDir: string): string[] {
  const under = (p: string, dir: string) => {
    const r = relative(dir, p);
    return r !== "" && !r.startsWith("..") && !r.startsWith(sep);
  };
  const out = new Set<string>();
  for (const a of args) {
    if (!isAbsolute(a) || !under(a, home) || under(a, claudeDir)) continue;
    try {
      if (statSync(a).isFile()) out.add(a);
    } catch {
      // not a file on this machine
    }
  }
  return [...out];
}

const slug = (cmd: string) =>
  words(cmd).slice(0, 4).join("-").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40).replace(/-+$/, "");

export function parseCrontab(text: string, readText: ReadText, home: string, claudeDir: string): JobDef[] {
  const jobs: JobDef[] = [];
  const used = new Set<string>();
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (!line || line.startsWith("#") || /^[A-Za-z_][A-Za-z0-9_]*\s*=/.test(line)) continue;
    const m = /^(@\w+)\s+(.+)$/.exec(line) ?? /^(\S+\s+\S+\s+\S+\s+\S+\s+\S+)\s+(.+)$/.exec(line);
    if (!m) continue;
    const [, cron, command] = m;
    const args = words(command).map((w) => expandTilde(w, home));
    if (!CLAUDE_CMD.test(command) && !runsClaude(args, readText)) continue;
    const base = `cron-${slug(command) || "job"}`;
    let label = base;
    for (let i = 2; used.has(label); i++) label = `${base}-${i}`;
    used.add(label);
    jobs.push({ label, kind: "cron", program: [command], schedule: { cron }, env: {}, dependencies: dependencies(args, home, claudeDir) });
  }
  return jobs;
}

export function cronLine(job: JobDef): string {
  return `${String(job.schedule.cron)} ${job.program.join(" ")}`;
}

function fromPlist(p: Record<string, unknown>, t: Target, readText: ReadText): { job: JobDef; runs: boolean } {
  const program = Array.isArray(p.ProgramArguments) ? (p.ProgramArguments as unknown[]).map(String) : typeof p.Program === "string" ? [p.Program] : [];
  const wd = typeof p.WorkingDirectory === "string" ? p.WorkingDirectory : undefined;
  const resolved = program.map((a) => expandTilde(a, t.home)).map((a) => (wd && !isAbsolute(a) && /[/.]/.test(a) ? resolve(wd, a) : a));
  const schedule: Record<string, unknown> = {};
  const extra: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(p)) {
    if (SCHEDULE_KEYS.includes(k)) schedule[k] = v;
    else if (!MODELLED.has(k)) extra[k] = v;
  }
  const job: JobDef = {
    label: String(p.Label),
    kind: "launchd",
    program,
    schedule,
    ...(wd !== undefined ? { workingDirectory: wd } : {}),
    env: Object.fromEntries(Object.entries((p.EnvironmentVariables as Record<string, unknown>) ?? {}).map(([k, v]) => [k, String(v)])),
    ...(typeof p.StandardOutPath === "string" ? { stdout: p.StandardOutPath } : {}),
    ...(typeof p.StandardErrorPath === "string" ? { stderr: p.StandardErrorPath } : {}),
    dependencies: dependencies(resolved, t.home, t.claudeDir),
    ...(Object.keys(extra).length ? { extra } : {}),
  };
  return { job, runs: runsClaude(resolved, readText) };
}

/** The launchd property list for a job (inverse of how it was read). */
export function plistFor(job: JobDef): Record<string, PlistValue> {
  const p: Record<string, unknown> = { Label: job.label, ProgramArguments: job.program };
  if (Object.keys(job.env).length) p.EnvironmentVariables = job.env;
  if (job.workingDirectory !== undefined) p.WorkingDirectory = job.workingDirectory;
  if (job.stdout !== undefined) p.StandardOutPath = job.stdout;
  if (job.stderr !== undefined) p.StandardErrorPath = job.stderr;
  return { ...p, ...job.schedule, ...(job.extra ?? {}) } as Record<string, PlistValue>;
}

export function readPlistFile(path: string): Record<string, unknown> {
  const data = readFileSync(path);
  const v = data.subarray(0, 8).toString("latin1") === "bplist00" ? parseBinary(data) : parse(data.toString("utf8"));
  if (!v || typeof v !== "object" || Array.isArray(v)) throw new Error("not a dictionary");
  return v as Record<string, unknown>;
}

export async function readJobs(t: Target, opts: { labels?: string[]; readText?: ReadText } = {}): Promise<{ jobs: JobDef[]; warnings: Warning[] }> {
  const readText = opts.readText ?? readSmallText;
  const labels = new Set(opts.labels ?? []);
  const jobs: JobDef[] = [];
  const warnings: Warning[] = [];

  const agents = join(t.home, "Library", "LaunchAgents");
  if (t.platform === "darwin" && existsSync(agents)) {
    for (const name of readdirSync(agents).filter((n) => n.endsWith(".plist")).sort()) {
      const path = join(agents, name);
      try {
        const p = readPlistFile(path);
        if (typeof p.Label !== "string") throw new Error("no Label");
        const { job, runs } = fromPlist(p, t, readText);
        if (runs || labels.has(job.label)) jobs.push(job);
      } catch {
        warnings.push({ kind: "other", message: `${path} could not be read; skipped` });
      }
    }
  }

  const crontab = await tool(t, process.env.SAC_CRONTAB_BIN ?? "crontab", 30_000).run(["-l"]);
  if (crontab.code === 0) jobs.push(...parseCrontab(crontab.stdout, readText, t.home, t.claudeDir));

  for (const l of labels) if (!jobs.some((j) => j.label === l)) warnings.push({ kind: "other", message: `no job labelled ${l} was found` });
  return { jobs, warnings };
}
