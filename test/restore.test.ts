import { chmodSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parse } from "plist";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { targetFor } from "../src/paths.js";
import { restore, type RestoreOptions } from "../src/restore.js";
import { save } from "../src/save.js";
import type { Target } from "../src/types.js";
import { fixtureHome, type FixtureHome } from "./helpers/home.js";
import { file, makeLayer, settingsFile } from "./helpers/layers.js";
import { treeOf } from "./helpers/repo.js";
import { ALL_FAKE_VALUES, FAKE } from "./helpers/secrets.js";

const MUTATING = (argv: string[]) =>
  ["install", "add", "add-json", "remove", "enable", "disable", "bootstrap", "bootout"].some((w) => argv.includes(w)) || argv.includes("--values-stdin") || argv[0] === "-";

let tmp: string;
let a: FixtureHome;
let b: FixtureHome;
let repo: string;
let envFile: string;
let out: string;

const io = { out: (s: string) => { out += s; }, err: (s: string) => { out += s; } };
const darwin = (fx: FixtureHome): Target => ({ ...fx.target, platform: "darwin" });
const run = (o: Partial<RestoreOptions> & { target: Target }) => restore({ sources: [repo], yes: true, io, interactive: false, ...o });
const mapHome = (s: string) => s.split(a.home).join(b.home);

function addDependencies(fx: FixtureHome) {
  mkdirSync(join(fx.home, "my-plugins/.claude-plugin"), { recursive: true });
  writeFileSync(join(fx.home, "my-plugins/.claude-plugin/marketplace.json"), JSON.stringify({ name: "local", plugins: [] }));
  mkdirSync(join(fx.home, "bin"), { recursive: true });
  writeFileSync(join(fx.home, "bin/nightly.sh"), "#!/bin/sh\nclaude -p 'nightly sweep'\n", { mode: 0o755 });
}

beforeEach(async () => {
  out = "";
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "sac-restore-")));
  a = fixtureHome();
  repo = join(tmp, "repo");
  await save({ dir: repo, target: darwin(a) });
  envFile = join(tmp, "test.env");
  writeFileSync(envFile, [
    `GITHUB_TOKEN=${FAKE.GH_TOKEN}`, `DEPLOY_HOOK=${FAKE.SLACK_BOT}`, `GITHUB_PERSONAL_ACCESS_TOKEN=${FAKE.GH_MCP}`,
    `LINEAR_AUTHORIZATION="${FAKE.LINEAR}"`, `SLACKBOT_ARGS_2=${FAKE.SLACK_MCP}`, `DEPLOY_APIKEY=${FAKE.DEPLOY_KEY}`, `SLACK_TOKEN=${FAKE.SLACK_JOB}`,
  ].join("\n"));
  b = fixtureHome(null);
});

afterEach(() => {
  for (const v of ALL_FAKE_VALUES) expect(out, "a secret value was printed").not.toContain(v);
  b.restore();
  a.restore();
  rmSync(tmp, { recursive: true, force: true });
});

describe("sac restore", () => {
  it("rebuilds the setup on an empty machine, and saving it gives the same repo", async () => {
    addDependencies(b);
    expect(await run({ target: darwin(b), envFile })).toBe(0);

    for (const f of ["CLAUDE.md", "RTK.md", "notes/my file.md", "rules/style.md", "skills/my skill/SKILL.md", "skills/hello/logo.bin", "hooks/format.sh", "statusline.sh", "keybindings.json"]) {
      const want = readFileSync(join(a.target.claudeDir, f));
      const got = readFileSync(join(b.target.claudeDir, f));
      expect(got.toString("latin1"), f).toBe(mapHome(want.toString("latin1")));
      expect(statSync(join(b.target.claudeDir, f)).mode & 0o777, f).toBe(statSync(join(a.target.claudeDir, f)).mode & 0o777);
    }
    const wantSettings = JSON.parse(mapHome(readFileSync(join(a.target.claudeDir, "settings.json"), "utf8")));
    delete wantSettings.enabledPlugins;
    expect(JSON.parse(readFileSync(join(b.target.claudeDir, "settings.json"), "utf8"))).toEqual(wantSettings);
    expect(JSON.parse(readFileSync(b.target.claudeJson, "utf8")).mcpServers).toEqual(JSON.parse(mapHome(readFileSync(a.target.claudeJson, "utf8"))).mcpServers);

    const state = b.state();
    expect(state.marketplaces.map((m: { name: string }) => m.name)).toEqual(["claude-plugins-official", "acme", "local"]);
    expect(state.plugins).toEqual([
      { id: "superpowers@claude-plugins-official", scope: "user", enabled: true },
      { id: "posthog@claude-plugins-official", scope: "user", enabled: false },
      { id: "deploy@acme", scope: "user", enabled: true },
    ]);
    expect(state.configure["deploy@acme"].inputs).toEqual({ region: "eu-west-1", apiKey: FAKE.DEPLOY_KEY });

    // Unloaded jobs wait in ~/.sac/jobs: launchd would load anything in LaunchAgents at the next login.
    for (const label of ["com.example.claude-digest", "com.example.script-job"]) {
      const want = parse(mapHome(readFileSync(join(a.home, "Library/LaunchAgents", `${label}.plist`), "utf8")));
      expect(parse(readFileSync(join(b.home, ".sac/jobs", `${label}.plist`), "utf8"))).toEqual(want);
    }
    expect(existsSync(join(b.home, "Library"))).toBe(false);
    expect(b.calls().filter((c) => c.bin === "launchctl")).toEqual([]);
    expect(out).toContain("launchctl bootstrap");

    expect(await run({ target: darwin(b), loadJobs: true })).toBe(0);
    const repo2 = join(tmp, "repo2");
    await save({ dir: repo2, target: darwin(b) });
    const t1 = treeOf(repo);
    const t2 = treeOf(repo2);
    t1.delete("sac.yaml");
    t2.delete("sac.yaml");
    expect([...t2.keys()]).toEqual([...t1.keys()]);
    for (const [p, data] of t1) expect(t2.get(p)!.toString("latin1"), p).toBe(data.toString("latin1"));
  });

  it("changes nothing the second time and needs no secrets for it", async () => {
    addDependencies(b);
    await run({ target: darwin(b), envFile });
    b.clearCalls();
    out = "";
    expect(await run({ target: darwin(b) })).toBe(0);
    expect(out).toContain("Everything already matches");
    expect(b.calls().filter((c) => MUTATING(c.argv))).toEqual([]);
    await run({ target: darwin(b), loadJobs: true });
    b.clearCalls();
    out = "";
    expect(await run({ target: darwin(b) })).toBe(0);
    expect(out).toContain("Everything already matches");
    expect(b.calls().filter((c) => MUTATING(c.argv))).toEqual([]);
  });

  it("merges into a machine that has its own config, and backs up what it overwrites", async () => {
    addDependencies(b);
    writeFileSync(join(b.target.claudeDir, "settings.json"), JSON.stringify({ model: "sonnet", theme: "dark", permissions: { allow: ["Bash(npm:*)"] } }));
    writeFileSync(b.target.claudeJson, JSON.stringify({ numStartups: 9, mcpServers: { github: { type: "stdio", command: "/old", args: [] } } }));
    b.setState((s) => {
      s.marketplaces = [{ name: "claude-plugins-official", source: "github", repo: "anthropics/claude-plugins-official" }];
      s.plugins = [{ id: "extra@claude-plugins-official", scope: "user", enabled: true }];
    });
    expect(await run({ target: darwin(b), envFile })).toBe(0);
    const settings = JSON.parse(readFileSync(join(b.target.claudeDir, "settings.json"), "utf8"));
    expect(settings.model).toBe("opus");
    expect(settings.theme).toBe("dark");
    expect(settings.permissions).toEqual({ allow: ["Bash(npm:*)", "Bash(ls:*)"], deny: ["Bash(rm -rf:*)"] });
    const cj = JSON.parse(readFileSync(b.target.claudeJson, "utf8"));
    expect(cj.numStartups).toBe(9);
    expect(cj.mcpServers.github.command).toBe(join(b.home, "bin/github-mcp"));
    expect(b.state().plugins.map((p: { id: string }) => p.id)).toContain("extra@claude-plugins-official");
    const backups = join(b.home, ".sac/backups");
    const [stamp] = readdirSync(backups);
    expect(JSON.parse(readFileSync(join(backups, stamp, ".claude/settings.json"), "utf8")).model).toBe("sonnet");
    expect(out).toMatch(/~ settings\.json/);
    expect(out).toMatch(/~ github/);
  });

  it("skips what it cannot do, says why, and exits 1", async () => {
    expect(await run({ target: darwin(b) })).toBe(1);
    expect(out).toContain("needs secret GITHUB_PERSONAL_ACCESS_TOKEN");
    expect(out).toContain(`folder ${join(b.home, "my-plugins")} is not on this machine`);
    expect(out).toContain(`needs ${join(b.home, "bin/nightly.sh")}, which is not on this machine`);
    expect(existsSync(join(b.target.claudeDir, "CLAUDE.md"))).toBe(true);
    expect(JSON.parse(readFileSync(join(b.target.claudeDir, "settings.json"), "utf8")).env).toEqual({ LOG_LEVEL: "info" });
  });

  it("exits 0 for missing secrets with --allow-missing", async () => {
    addDependencies(b);
    expect(await run({ target: darwin(b), allowMissing: true })).toBe(0);
    expect(out).toContain("needs secret");
  });

  it("asks for missing secrets and for confirmation when interactive", async () => {
    addDependencies(b);
    const asked: string[] = [];
    const values: Record<string, string> = { GITHUB_TOKEN: FAKE.GH_TOKEN, DEPLOY_HOOK: FAKE.SLACK_BOT, GITHUB_PERSONAL_ACCESS_TOKEN: FAKE.GH_MCP, LINEAR_AUTHORIZATION: FAKE.LINEAR, SLACKBOT_ARGS_2: FAKE.SLACK_MCP, DEPLOY_APIKEY: FAKE.DEPLOY_KEY, SLACK_TOKEN: FAKE.SLACK_JOB };
    const code = await run({
      target: darwin(b), yes: false, interactive: true,
      ask: async (name) => { asked.push(name); return values[name]; },
      confirm: async () => true,
    });
    expect(code).toBe(0);
    expect(asked.sort()).toEqual(Object.keys(values).sort());
    expect(JSON.parse(readFileSync(b.target.claudeJson, "utf8")).mcpServers.github.env.GITHUB_PERSONAL_ACCESS_TOKEN).toBe(FAKE.GH_MCP);
  });

  it("does nothing on a dry run or when not confirmed", async () => {
    addDependencies(b);
    expect(await run({ target: darwin(b), envFile, dryRun: true })).toBe(0);
    expect(out).toContain("+ CLAUDE.md");
    expect(readdirSync(b.target.claudeDir)).toEqual([]);
    expect(await run({ target: darwin(b), envFile, yes: false, interactive: true, confirm: async () => false })).toBe(1);
    expect(readdirSync(b.target.claudeDir)).toEqual([]);
    expect(b.calls().filter((c) => MUTATING(c.argv))).toEqual([]);
    expect(await run({ target: darwin(b), envFile, yes: false, interactive: false })).toBe(2);
  });

  it("restores layers: an org base under a personal setup", async () => {
    makeLayer(join(tmp, "org"), { name: "org", files: [file("CLAUDE.md", "Org rules.\n"), settingsFile({ permissions: { deny: ["Bash(curl:*)"] } })] });
    makeLayer(join(tmp, "me"), { name: "me", extends: ["../org"], files: [file("CLAUDE.md", "Mine.\n"), settingsFile({ permissions: { deny: ["Bash(rm:*)"] } })] });
    expect(await restore({ sources: [join(tmp, "me")], target: darwin(b), yes: true, io, interactive: false })).toBe(0);
    expect(readFileSync(join(b.target.claudeDir, "CLAUDE.md"), "utf8")).toBe("Mine.\n");
    expect(readFileSync(join(b.target.claudeDir, "rules/sac-org.md"), "utf8")).toBe("Org rules.\n");
    expect(JSON.parse(readFileSync(join(b.target.claudeDir, "settings.json"), "utf8")).permissions.deny).toEqual(["Bash(curl:*)", "Bash(rm:*)"]);
  });

  it("restores into a separate Claude home, keeping jobs out of LaunchAgents", async () => {
    addDependencies(b);
    const alt = join(b.home, "alt");
    const t = targetFor({ home: b.home, configDir: alt, platform: "darwin" });
    expect(await run({ target: t, envFile })).toBe(0);
    expect(existsSync(join(alt, "CLAUDE.md"))).toBe(true);
    expect(existsSync(join(b.home, ".claude/CLAUDE.md"))).toBe(false);
    expect(Object.keys(JSON.parse(readFileSync(join(alt, ".claude.json"), "utf8")).mcpServers)).toEqual(["github", "linear", "slackbot"]);
    expect(b.state(alt).plugins).toHaveLength(3);
    expect(existsSync(join(alt, "sac-jobs/com.example.claude-digest.plist"))).toBe(true);
    expect(existsSync(join(b.home, "Library"))).toBe(false);
    expect(JSON.parse(readFileSync(join(alt, "settings.json"), "utf8")).statusLine.command).toBe(join(alt, "statusline.sh"));
    expect(await run({ target: t, envFile, loadJobs: true })).toBe(2);
  });

  it("loads jobs only with --load-jobs", async () => {
    addDependencies(b);
    expect(await run({ target: darwin(b), envFile, loadJobs: true })).toBe(0);
    const uid = process.getuid!();
    expect(b.calls().filter((c) => c.bin === "launchctl").map((c) => c.argv)).toEqual([
      ["bootstrap", `gui/${uid}`, join(b.home, "Library/LaunchAgents/com.example.claude-digest.plist")],
      ["bootstrap", `gui/${uid}`, join(b.home, "Library/LaunchAgents/com.example.script-job.plist")],
    ]);
  });

  it("never follows a symlink inside a setup repo", async () => {
    const dir = makeLayer(join(tmp, "evil"), { name: "evil", files: [file("CLAUDE.md", "hi\n")] });
    writeFileSync(join(tmp, "private.txt"), "not for you\n");
    mkdirSync(join(dir, "claude/agents"), { recursive: true });
    symlinkSync(join(tmp, "private.txt"), join(dir, "claude/agents/x.md"));
    symlinkSync(tmp, join(dir, "claude/skills"));
    expect(await restore({ sources: [dir], target: darwin(b), yes: true, io, interactive: false })).toBe(0);
    expect(readdirSync(b.target.claudeDir)).toEqual(["CLAUDE.md"]);
  });

  it("never shows an old or new secret value in a settings diff, and keeps the file's mode", async () => {
    const old = "old-password-value-123";
    const fresh = "new-password-value-456";
    makeLayer(join(tmp, "s"), { name: "s", files: [settingsFile({ env: { DB_PASSWORD: "{{secret:DB_PASSWORD}}", MODE: "a" } })] });
    const sp = join(b.target.claudeDir, "settings.json");
    writeFileSync(sp, JSON.stringify({ env: { DB_PASSWORD: old, MODE: "b" } }));
    chmodSync(sp, 0o600);
    writeFileSync(join(tmp, "s.env"), `DB_PASSWORD=${fresh}\n`);
    expect(await restore({ sources: [join(tmp, "s")], target: darwin(b), yes: true, io, interactive: false, envFile: join(tmp, "s.env") })).toBe(0);
    expect(out).toMatch(/~ settings\.json/);
    expect(out).not.toContain(old);
    expect(out).not.toContain(fresh);
    expect(JSON.parse(readFileSync(sp, "utf8")).env).toEqual({ DB_PASSWORD: fresh, MODE: "a" });
    expect(statSync(sp).mode & 0o777).toBe(0o600);
  });

  it("does not ask for secrets with --yes, even in a terminal", async () => {
    addDependencies(b);
    const asked: string[] = [];
    expect(await run({ target: darwin(b), interactive: true, ask: async (n) => { asked.push(n); return undefined; } })).toBe(1);
    expect(asked).toEqual([]);
  });
});
