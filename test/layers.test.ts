import { mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execa } from "execa";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { isGitUrl, loadLayers, mergeLayers, mergeSettings } from "../src/layers.js";
import { file, makeLayer, settingsFile } from "./helpers/layers.js";

let root: string;
beforeEach(() => { root = realpathSync(mkdtempSync(join(tmpdir(), "sac-layers-"))); });
afterEach(() => rmSync(root, { recursive: true, force: true }));
const cache = () => join(root, "cache");

describe("mergeSettings", () => {
  it("combines lists without duplicates, merges objects, later values win", () => {
    expect(mergeSettings(
      { model: "sonnet", permissions: { allow: ["a", "b"], deny: ["x"] }, env: { A: "1" }, theme: "dark" },
      { model: "opus", permissions: { allow: ["b", "c"], deny: ["y"] }, env: { B: "2" } },
    )).toEqual({ model: "opus", permissions: { allow: ["a", "b", "c"], deny: ["x", "y"] }, env: { A: "1", B: "2" }, theme: "dark" });
  });
});

describe("isGitUrl", () => {
  it("tells URLs from folders", () => {
    for (const u of ["https://github.com/a/b", "git@github.com:a/b.git", "ssh://x/y", "file:///tmp/x", "../base.git"]) expect(isGitUrl(u), u).toBe(true);
    for (const p of ["../base", "/abs/dir", "~/my-claude"]) expect(isGitUrl(p), p).toBe(false);
  });
});

describe("layers", () => {
  it("puts bases first and merges by the spec's rules", async () => {
    makeLayer(join(root, "base"), {
      name: "base",
      files: [
        file("CLAUDE.md", "Base rules.\n@RTK.md\n@missing.md\n"), file("RTK.md", "rtk\n"), file("rules/security.md", "no secrets\n"),
        file("rules/shared.md", "base wording\n"),
        settingsFile({ model: "sonnet", permissions: { allow: ["Bash(ls:*)"], deny: ["Bash(rm:*)"] } }),
      ],
      mcp: { shared: { command: "a" }, linear: { type: "http", url: "https://one" } },
      marketplaces: [{ name: "m1", source: "acme/m1", kind: "github" }],
      plugins: [{ id: "p1@m1", enabled: true, settings: {} }],
    });
    const top = makeLayer(join(root, "sam"), {
      name: "sam",
      extends: ["../base"],
      files: [
        file("CLAUDE.md", "Sam.\n"), file("rules/style.md", "short\n"), file("rules/shared.md", "sam wording\n"),
        settingsFile({ model: "opus", permissions: { allow: ["Bash(git:*)"], deny: ["Bash(curl:*)"] } }),
      ],
      mcp: { linear: { type: "http", url: "https://two" } },
      plugins: [{ id: "p1@m1", enabled: false, settings: {} }],
    });
    const layers = await loadLayers([top], { cacheDir: cache() });
    expect(layers.map((l) => l.name)).toEqual(["base", "sam"]);
    const { setup, warnings } = mergeLayers(layers);
    const text = (p: string) => setup.files.find((f) => f.path === p)?.data.toString("utf8");
    expect(setup.name).toBe("sam");
    expect(text("CLAUDE.md")).toBe("Sam.\n");
    expect(text("rules/sac-base.md")).toBe("Base rules.\n@../RTK.md\n@missing.md\n");
    expect(text("RTK.md")).toBe("rtk\n");
    expect(text("rules/security.md")).toBe("no secrets\n");
    expect(text("rules/shared.md")).toBe("sam wording\n");
    expect(JSON.parse(text("settings.json")!)).toEqual({ model: "opus", permissions: { allow: ["Bash(ls:*)", "Bash(git:*)"], deny: ["Bash(rm:*)", "Bash(curl:*)"] } });
    expect(setup.mcp).toEqual({ shared: { command: "a" }, linear: { type: "http", url: "https://two" } });
    expect(setup.plugins).toEqual([{ id: "p1@m1", enabled: false, settings: {} }]);
    expect(setup.marketplaces).toEqual([{ name: "m1", source: "acme/m1", kind: "github" }]);
    expect(warnings).toEqual([
      { kind: "clash", message: "rules/shared.md: sam replaces base's version" },
      { kind: "clash", message: "MCP server linear: sam replaces base's version" },
      { kind: "clash", message: "plugin p1@m1: sam replaces base's version" },
    ]);
  });

  it("clones a git layer into the cache and updates it later", async () => {
    const src = makeLayer(join(root, "src"), { name: "org", files: [file("rules/a.md", "one\n")] });
    const git = (...a: string[]) => execa("git", ["-C", src, "-c", "user.name=t", "-c", "user.email=t@t", ...a]);
    await execa("git", ["init", "-q", "-b", "main", src]);
    await git("add", "-A");
    await git("commit", "-qm", "one");
    const url = `file://${src}`;
    let [layer] = await loadLayers([url], { cacheDir: cache() });
    expect(layer.dir.startsWith(cache())).toBe(true);
    expect(layer.setup.files[0].data.toString()).toBe("one\n");
    writeFileSync(join(src, "claude/rules/a.md"), "two\n");
    await git("commit", "-qam", "two");
    [layer] = await loadLayers([url], { cacheDir: cache() });
    expect(readFileSync(join(layer.dir, "claude/rules/a.md"), "utf8")).toBe("two\n");
  });

  it("refuses a cycle and a chain deeper than 5", async () => {
    makeLayer(join(root, "a"), { name: "a", extends: ["../b"] });
    makeLayer(join(root, "b"), { name: "b", extends: ["../a"] });
    await expect(loadLayers([join(root, "a")], { cacheDir: cache() })).rejects.toThrow("extends cycle: a -> b -> a");
    for (let i = 0; i < 7; i++) makeLayer(join(root, `l${i}`), { name: `l${i}`, extends: i < 6 ? [`../l${i + 1}`] : [] });
    await expect(loadLayers([join(root, "l0")], { cacheDir: cache() })).rejects.toThrow("more than 5 levels");
    expect((await loadLayers([join(root, "l1")], { cacheDir: cache() })).map((l) => l.name)).toEqual(["l6", "l5", "l4", "l3", "l2", "l1"]);
  });

  it("loads a shared base once, in order", async () => {
    makeLayer(join(root, "d"), { name: "d" });
    makeLayer(join(root, "b"), { name: "b", extends: ["../d"] });
    makeLayer(join(root, "c"), { name: "c", extends: ["../d"] });
    makeLayer(join(root, "top"), { name: "top", extends: ["../b", "../c"] });
    expect((await loadLayers([join(root, "top")], { cacheDir: cache() })).map((l) => l.name)).toEqual(["d", "b", "c", "top"]);
  });

  it("names repeated layer names apart and fails clearly on a folder without sac.yaml", async () => {
    makeLayer(join(root, "x1"), { name: "same" });
    makeLayer(join(root, "x2"), { name: "same" });
    expect((await loadLayers([join(root, "x1"), join(root, "x2")], { cacheDir: cache() })).map((l) => l.name)).toEqual(["same", "same-2"]);
    await expect(loadLayers([join(root, "nope")], { cacheDir: cache() })).rejects.toThrow("is not a setup repo");
  });
});
