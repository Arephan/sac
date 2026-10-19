import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { readUserMcp } from "../src/capture/mcp.js";
import { marketplaceSource, readPlugins } from "../src/capture/plugins.js";
import { claudeCli } from "../src/claude-cli.js";
import { fixtureHome, type FixtureHome } from "./helpers/home.js";
import { FAKE } from "./helpers/secrets.js";

describe("MCP servers", () => {
  let fx: FixtureHome;
  beforeEach(() => { fx = fixtureHome(); });
  afterEach(() => fx.restore());

  it("reads only the user-scope servers", () => {
    const m = readUserMcp(fx.target);
    expect(Object.keys(m)).toEqual(["github", "linear", "slackbot"]);
    expect((m.github as { env: Record<string, string> }).env.GITHUB_PERSONAL_ACCESS_TOKEN).toBe(FAKE.GH_MCP);
  });

  it("returns nothing for a missing or broken file", () => {
    writeFileSync(fx.target.claudeJson, "{ not json");
    expect(readUserMcp(fx.target)).toEqual({});
    const empty = fixtureHome(null);
    try {
      expect(readUserMcp(empty.target)).toEqual({});
    } finally {
      empty.restore();
    }
  });
});

describe("plugins", () => {
  let fx: FixtureHome;
  beforeEach(() => { fx = fixtureHome(); });
  afterEach(() => fx.restore());

  it("reads marketplaces as add-able sources and user plugins with their options", async () => {
    const r = await readPlugins(claudeCli(fx.target));
    expect(r.marketplaces).toEqual([
      { name: "claude-plugins-official", source: "anthropics/claude-plugins-official", kind: "github" },
      { name: "acme", source: "https://git.example.invalid/acme/plugins.git", kind: "git" },
      { name: "local", source: join(fx.home, "my-plugins"), kind: "directory" },
    ]);
    expect(r.plugins).toEqual([
      { id: "superpowers@claude-plugins-official", enabled: true, settings: {} },
      { id: "posthog@claude-plugins-official", enabled: false, settings: {} },
      { id: "deploy@acme", enabled: true, settings: { region: "eu-west-1", apiKey: FAKE.DEPLOY_KEY } },
    ]);
    expect(r.sensitive).toEqual({ "deploy@acme": ["apiKey"] });
  });

  it("maps unknown marketplace kinds to 'other'", () => {
    expect(marketplaceSource({ name: "x", source: "url", url: "https://x/m.json" })).toEqual({ source: "https://x/m.json", kind: "other" });
  });
});
