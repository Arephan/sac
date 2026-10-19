import { existsSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { confirm as askConfirm, password } from "@inquirer/prompts";
import { applyPlan, type ApplyResult } from "./apply.js";
import { isText } from "./capture/files.js";
import { claudeCli, type ClaudeCli } from "./claude-cli.js";
import { isGitUrl, loadLayers, mergeLayers } from "./layers.js";
import { changes, describe, learnSecrets, makePlan, readMachine, renderPlan, skips, type Plan } from "./plan.js";
import { placeholdersIn } from "./secrets.js";
import { secretsFrom } from "./secrets-input.js";
import type { Setup, Target } from "./types.js";
import { plural, pretty, type Io } from "./ui.js";

export interface RestoreOptions {
  sources: string[];
  target: Target;
  io: Io;
  /** A person is at the terminal: secrets and confirmation may be asked for. */
  interactive: boolean;
  yes?: boolean;
  dryRun?: boolean;
  envFile?: string;
  allowMissing?: boolean;
  loadJobs?: boolean;
  ask?: (name: string) => Promise<string | undefined>;
  confirm?: () => Promise<boolean>;
  cli?: ClaudeCli;
  now?: Date;
}

export const sacHome = () => process.env.SAC_HOME ?? join(homedir(), ".sac");

/** Every secret name a setup uses, text files included. */
export function secretNames(setup: Setup): string[] {
  const text = setup.files.filter((f) => isText(f.data)).map((f) => f.data.toString("utf8"));
  return placeholdersIn({ text, mcp: setup.mcp, plugins: setup.plugins, jobs: setup.jobs });
}

/** Loads the layers and the secrets for them: environment, env file, then the machine itself. */
export async function prepare(sources: string[], t: Target, cli: ClaudeCli, envFile: string | undefined) {
  const layers = await loadLayers(sources, { cacheDir: join(sacHome(), "cache") });
  const { setup, warnings } = mergeLayers(layers);
  const top = layers[layers.length - 1];
  // A .env inside a repo pulled from a git URL is someone else's file: only a local top layer's counts.
  const file = envFile ?? (isGitUrl(top.source) ? null : join(top.dir, ".env"));
  if (envFile && !existsSync(envFile)) throw new UsageError(`env file ${envFile} does not exist`);
  const machine = await readMachine(setup, t, cli);
  const learned = learnSecrets(setup, t, machine, sacHome());
  const given = secretsFrom(secretNames(setup), { envFile: file });
  const values = { ...learned, ...given };
  return { layers, setup, warnings, machine, values, mask: [...Object.values(learned), ...Object.values(given)] };
}

export class UsageError extends Error {}

const stamp = (d: Date) => d.toISOString().replace(/\.\d{3}Z$/, "Z").replace(/[-:]/g, "");

function exitCode(plan: Plan, applied: ApplyResult | null, allowMissing: boolean): number {
  const results = applied?.results ?? skips(plan).map((step) => ({ step, status: "skipped" as const }));
  if (results.some((r) => r.status === "failed")) return 1;
  const skipped = results.filter((r) => r.status === "skipped");
  if (skipped.some((r) => !r.step.needs?.length)) return 1;
  const secretGaps = skipped.length > 0 || plan.warnings.some((w) => w.kind === "skipped");
  return secretGaps && !allowMissing ? 1 : 0;
}

function report(r: ApplyResult, t: Target): string {
  const lines: string[] = [];
  const done = r.results.filter((x) => x.status === "done").length;
  const failed = r.results.filter((x) => x.status === "failed");
  const skipped = r.results.filter((x) => x.status === "skipped");
  lines.push(`Done: ${plural(done, "change")} applied, ${skipped.length} skipped, ${failed.length} failed.`);
  for (const f of failed) lines.push(`  ! ${describe(f.step)}: ${f.message}`);
  if (r.backedUp.length) lines.push(`Backed up ${plural(r.backedUp.length, "file")} it overwrote.`);
  const parked = t.configDir ? r.results.filter((x) => x.status === "done" && x.step.job?.kind === "launchd") : [];
  if (parked.length) lines.push("", `${plural(parked.length, "launchd job")} written to ${pretty(join(t.configDir!, "sac-jobs"))}; jobs restored into a separate Claude home are never loaded.`);
  if (r.loadCommands.length) {
    lines.push("", "Jobs were written but not loaded. To load them:");
    for (const c of r.loadCommands) lines.push(`  ${c}`);
    lines.push("(or run sac restore again with --load-jobs)");
  }
  return `${lines.join("\n")}\n`;
}

export async function restore(o: RestoreOptions): Promise<number> {
  const { io, target: t } = o;
  if (o.loadJobs && t.configDir) {
    io.err("sac: --load-jobs cannot be used with --config-dir; jobs restored into a separate Claude home are never loaded\n");
    return 2;
  }
  if (!o.dryRun && !o.yes && !o.interactive) {
    io.err("sac: not running in a terminal; pass --yes to apply, or --dry-run to only show the plan\n");
    return 2;
  }
  const cli = o.cli ?? claudeCli(t);
  let prep;
  try {
    prep = await prepare(o.sources, t, cli, o.envFile);
  } catch (e) {
    if (e instanceof UsageError) {
      io.err(`sac: ${e.message}\n`);
      return 2;
    }
    throw e;
  }
  const { layers, setup, machine, values } = prep;
  const planOpts = { sacHome: sacHome(), mask: prep.mask, loadJobs: !!o.loadJobs };
  let plan = await makePlan(setup, t, machine, values, planOpts);
  if (o.interactive && !o.yes && !o.dryRun && plan.missing.length) {
    io.out(`This setup uses ${plural(plan.missing.length, "secret")} that ${plan.missing.length === 1 ? "is" : "are"} not set. Leave one empty to skip what needs it.\n`);
    const ask = o.ask ?? ((name: string) => password({ message: name, mask: "*" }));
    for (const name of plan.missing) {
      const v = await ask(name);
      if (v) values[name] = v;
    }
    plan = await makePlan(setup, t, machine, values, planOpts);
  }
  plan.warnings.unshift(...prep.warnings);

  const where = pretty(t.claudeDir);
  const from = layers.map((l) => l.name).join(" + ");
  if (!changes(plan).length && !skips(plan).length) {
    io.out(`Everything already matches ${from} in ${where}.\n`);
    if (plan.warnings.length) io.out(`Notes\n${plan.warnings.map((w) => `  ${w.message}\n`).join("")}`);
    return exitCode(plan, null, !!o.allowMissing);
  }
  io.out(`${o.dryRun ? "Would restore" : "Restoring"} ${from} into ${where}:\n${renderPlan(plan)}`);
  if (o.dryRun) return 0;
  if (!changes(plan).length) {
    io.out("Nothing to apply.\n");
    return exitCode(plan, null, !!o.allowMissing);
  }
  if (!o.yes) {
    const ok = await (o.confirm ?? (() => askConfirm({ message: "Apply these changes?", default: false })))();
    if (!ok) {
      io.out("Nothing changed.\n");
      return 1;
    }
  }
  mkdirSync(t.claudeDir, { recursive: true });
  const applied = await applyPlan(plan, t, cli, {
    backupDir: join(sacHome(), "backups", stamp(o.now ?? new Date())),
    loadJobs: !!o.loadJobs,
    values: { ...values, ...Object.fromEntries(prep.mask.map((v, i) => [`#${i}`, v])) },
  });
  io.out(report(applied, t));
  return exitCode(plan, applied, !!o.allowMissing);
}
