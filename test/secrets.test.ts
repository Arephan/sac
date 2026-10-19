import { describe, expect, it } from "vitest";
import {
  extractSecrets, fillPlaceholders, findSecretSpans, looksSecret, ph, placeholdersIn, redact, scanText, scrub, SECRET_KEY, SecretVault,
} from "../src/secrets.js";
import { FAKE } from "./helpers/secrets.js";

describe("detection", () => {
  it("recognises token shapes and leaves prose alone", async () => {
    expect(await looksSecret(FAKE.SLACK_BOT)).toBe(true);
    expect(await looksSecret(FAKE.GH_TOKEN)).toBe(true);
    expect(await looksSecret("info")).toBe(false);
    expect(await looksSecret("https://mcp.linear.app/mcp")).toBe(false);
  });
  it("finds the span of a token inside a longer string", async () => {
    const s = `curl -H 'x: ${FAKE.SLACK_BOT}' https://example.com`;
    const [[a, b]] = await findSecretSpans(s);
    expect(s.slice(a, b)).toBe(FAKE.SLACK_BOT);
  });
  it("knows credential-looking keys", () => {
    for (const k of ["GITHUB_TOKEN", "Authorization", "X-Api-Key", "OPENAI_API_KEY", "STRIPE_KEY", "DB_PASSWORD", "client_secret", "GH_AUTH"]) expect(SECRET_KEY.test(k)).toBe(true);
    for (const k of ["LOG_LEVEL", "AUTHOR_NAME", "TZ", "PATH", "KEYBOARD"]) expect(SECRET_KEY.test(k), k).toBe(false);
  });
  it("scans text and reports lines without the value", async () => {
    const found = await scanText(`line one\nexport T=${FAKE.SLACK_BOT}\n`);
    expect(found).toEqual([{ line: 2, rule: "slack" }]);
    expect(await scanText("nothing here\n")).toEqual([]);
  });
});

describe("redact", () => {
  it("redacts MCP servers by key, by shape and after a credential flag", async () => {
    const vault = new SecretVault();
    const mcp = {
      github: { command: "/x", env: { GITHUB_PERSONAL_ACCESS_TOKEN: FAKE.GH_MCP, LOG_LEVEL: "debug" } },
      linear: { url: "https://mcp.linear.app/mcp", headers: { Authorization: FAKE.LINEAR } },
      slackbot: { command: "npx", args: ["slack-mcp", "--token", "short-but-flagged"] },
    };
    expect(await redact(mcp, ["mcp"], vault)).toEqual({
      github: { command: "/x", env: { GITHUB_PERSONAL_ACCESS_TOKEN: ph("GITHUB_PERSONAL_ACCESS_TOKEN"), LOG_LEVEL: "debug" } },
      linear: { url: "https://mcp.linear.app/mcp", headers: { Authorization: ph("LINEAR_AUTHORIZATION") } },
      slackbot: { command: "npx", args: ["slack-mcp", "--token", ph("SLACKBOT_ARGS_2")] },
    });
  });

  it("redacts settings env by key or shape, and nothing outside env by key", async () => {
    const vault = new SecretVault();
    const settings = {
      apiKeyHelper: "~/bin/get-key.sh",
      env: { DEPLOY_HOOK: FAKE.SLACK_BOT, GITHUB_TOKEN: FAKE.GH_TOKEN, LOG_LEVEL: "info", AUTHOR_NAME: "Sam" },
      hooks: { Stop: [{ hooks: [{ type: "command", command: "curl -H \"Authorization: Bearer ${API_TOKEN}\" https://x" }] }] },
    };
    const out = await redact(settings, ["settings"], vault);
    expect(out.apiKeyHelper).toBe("~/bin/get-key.sh");
    expect(out.env).toEqual({ DEPLOY_HOOK: ph("DEPLOY_HOOK"), GITHUB_TOKEN: ph("GITHUB_TOKEN"), LOG_LEVEL: "info", AUTHOR_NAME: "Sam" });
    expect(out.hooks).toEqual(settings.hooks);
    expect(placeholdersIn(out.hooks)).toEqual([]);
  });

  it("replaces only the token inside a longer value", async () => {
    const vault = new SecretVault();
    const out = await redact({ x: { program: ["/bin/sh", "-c", `post --hook ${FAKE.SLACK_JOB} now`] } }, ["jobs"], vault);
    expect(out.x.program[2]).toBe(`post --hook ${ph("X_PROGRAM_2")} now`);
  });

  it("redacts sensitive plugin keys whatever their shape", async () => {
    const vault = new SecretVault();
    expect(await redact({ region: "eu", apiKey: "short" }, ["plugins", "deploy@acme"], vault, { sensitiveKeys: new Set(["apiKey"]), byKey: "all" }))
      .toEqual({ region: "eu", apiKey: ph("DEPLOY_APIKEY") });
  });

  it("reuses a name for the same value and suffixes a clash", () => {
    const v = new SecretVault();
    expect(v.name(["settings", "env", "TOKEN"], "a")).toBe("TOKEN");
    expect(v.name(["jobs", "x", "env", "TOKEN"], "a")).toBe("TOKEN");
    expect(v.name(["jobs", "y", "env", "TOKEN"], "b")).toBe("TOKEN_2");
    expect(v.name(["mcp", "my-server", "env", "api-key"], "c")).toBe("API_KEY");
    expect(v.refs()).toEqual([
      { name: "TOKEN", where: "settings.env.TOKEN" },
      { name: "TOKEN_2", where: "jobs.y.env.TOKEN" },
      { name: "API_KEY", where: "mcp.my-server.env.api-key" },
    ]);
    expect(v.values()).toEqual({ TOKEN: "a", TOKEN_2: "b", API_KEY: "c" });
  });
});

describe("placeholders", () => {
  it("finds and fills placeholders, reporting what is missing", () => {
    const v = { a: ph("X"), b: [`pre ${ph("Y")} post`, ph("X")], c: 1, d: "${NOT_OURS}" };
    expect(placeholdersIn(v)).toEqual(["X", "Y"]);
    expect(fillPlaceholders(v, { X: "1" })).toEqual({ value: { a: "1", b: [`pre ${ph("Y")} post`, "1"], c: 1, d: "${NOT_OURS}" }, missing: ["Y"] });
  });

  it("learns secret values from a machine that already has the item", () => {
    const template = { a: `Bearer ${ph("T")}`, b: [ph("U")], c: "x", d: { e: ph("V") } };
    expect(extractSecrets(template, { a: "Bearer abc", b: ["u1"], c: "y", d: {} })).toEqual({ T: "abc", U: "u1" });
    expect(extractSecrets(template, { a: "Basic abc" })).toEqual({});
    expect(extractSecrets(template, null)).toEqual({});
  });

  it("scrubs known values out of text", () => {
    expect(scrub(`failed: token ${FAKE.GH_TOKEN} rejected`, [FAKE.GH_TOKEN, ""])).toBe("failed: token *** rejected");
  });
});
