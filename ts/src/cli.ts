import { Command, CommanderError } from "commander";
import { isGitUrl } from "./layers.js";
import { targetFor } from "./paths.js";
import { restore } from "./restore.js";
import { save, type SaveResult } from "./save.js";
import { absolute, plural, pretty, stdio, table, type Io } from "./ui.js";

export const DEFAULT_REPO = "~/my-claude";

function saveReport(r: SaveResult): string {
  const s = r.setup;
  const lines: string[] = [];
  const what = [plural(s.files.length, "file"), plural(Object.keys(s.mcp).length, "MCP server"), plural(s.marketplaces.length, "marketplace"), plural(s.plugins.length, "plugin"), plural(s.jobs.length, "job")].join(", ");
  const changed = r.changes.added.length + r.changes.changed.length + r.changes.removed.length;
  if (r.dryRun && !changed) return `Nothing to save: ${pretty(r.dir)} already matches this machine.\n`;
  if (r.dryRun) lines.push(`Would save to ${pretty(r.dir)}: ${what}.`);
  else if (r.committed) lines.push(`Saved to ${pretty(r.dir)} and committed: ${what}.`);
  else return `No changes since the last save in ${pretty(r.dir)}.\n`;
  if (changed) lines.push(`  ${r.changes.added.length} added, ${r.changes.changed.length} changed, ${r.changes.removed.length} removed`);
  if (r.secrets.length) {
    lines.push("", "Secrets (only their names are saved; .env.example lists them):");
    lines.push(table(r.secrets.map((x) => [x.name, x.where])).trimEnd());
  }
  const deps = r.warnings.filter((w) => w.kind === "dependency");
  if (deps.length) {
    lines.push("", "Not saved (keep these yourself):");
    for (const d of deps) lines.push(`  ${d.message}`);
  }
  const jobDeps = s.jobs.flatMap((j) => j.dependencies.map((d) => `${j.label} uses ${d.replace("{{home}}", "~")}`));
  if (jobDeps.length) {
    lines.push("", "Files your jobs use (not saved; restore checks they exist):");
    for (const d of jobDeps) lines.push(`  ${d}`);
  }
  const other = r.warnings.filter((w) => w.kind !== "dependency");
  if (other.length) {
    lines.push("", "Notes:");
    for (const w of other) lines.push(`  ${w.message}`);
  }
  if (!r.dryRun && r.committed) {
    lines.push("", "Next: keep the repo somewhere private, for example:", `  gh repo create ${s.name} --private --source ${pretty(r.dir)} --push`);
  }
  return `${lines.join("\n")}\n`;
}

export function program(io: Io, setCode: (n: number) => void): Command {
  const p = new Command();
  p.name("sac")
    .description("Save your Claude Code setup as a git repo and bring it back on any machine.")
    .version("1.0.0-dev")
    .exitOverride()
    .configureOutput({ writeOut: io.out, writeErr: io.err });

  p.command("save")
    .description(`save this machine's Claude Code setup into a git repo (default ${DEFAULT_REPO})`)
    .argument("[dir]", "the setup repo folder", DEFAULT_REPO)
    .option("--dry-run", "show what would change, write nothing")
    .option("--config-dir <dir>", "read a Claude home other than ~/.claude (like CLAUDE_CONFIG_DIR)")
    .option("--job <label...>", "also save these launchd jobs even if they do not run claude")
    .action(async (dir: string, o: { dryRun?: boolean; configDir?: string; job?: string[] }) => {
      const r = await save({ dir: absolute(dir), target: targetFor({ configDir: o.configDir ? absolute(o.configDir) : null }), dryRun: o.dryRun, jobs: o.job });
      if (r.dryRun && r.diff) io.out(`${r.diff}\n`);
      io.out(saveReport(r));
      setCode(0);
    });

  p.command("restore")
    .description(`bring a saved setup back onto this machine (default ${DEFAULT_REPO}); later repos override earlier ones`)
    .argument("[repos...]", "setup repo folders or git URLs")
    .option("--dry-run", "show the plan, change nothing")
    .option("-y, --yes", "apply without asking; secrets that are not set skip what needs them")
    .option("--config-dir <dir>", "restore into a separate Claude home (like CLAUDE_CONFIG_DIR); jobs go to <dir>/sac-jobs")
    .option("--env-file <file>", "secret values (default: .env beside the top repo)")
    .option("--allow-missing", "exit 0 even when items were skipped for missing secrets")
    .option("--load-jobs", "load the written jobs with launchctl or crontab")
    .action(async (repos: string[], o: { dryRun?: boolean; yes?: boolean; configDir?: string; envFile?: string; allowMissing?: boolean; loadJobs?: boolean }) => {
      setCode(
        await restore({
          sources: (repos.length ? repos : [DEFAULT_REPO]).map((r) => (isGitUrl(r) ? r : absolute(r))),
          target: targetFor({ configDir: o.configDir ? absolute(o.configDir) : null }),
          io,
          interactive: !!process.stdin.isTTY && !!process.stdout.isTTY,
          yes: o.yes,
          dryRun: o.dryRun,
          envFile: o.envFile ? absolute(o.envFile) : undefined,
          allowMissing: o.allowMissing,
          loadJobs: o.loadJobs,
        }),
      );
    });

  return p;
}

/** Runs the CLI and returns the exit code. */
export async function main(argv: string[], io: Io = stdio): Promise<number> {
  let code = 0;
  try {
    await program(io, (n) => { code = n; }).parseAsync(argv);
    return code;
  } catch (e) {
    if (e instanceof CommanderError) return e.exitCode;
    io.err(`sac: ${(e as Error).message}\n`);
    return 1;
  }
}
