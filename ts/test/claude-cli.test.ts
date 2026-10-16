import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { claudeCli, claudeVersion, listMarketplaces, listPlugins, pluginOptions, tool } from "../src/claude-cli.js";
import { targetFor } from "../src/paths.js";
import { fixtureHome, type FixtureHome } from "./helpers/home.js";

describe("claude CLI wrapper", () => {
  let fx: FixtureHome;
  beforeEach(() => { fx = fixtureHome(); });
  afterEach(() => fx.restore());

  it("reads version, plugins, marketplaces and plugin options", async () => {
    const cli = claudeCli(fx.target);
    expect(await claudeVersion(cli)).toBe("2.1.301");
    expect((await listPlugins(cli)).map((p) => `${p.id}:${p.scope}:${p.enabled}`)).toContain("posthog@claude-plugins-official:user:false");
    expect((await listMarketplaces(cli)).map((m) => m.name)).toEqual(["claude-plugins-official", "acme", "local"]);
    expect((await pluginOptions(cli, "deploy@acme")).schema.apiKey).toEqual({ type: "string", sensitive: true });
    expect(await pluginOptions(cli, "superpowers@claude-plugins-official")).toEqual({ schema: {}, inputs: {} });
  });

  it("passes CLAUDE_CONFIG_DIR only for a config-dir target", async () => {
    await claudeCli(targetFor({ home: fx.home, configDir: join(fx.home, "alt") })).run(["mcp", "list"]);
    await claudeCli(fx.target).run(["mcp", "list"]);
    expect(fx.calls().map((c) => c.configDir)).toEqual([join(fx.home, "alt"), null]);
  });

  it("reports a failing command instead of throwing, and a missing program as exit 127", async () => {
    expect((await claudeCli(fx.target).run(["nonsense"])).code).toBe(2);
    expect((await claudeCli(fx.target, "/nonexistent/claude").run(["--version"])).code).toBe(127);
    expect(await claudeVersion(claudeCli(fx.target, "/nonexistent/claude"))).toBeNull();
  });

  it("runs other tools with the target's HOME", async () => {
    const r = await tool(fx.target, process.env.SAC_CRONTAB_BIN!).run(["-l"]);
    expect(r.code).toBe(1);
    expect(fx.calls().at(-1)).toMatchObject({ bin: "crontab", argv: ["-l"] });
  });
});
