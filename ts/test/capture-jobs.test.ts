import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { buildBinary, parse } from "plist";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cronLine, parseCrontab, plistFor, readJobs, runsClaude } from "../src/capture/jobs.js";
import type { Target } from "../src/types.js";
import { fixtureHome, type FixtureHome } from "./helpers/home.js";
import { FAKE } from "./helpers/secrets.js";

const noFiles = () => null;

describe("runsClaude", () => {
  it("spots claude as a command, not as part of another name", () => {
    expect(runsClaude(["/usr/local/bin/claude", "-p", "x"], noFiles)).toBe(true);
    expect(runsClaude(["/bin/sh", "-c", "cd ~/x && claude -p 'go'"], noFiles)).toBe(true);
    expect(runsClaude(["npx", "@anthropic-ai/claude-code", "-p", "x"], noFiles)).toBe(true);
    expect(runsClaude(["/bin/sh", "/Users/x/.claude/hooks/a.sh"], noFiles)).toBe(false);
    expect(runsClaude(["/bin/echo", "com.example.claude-digest"], noFiles)).toBe(false);
  });
  it("looks inside scripts, ignoring comment lines", () => {
    const files: Record<string, string> = { "/s/run.sh": "#!/bin/sh\n# not claude here\nexec claude -p hi\n", "/s/quiet.sh": "#!/bin/sh\n# claude\necho hi\n" };
    expect(runsClaude(["/bin/sh", "/s/run.sh"], (p) => files[p] ?? null)).toBe(true);
    expect(runsClaude(["/bin/sh", "/s/quiet.sh"], (p) => files[p] ?? null)).toBe(false);
  });
});

describe("parseCrontab", () => {
  it("keeps claude lines with their schedules and labels", () => {
    const text = [
      'MAILTO=""',
      "# nightly",
      "0 8 * * * claude -p 'daily digest' >> /tmp/c.log 2>&1",
      "*/5 * * * * /usr/bin/true",
      "@reboot cd /srv && claude --continue",
      "30 9 * * 1-5 claude -p 'daily digest' --model opus",
      "",
    ].join("\n");
    const jobs = parseCrontab(text, noFiles, "/h", "/h/.claude");
    expect(jobs.map((j) => [j.label, j.schedule.cron, j.program[0]])).toEqual([
      ["cron-claude-p-daily-digest", "0 8 * * *", "claude -p 'daily digest' >> /tmp/c.log 2>&1"],
      ["cron-cd-srv-claude-continue", "@reboot", "cd /srv && claude --continue"],
      ["cron-claude-p-daily-digest-2", "30 9 * * 1-5", "claude -p 'daily digest' --model opus"],
    ]);
    expect(jobs.map(cronLine)).toEqual([
      "0 8 * * * claude -p 'daily digest' >> /tmp/c.log 2>&1",
      "@reboot cd /srv && claude --continue",
      "30 9 * * 1-5 claude -p 'daily digest' --model opus",
    ]);
  });
});

describe("readJobs", () => {
  let fx: FixtureHome;
  let t: Target;
  beforeEach(() => {
    fx = fixtureHome();
    t = { ...fx.target, platform: "darwin" };
  });
  afterEach(() => fx.restore());

  it("captures launchd jobs that run claude, directly or through a script", async () => {
    const { jobs, warnings } = await readJobs(t);
    expect(warnings).toEqual([]);
    expect(jobs.map((j) => j.label)).toEqual(["com.example.claude-digest", "com.example.script-job"]);
    expect(jobs[0]).toEqual({
      label: "com.example.claude-digest",
      kind: "launchd",
      program: ["/usr/local/bin/claude", "-p", "daily digest"],
      schedule: { StartCalendarInterval: { Hour: 8, Minute: 0 } },
      workingDirectory: fx.home,
      env: { SLACK_TOKEN: FAKE.SLACK_JOB, TZ: "UTC" },
      stdout: join(fx.home, "logs/digest.log"),
      dependencies: [],
      extra: { ProcessType: "Background" },
    });
    expect(jobs[1]).toMatchObject({ schedule: { StartInterval: 3600 }, dependencies: [join(fx.home, "bin/nightly.sh")] });
  });

  it("adds jobs named with labels, and reads binary plists", async () => {
    const dir = join(fx.home, "Library/LaunchAgents");
    writeFileSync(join(dir, "com.example.bin.plist"), Buffer.from(buildBinary({ Label: "com.example.bin", ProgramArguments: ["claude", "-p", "x"] })));
    const { jobs } = await readJobs(t, { labels: ["com.example.unrelated"] });
    expect(jobs.map((j) => j.label)).toEqual(["com.example.bin", "com.example.claude-digest", "com.example.script-job", "com.example.unrelated"]);
  });

  it("builds each plist back exactly", async () => {
    const { jobs } = await readJobs(t, { labels: ["com.example.unrelated"] });
    for (const j of jobs) {
      const original = parse(readFileSync(join(fx.home, "Library/LaunchAgents", `${j.label}.plist`), "utf8"));
      expect(plistFor(j)).toEqual(original);
    }
  });

  it("reads claude lines from the crontab on any platform", async () => {
    writeFileSync(join(fx.home, ".sac-test-crontab"), "0 8 * * * claude -p hi\n* * * * * /usr/bin/true\n");
    const { jobs } = await readJobs({ ...t, platform: "linux" });
    expect(jobs).toEqual([
      { label: "cron-claude-p-hi", kind: "cron", program: ["claude -p hi"], schedule: { cron: "0 8 * * *" }, env: {}, dependencies: [] },
    ]);
  });

  it("reports an unreadable plist and carries on", async () => {
    writeFileSync(join(fx.home, "Library/LaunchAgents/broken.plist"), "<plist><dict><key>Label</key>");
    const { jobs, warnings } = await readJobs(t);
    expect(jobs).toHaveLength(2);
    expect(warnings).toEqual([{ kind: "other", message: `${join(fx.home, "Library/LaunchAgents/broken.plist")} could not be read; skipped` }]);
  });
});
