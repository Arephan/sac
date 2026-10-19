import { chmodSync, copyFileSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import { tool, type ClaudeCli } from "./claude-cli.js";
import type { Plan, Step } from "./plan.js";
import { scrub } from "./secrets.js";
import type { Target } from "./types.js";

export interface StepResult {
  step: Step;
  status: "done" | "failed" | "skipped";
  message?: string;
}

export interface ApplyOptions {
  backupDir: string;
  loadJobs: boolean;
  /** Secret values, hidden from any error text that is reported. */
  values: Record<string, string>;
  launchctl?: ClaudeCli;
  crontab?: ClaudeCli;
  uid?: number;
}

export interface ApplyResult {
  results: StepResult[];
  backedUp: string[];
  /** Jobs written but not loaded: the commands that would load them. */
  loadCommands: string[];
}

const firstLine = (s: string) => s.trim().split("\n")[0] ?? "";

/** Where a file's backup goes: its path under the home folder, or its absolute path, below `dir`. */
export function backupPath(file: string, t: Target, dir: string): string {
  const r = relative(t.home, file);
  return join(dir, !r.startsWith("..") && !r.startsWith(sep) ? r : file.replace(/^[/\\]+/, ""));
}

function write(path: string, data: Buffer, mode: number) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, data);
  chmodSync(path, mode);
}

const shellQuote = (s: string) => (/^[A-Za-z0-9_./:@%+=-]+$/.test(s) ? s : `'${s.replace(/'/g, `'\\''`)}'`);

/** Runs the plan in order: backups, files, settings, marketplaces, plugins, plugin settings, MCP servers, jobs. */
export async function applyPlan(plan: Plan, t: Target, cli: ClaudeCli, opts: ApplyOptions): Promise<ApplyResult> {
  const results: StepResult[] = [];
  const backedUp: string[] = [];
  const loadCommands: string[] = [];
  const hide = (s: string) => scrub(s, Object.values(opts.values));
  const todo = plan.steps.filter((s) => s.op !== "unchanged" || s.skip);
  const failed = new Set<string>();
  const launchctl = opts.launchctl ?? tool(t, process.env.SAC_LAUNCHCTL_BIN ?? "launchctl", 60_000);
  const crontab = opts.crontab ?? tool(t, process.env.SAC_CRONTAB_BIN ?? "crontab", 30_000);
  const uid = opts.uid ?? process.getuid?.() ?? 0;

  const run = async (step: Step, args: string[], input?: string): Promise<boolean> => {
    const r = await cli.run(args, input);
    if (r.code === 0) return true;
    results.push({ step, status: "failed", message: hide(`claude ${args.slice(0, 3).join(" ")} failed: ${firstLine(r.stderr || r.stdout) || `exit ${r.code}`}`) });
    failed.add(`${step.kind}:${step.key}`);
    return false;
  };
  const done = (step: Step) => results.push({ step, status: "done" });

  for (const s of todo) if (s.skip) results.push({ step: s, status: "skipped", message: s.skip });
  const live = todo.filter((s) => !s.skip);

  for (const s of live) {
    if (!s.path || !s.exists || !["file", "settings", "job"].includes(s.kind)) continue;
    const dest = backupPath(s.path, t, opts.backupDir);
    mkdirSync(dirname(dest), { recursive: true });
    copyFileSync(s.path, dest);
    backedUp.push(s.path);
  }

  for (const s of live.filter((x) => x.kind === "file" || x.kind === "settings")) {
    try {
      write(s.path!, s.data!, s.mode ?? 0o644);
      done(s);
    } catch (e) {
      results.push({ step: s, status: "failed", message: hide((e as Error).message) });
    }
  }

  for (const s of live.filter((x) => x.kind === "marketplace")) {
    if (await run(s, ["plugin", "marketplace", "add", s.source!])) done(s);
  }

  for (const s of live.filter((x) => x.kind === "plugin")) {
    const mk = s.key.split("@")[1] ?? "";
    if (failed.has(`marketplace:${mk}`)) {
      results.push({ step: s, status: "skipped", message: `marketplace ${mk} could not be added` });
      failed.add(`plugin:${s.key}`);
      continue;
    }
    if (s.op === "install") {
      if (!(await run(s, ["plugin", "install", s.key, "--scope", "user"]))) continue;
      if (!s.enabled && !(await run(s, ["plugin", "disable", s.key, "--scope", "user"]))) continue;
    } else if (!(await run(s, ["plugin", s.op, s.key, "--scope", "user"]))) continue;
    done(s);
  }

  for (const s of live.filter((x) => x.kind === "plugin-settings")) {
    if (failed.has(`plugin:${s.key}`)) {
      results.push({ step: s, status: "skipped", message: `plugin ${s.key} is not installed` });
      continue;
    }
    if (await run(s, ["plugin", "configure", s.key, "--values-stdin"], JSON.stringify(s.values))) done(s);
  }

  for (const s of live.filter((x) => x.kind === "mcp")) {
    if (s.op === "update" && !(await run(s, ["mcp", "remove", s.key, "--scope", "user"]))) continue;
    if (await run(s, ["mcp", "add-json", "--scope", "user", s.key, JSON.stringify(s.config)])) done(s);
  }

  for (const s of live.filter((x) => x.kind === "job")) {
    try {
      write(s.path!, s.data!, s.mode ?? 0o644);
    } catch (e) {
      results.push({ step: s, status: "failed", message: hide((e as Error).message) });
      continue;
    }
    const job = s.job!;
    if (job.kind === "launchd" && t.configDir) {
      done(s);
      continue;
    }
    if (job.kind === "launchd") {
      if (!opts.loadJobs) {
        loadCommands.push(`launchctl bootstrap gui/${uid} ${shellQuote(s.path!)}`);
        done(s);
        continue;
      }
      if (s.op === "update") await launchctl.run(["bootout", `gui/${uid}/${job.label}`]);
      const r = await launchctl.run(["bootstrap", `gui/${uid}`, s.path!]);
      if (r.code === 0) done(s);
      else results.push({ step: s, status: "failed", message: hide(`launchctl bootstrap failed: ${firstLine(r.stderr) || `exit ${r.code}`}`) });
      continue;
    }
    const line = s.data!.toString("utf8").trimEnd();
    if (!opts.loadJobs) {
      loadCommands.push(`(crontab -l 2>/dev/null; cat ${shellQuote(s.path!)}) | crontab -`);
      done(s);
      continue;
    }
    const cur = await crontab.run(["-l"]);
    const table = cur.code === 0 ? cur.stdout : "";
    if (table.split("\n").includes(line)) {
      done(s);
      continue;
    }
    const r = await crontab.run(["-"], `${table}${table && !table.endsWith("\n") ? "\n" : ""}${line}\n`);
    if (r.code === 0) done(s);
    else results.push({ step: s, status: "failed", message: hide(`crontab failed: ${firstLine(r.stderr) || `exit ${r.code}`}`) });
  }

  return { results, backedUp, loadCommands };
}
