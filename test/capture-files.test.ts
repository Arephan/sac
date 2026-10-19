import { mkdirSync, readFileSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { findImports, isText, mapImports, readFiles } from "../src/capture/files.js";
import { fixtureHome, type FixtureHome } from "./helpers/home.js";

describe("imports", () => {
  it("finds imports outside code, with escaped spaces", () => {
    const md = "a @x.md\n@notes/my\\ file.md\n`@no`\n```\n@no2\n```\nmail me@site.com\n@~/o.md";
    expect(findImports(md)).toEqual(["x.md", "notes/my file.md", "~/o.md"]);
  });
  it("rewrites import paths and leaves everything else", () => {
    const md = "see @a.md and `@b.md`\n```\n@c.md\n```\n@my\\ d.md";
    expect(mapImports(md, (p) => `../${p}`)).toBe("see @../a.md and `@b.md`\n```\n@c.md\n```\n@../my\\ d.md");
  });
});

describe("isText", () => {
  it("rejects NUL bytes and invalid UTF-8", () => {
    expect(isText(Buffer.from("héllo\n"))).toBe(true);
    expect(isText(Buffer.from([0x41, 0x00]))).toBe(false);
    expect(isText(Buffer.from([0xff, 0xfe, 0x41]))).toBe(false);
  });
});

describe("readFiles", () => {
  let fx: FixtureHome;
  beforeEach(() => { fx = fixtureHome(); });
  afterEach(() => fx.restore());

  it("captures exactly the config files, byte for byte, with modes", async () => {
    const { files, warnings } = await readFiles(fx.target);
    expect(files.map((f) => f.path)).toEqual([
      "CLAUDE.md", "RTK.md", "agents/reviewer.md", "commands/ship.md", "hooks/format.sh", "keybindings.json",
      "notes/my file.md", "output-styles/terse.md", "rules/style.md", "settings.json",
      "skills/hello/SKILL.md", "skills/hello/logo.bin", "skills/my skill/SKILL.md", "statusline.sh",
    ]);
    const get = (p: string) => files.find((f) => f.path === p)!;
    expect(get("hooks/format.sh").mode).toBe(0o755);
    expect(get("rules/style.md").mode).toBe(0o644);
    expect(get("skills/hello/logo.bin").data.equals(readFileSync(join(fx.target.claudeDir, "skills/hello/logo.bin")))).toBe(true);
    expect(warnings).toEqual([
      { kind: "dependency", message: `CLAUDE.md imports ${join(fx.home, "outside-notes.md")}, which is outside the Claude folder; not saved` },
    ]);
  });

  it("reports a symlinked skill instead of following it", async () => {
    mkdirSync(join(fx.home, "elsewhere"));
    writeFileSync(join(fx.home, "elsewhere", "SKILL.md"), "x");
    symlinkSync(join(fx.home, "elsewhere"), join(fx.target.claudeDir, "skills", "linked"));
    const { files, warnings } = await readFiles(fx.target);
    expect(files.some((f) => f.path.startsWith("skills/linked"))).toBe(false);
    expect(warnings).toContainEqual({ kind: "dependency", message: "skills/linked is a symlink; not saved" });
  });

  it("finds hook scripts written with $HOME and quotes, and reports ones outside the Claude folder", async () => {
    const s = JSON.parse(readFileSync(join(fx.target.claudeDir, "settings.json"), "utf8"));
    s.hooks.Stop = [{ hooks: [{ type: "command", command: `"$HOME/.claude/hooks/stop.sh" && ${fx.home}/bin/github-mcp` }] }];
    writeFileSync(join(fx.target.claudeDir, "settings.json"), JSON.stringify(s));
    writeFileSync(join(fx.target.claudeDir, "hooks", "stop.sh"), "#!/bin/sh\n");
    const { files, warnings } = await readFiles(fx.target);
    expect(files.map((f) => f.path)).toContain("hooks/stop.sh");
    expect(warnings).toContainEqual({ kind: "dependency", message: `settings.json runs ${join(fx.home, "bin/github-mcp")}, which is outside the Claude folder; not saved` });
  });

  it("never saves scripts from plugin caches or other never-saved folders", async () => {
    const s = JSON.parse(readFileSync(join(fx.target.claudeDir, "settings.json"), "utf8"));
    s.hooks.Stop = [{ hooks: [{ type: "command", command: "~/.claude/plugins/cache/p/hook.sh" }] }];
    writeFileSync(join(fx.target.claudeDir, "settings.json"), JSON.stringify(s));
    mkdirSync(join(fx.target.claudeDir, "plugins/cache/p"), { recursive: true });
    writeFileSync(join(fx.target.claudeDir, "plugins/cache/p/hook.sh"), "#!/bin/sh\n");
    const { files, warnings } = await readFiles(fx.target);
    expect(files.some((f) => f.path.startsWith("plugins/"))).toBe(false);
    expect(warnings.some((w) => w.message.includes("plugins/"))).toBe(false);
  });

  it("works on an empty Claude folder", async () => {
    const empty = fixtureHome(null);
    try {
      expect(await readFiles(empty.target)).toEqual({ files: [], warnings: [] });
    } finally {
      empty.restore();
    }
  });
});
