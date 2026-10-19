import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { claudeCli } from "../src/claude-cli.js";
import { doctor, parseMcpList, type DoctorResult } from "../src/doctor.js";
import { prepare, restore, sacHome } from "../src/restore.js";
import { save } from "../src/save.js";
import type { Target } from "../src/types.js";
import { fixtureHome, type FixtureHome } from "./helpers/home.js";
import { ALL_FAKE_VALUES, FAKE } from "./helpers/secrets.js";

let tmp: string;
let a: FixtureHome;
let b: FixtureHome;
let repo: string;
let out: string;

const io = { out: (s: string) => { out += s; }, err: (s: string) => { out += s; } };
const darwin = (fx: FixtureHome): Target => ({ ...fx.target, platform: "darwin" });

async function check(captured?: string): Promise<DoctorResult> {
  const t = darwin(b);
  const cli = claudeCli(t);
  const prep = await prepare([repo], t, cli, undefined);
  const r = await doctor(prep.setup, t, cli, prep.machine, { values: prep.values, sacHome: sacHome(), capturedClaude: captured ?? prep.setup.captured?.claude });
  out += JSON.stringify(r);
  return r;
}

const messages = (r: DoctorResult, level: string) => r.findings.filter((f) => f.level === level).map((f) => f.message);

beforeEach(async () => {
  out = "";
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "sac-doctor-")));
  a = fixtureHome();
  repo = join(tmp, "repo");
  await save({ dir: repo, target: darwin(a) });
  const envFile = join(tmp, "test.env");
  writeFileSync(envFile, [
    `GITHUB_TOKEN=${FAKE.GH_TOKEN}`, `DEPLOY_HOOK=${FAKE.SLACK_BOT}`, `GITHUB_PERSONAL_ACCESS_TOKEN=${FAKE.GH_MCP}`,
    `LINEAR_AUTHORIZATION="${FAKE.LINEAR}"`, `SLACKBOT_ARGS_2=${FAKE.SLACK_MCP}`, `DEPLOY_APIKEY=${FAKE.DEPLOY_KEY}`, `SLACK_TOKEN=${FAKE.SLACK_JOB}`,
  ].join("\n"));
  b = fixtureHome(null);
  mkdirSync(join(b.home, "my-plugins/.claude-plugin"), { recursive: true });
  writeFileSync(join(b.home, "my-plugins/.claude-plugin/marketplace.json"), JSON.stringify({ name: "local", plugins: [] }));
  mkdirSync(join(b.home, "bin"), { recursive: true });
  writeFileSync(join(b.home, "bin/nightly.sh"), "#!/bin/sh\nclaude -p 'nightly sweep'\n", { mode: 0o755 });
  expect(await restore({ sources: [repo], target: darwin(b), yes: true, io, interactive: false, envFile })).toBe(0);
  out = "";
});

afterEach(() => {
  for (const v of ALL_FAKE_VALUES) expect(out, "a secret value was printed").not.toContain(v);
  b.restore();
  a.restore();
  rmSync(tmp, { recursive: true, force: true });
});

describe("sac doctor", () => {
  it("passes right after a restore, warning only that jobs are not loaded", async () => {
    const r = await check();
    expect(r.ok).toBe(true);
    expect(messages(r, "missing")).toEqual([]);
    expect(messages(r, "differs")).toEqual([]);
    expect(messages(r, "warn")).toEqual(["job com.example.claude-digest is written but not loaded", "job com.example.script-job is written but not loaded"]);
  });

  it("reports a removed skill, a disabled plugin, a failing server and an unknown secret", async () => {
    rmSync(join(b.target.claudeDir, "skills/my skill"), { recursive: true });
    b.setState((s) => {
      s.plugins.find((p: { id: string }) => p.id === "superpowers@claude-plugins-official").enabled = false;
      s.mcpStatus = { linear: "✗ Failed to connect" };
      delete s.configure["deploy@acme"];
    });
    const r = await check();
    expect(r.ok).toBe(false);
    expect(messages(r, "missing")).toContain("file skills/my skill/SKILL.md is not on this machine");
    expect(messages(r, "differs")).toContain("plugin superpowers@claude-plugins-official should be enabled");
    expect(messages(r, "failing")).toEqual(["MCP server linear fails to connect"]);
    expect(messages(r, "missing")).toContain("secret DEPLOY_APIKEY is not set (environment, env file, or on this machine)");
  });

  it("lists extras without failing, and warns about an older Claude Code", async () => {
    b.setState((s) => { s.plugins.push({ id: "extra@claude-plugins-official", scope: "user", enabled: true }); });
    mkdirSync(join(b.target.claudeDir, "agents"), { recursive: true });
    writeFileSync(join(b.target.claudeDir, "agents/local-only.md"), "mine\n");
    const r = await check("9.0.0");
    expect(r.ok).toBe(true);
    expect(messages(r, "info")).toEqual(["plugin extra@claude-plugins-official is on this machine but in no layer", "agents/local-only.md is on this machine but in no layer"]);
    expect(messages(r, "warn")).toContain("Claude Code 2.1.301 is older than 9.0.0, which this setup was saved with");
  });

  it("reads claude mcp list output", () => {
    expect(parseMcpList("Checking MCP server health...\n\ngithub: /x/github-mcp stdio - ✓ Connected\nlinear: https://mcp.linear.app/mcp (HTTP) - ! Needs authentication\nbad: npx bad - ✗ Failed to connect\n")).toEqual({ github: "ok", linear: "auth", bad: "failing" });
  });
});
