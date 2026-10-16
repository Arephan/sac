import { mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execa } from "execa";
import YAML from "yaml";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { main } from "../src/cli.js";
import { readRepo } from "../src/repo.js";
import { assertNoSecrets, save } from "../src/save.js";
import { ph } from "../src/secrets.js";
import { fixtureHome, type FixtureHome } from "./helpers/home.js";
import { modeOf, treeOf } from "./helpers/repo.js";
import { ALL_FAKE_VALUES, FAKE } from "./helpers/secrets.js";

const commits = async (dir: string) => (await execa("git", ["-C", dir, "rev-list", "--count", "HEAD"])).stdout.trim();

describe("sac save", () => {
  let fx: FixtureHome;
  let dir: string;
  beforeEach(() => {
    fx = fixtureHome();
    dir = join(realpathSync(mkdtempSync(join(tmpdir(), "sac-repo-"))), "my-claude");
  });
  afterEach(() => {
    rmSync(join(dir, ".."), { recursive: true, force: true });
    fx.restore();
  });

  it("writes the repo layout, with placeholders and path tokens instead of values and paths", async () => {
    const r = await save({ dir, target: { ...fx.target, platform: "darwin" } });
    expect(r.committed).toBe(true);
    const tree = treeOf(dir);
    expect([...tree.keys()]).toEqual([
      ".env.example", ".gitignore",
      "claude/CLAUDE.md", "claude/RTK.md", "claude/agents/reviewer.md", "claude/commands/ship.md", "claude/hooks/format.sh",
      "claude/keybindings.json", "claude/notes/my file.md", "claude/output-styles/terse.md", "claude/rules/style.md",
      "claude/settings.json", "claude/skills/hello/SKILL.md", "claude/skills/hello/logo.bin", "claude/skills/my skill/SKILL.md",
      "claude/statusline.sh",
      "jobs/com.example.claude-digest.yaml", "jobs/com.example.script-job.yaml",
      "mcp.json", "plugins.json", "sac.yaml",
    ]);
    for (const [p, data] of tree) {
      for (const v of ALL_FAKE_VALUES) expect(data.includes(v), `${p} holds a secret`).toBe(false);
      expect(data.includes(fx.home), `${p} holds the home path`).toBe(false);
    }
    expect(modeOf(join(dir, "claude/hooks/format.sh"))).toBe(0o755);

    const text = (p: string) => tree.get(p)!.toString("utf8");
    const settings = JSON.parse(text("claude/settings.json"));
    expect(settings.enabledPlugins).toBeUndefined();
    expect(settings.env).toEqual({ GITHUB_TOKEN: ph("GITHUB_TOKEN"), LOG_LEVEL: "info", DEPLOY_HOOK: ph("DEPLOY_HOOK") });
    expect(settings.statusLine.command).toBe("{{claude}}/statusline.sh");
    expect(settings.hooks.PostToolUse[0].hooks[0].command).toBe("~/.claude/hooks/format.sh --quiet");
    expect(text("claude/RTK.md")).toBe("Use rtk. Logs go to {{home}}/logs and ~/logs.\n");

    const mcp = JSON.parse(text("mcp.json")).mcpServers;
    expect(mcp.github).toEqual({ type: "stdio", command: "{{home}}/bin/github-mcp", args: ["stdio"], env: { GITHUB_PERSONAL_ACCESS_TOKEN: ph("GITHUB_PERSONAL_ACCESS_TOKEN") } });
    expect(mcp.slackbot.args).toEqual(["slack-mcp", "--token", ph("SLACKBOT_ARGS_2")]);

    const plugins = JSON.parse(text("plugins.json"));
    expect(plugins.marketplaces[2]).toEqual({ name: "local", source: "{{home}}/my-plugins", kind: "directory" });
    expect(plugins.plugins[2]).toEqual({ id: "deploy@acme", enabled: true, settings: { region: "eu-west-1", apiKey: ph("DEPLOY_APIKEY") } });

    const digest = YAML.parse(text("jobs/com.example.claude-digest.yaml"));
    expect(digest).toMatchObject({ env: { SLACK_TOKEN: ph("SLACK_TOKEN"), TZ: "UTC" }, stdout: "{{home}}/logs/digest.log", workingDirectory: "{{home}}" });
    expect(YAML.parse(text("jobs/com.example.script-job.yaml")).dependencies).toEqual(["{{home}}/bin/nightly.sh"]);

    const env = text(".env.example");
    for (const n of ["DEPLOY_APIKEY", "DEPLOY_HOOK", "GITHUB_PERSONAL_ACCESS_TOKEN", "GITHUB_TOKEN", "LINEAR_AUTHORIZATION", "SLACKBOT_ARGS_2", "SLACK_TOKEN"]) {
      expect(env).toMatch(new RegExp(`^${n}=$`, "m"));
    }
    expect(text(".gitignore")).toMatch(/^\.env$/m);
    expect(YAML.parse(text("sac.yaml"))).toMatchObject({ version: 1, name: "my-claude", captured: { claude: "2.1.301", os: "darwin" } });
    expect(await commits(dir)).toBe("1");
  });

  it("reads back what it wrote", async () => {
    await save({ dir, target: fx.target });
    const s = readRepo(dir);
    expect(s.name).toBe("my-claude");
    expect(s.files.find((f) => f.path === "skills/hello/logo.bin")!.data.equals(readFileSync(join(fx.target.claudeDir, "skills/hello/logo.bin")))).toBe(true);
    expect(s.files.find((f) => f.path === "hooks/format.sh")!.mode).toBe(0o755);
    expect(Object.keys(s.mcp)).toEqual(["github", "linear", "slackbot"]);
    expect(s.plugins.map((p) => p.id)).toEqual(["superpowers@claude-plugins-official", "posthog@claude-plugins-official", "deploy@acme"]);
  });

  it("commits nothing when nothing changed, and keeps the name and extends", async () => {
    await save({ dir, target: fx.target });
    const meta = YAML.parse(readFileSync(join(dir, "sac.yaml"), "utf8"));
    writeFileSync(join(dir, "sac.yaml"), YAML.stringify({ ...meta, name: "sam", extends: ["../base"] }));
    await execa("git", ["-C", dir, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qam", "rename"]);
    const again = await save({ dir, target: fx.target });
    expect(again.committed).toBe(false);
    expect(await commits(dir)).toBe("2");
    expect(YAML.parse(readFileSync(join(dir, "sac.yaml"), "utf8"))).toMatchObject({ name: "sam", extends: ["../base"] });
  });

  it("shows a dry run as a diff and writes nothing", async () => {
    await save({ dir, target: fx.target, now: new Date("2026-10-16T10:00:00Z") });
    writeFileSync(join(fx.target.claudeDir, "rules/style.md"), "Prefer very short answers.\n");
    const r = await save({ dir, target: fx.target, dryRun: true, now: new Date("2026-10-16T11:00:00Z") });
    expect(r.diff).toContain("claude/rules/style.md");
    expect(r.diff).toContain("+Prefer very short answers.");
    expect(readFileSync(join(dir, "claude/rules/style.md"), "utf8")).toBe("Prefer short answers.\n");
    expect(r.changes).toEqual({ added: [], changed: ["claude/rules/style.md", "sac.yaml"], removed: [] });
  });

  it("removes files that are gone from the machine", async () => {
    await save({ dir, target: fx.target });
    rmSync(join(fx.target.claudeDir, "commands/ship.md"));
    const r = await save({ dir, target: fx.target });
    expect(r.changes.removed).toEqual(["claude/commands/ship.md"]);
    expect(treeOf(dir).has("claude/commands/ship.md")).toBe(false);
  });

  it("replaces a token inside a text file and says so", async () => {
    writeFileSync(join(fx.target.claudeDir, "skills/hello/notes.md"), `Use ${FAKE.SLACK_SPARE} for posting.\n`);
    const r = await save({ dir, target: fx.target });
    expect(readFileSync(join(dir, "claude/skills/hello/notes.md"), "utf8")).toBe(`Use ${ph("SKILLS_HELLO_NOTES_MD")} for posting.\n`);
    expect(r.warnings).toContainEqual({ kind: "secret-in-file", message: "skills/hello/notes.md line 1 holds a slack credential; saved as {{secret:SKILLS_HELLO_NOTES_MD}}" });
  });

  it("refuses output that still holds a secret", async () => {
    const files = new Map([["claude/x.md", { data: Buffer.from(`k=${FAKE.GH_TOKEN}`), mode: 0o644 }]]);
    await expect(assertNoSecrets(files, [])).rejects.toThrow("claude/x.md line 1 still holds a github credential");
    const quiet = new Map([["claude/y.md", { data: Buffer.from("value: plain-but-known"), mode: 0o644 }]]);
    await expect(assertNoSecrets(quiet, ["plain-but-known"])).rejects.toThrow("claude/y.md still holds a secret value");
  });

  it("prints a summary through the CLI and never a secret value", async () => {
    let out = "";
    const code = await main(["node", "sac", "save", dir], { out: (s) => { out += s; }, err: (s) => { out += s; } });
    expect(code).toBe(0);
    expect(out).toContain("GITHUB_TOKEN");
    expect(out).toContain("outside-notes.md");
    for (const v of ALL_FAKE_VALUES) expect(out).not.toContain(v);
  });
});
