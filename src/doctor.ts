import { existsSync } from "node:fs";
import { join } from "node:path";
import fg from "fast-glob";
import { DIR_ITEMS, IGNORE } from "./capture/files.js";
import { claudeVersion, tool, type ClaudeCli } from "./claude-cli.js";
import { describe, makePlan, type Machine } from "./plan.js";
import type { Setup, Target } from "./types.js";

export type Level = "missing" | "differs" | "failing" | "warn" | "info";

export interface Finding {
  level: Level;
  message: string;
}

export interface DoctorResult {
  findings: Finding[];
  ok: boolean;
}

export interface DoctorOptions {
  values: Record<string, string>;
  sacHome: string;
  /** Claude Code version the setup was captured with. */
  capturedClaude?: string | null;
  launchctl?: ClaudeCli;
  uid?: number;
}

/** Status of each server in `claude mcp list` output: "name: target - status". */
export function parseMcpList(stdout: string): Record<string, "ok" | "failing" | "auth"> {
  const out: Record<string, "ok" | "failing" | "auth"> = {};
  for (const line of stdout.split("\n")) {
    const m = /^([^:\s][^:]*):\s.*\s-\s(.+)$/.exec(line.trim());
    if (!m) continue;
    const status = m[2];
    out[m[1]] = /needs authentication/i.test(status) ? "auth" : /✗|failed/i.test(status) ? "failing" : "ok";
  }
  return out;
}

const older = (a: string, b: string) => {
  const x = a.split(".").map(Number);
  const y = b.split(".").map(Number);
  for (let i = 0; i < 3; i++) if ((x[i] ?? 0) !== (y[i] ?? 0)) return (x[i] ?? 0) < (y[i] ?? 0);
  return false;
};

/** Compares the machine with the setup. `ok` is false when anything is missing, differs or fails. */
export async function doctor(setup: Setup, t: Target, cli: ClaudeCli, m: Machine, opts: DoctorOptions): Promise<DoctorResult> {
  const findings: Finding[] = [];
  const add = (level: Level, message: string) => findings.push({ level, message });
  const plan = await makePlan(setup, t, m, opts.values, { sacHome: opts.sacHome });

  for (const s of plan.steps) {
    if (s.skip) {
      add("missing", `${describe(s)}: ${s.skip}`);
      continue;
    }
    if (s.op === "unchanged") continue;
    if (s.op === "add" || s.op === "install") add("missing", `${describe(s)} is not on this machine`);
    else if (s.op === "enable" || s.op === "disable") add("differs", `plugin ${s.key} should be ${s.op}d`);
    else add("differs", `${describe(s)} differs from the setup`);
  }
  for (const w of plan.warnings) add("missing", w.message);
  for (const n of plan.missing) add("missing", `secret ${n} is not set (environment, env file, or on this machine)`);

  const names = Object.keys(setup.mcp);
  if (names.length) {
    const r = await cli.run(["mcp", "list"]);
    const status = r.code === 0 ? parseMcpList(r.stdout) : {};
    for (const n of names) {
      if (!m.mcp[n]) continue;
      if (status[n] === "failing") add("failing", `MCP server ${n} fails to connect`);
      else if (status[n] === "auth") add("warn", `MCP server ${n} needs authentication (run /mcp in Claude Code)`);
    }
  }

  if (!t.configDir && t.platform === "darwin") {
    const launchctl = opts.launchctl ?? tool(t, process.env.SAC_LAUNCHCTL_BIN ?? "launchctl", 30_000);
    const uid = opts.uid ?? process.getuid?.() ?? 0;
    for (const j of setup.jobs) {
      if (j.kind !== "launchd" || !plan.steps.some((s) => s.kind === "job" && s.key === j.label && s.op === "unchanged" && !s.skip)) continue;
      if ((await launchctl.run(["print", `gui/${uid}/${j.label}`])).code !== 0) add("warn", `job ${j.label} is written but not loaded`);
    }
  }

  for (const n of Object.keys(m.mcp)) if (!setup.mcp[n]) add("info", `MCP server ${n} is on this machine but in no layer`);
  for (const p of m.plugins) if (!setup.plugins.some((x) => x.id === p.id)) add("info", `plugin ${p.id} is on this machine but in no layer`);
  const known = new Set(setup.files.map((f) => f.path));
  for (const dir of DIR_ITEMS) {
    if (!existsSync(join(t.claudeDir, dir))) continue;
    for (const p of await fg("**/*", { cwd: join(t.claudeDir, dir), dot: true, onlyFiles: true, ignore: IGNORE })) {
      if (!known.has(`${dir}/${p}`)) add("info", `${dir}/${p} is on this machine but in no layer`);
    }
  }

  if (opts.capturedClaude) {
    const v = await claudeVersion(cli);
    if (v && older(v, opts.capturedClaude)) add("warn", `Claude Code ${v} is older than ${opts.capturedClaude}, which this setup was saved with`);
  }

  return { findings, ok: !findings.some((f) => f.level === "missing" || f.level === "differs" || f.level === "failing") };
}

const MARK: Record<Level, string> = { missing: "✗", differs: "~", failing: "✗", warn: "!", info: "·" };

export function renderDoctor(r: DoctorResult, where: string): string {
  if (!r.findings.length) return `All good: ${where} matches the setup.\n`;
  const order: Level[] = ["failing", "missing", "differs", "warn", "info"];
  const lines = [...r.findings].sort((a, b) => order.indexOf(a.level) - order.indexOf(b.level)).map((f) => `  ${MARK[f.level]} ${f.message}`);
  const head = r.ok ? `${where} matches the setup, with notes:` : `${where} does not match the setup:`;
  return `${head}\n${lines.join("\n")}\n`;
}
