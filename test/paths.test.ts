import { describe, expect, it } from "vitest";
import { deepMapStrings, detokenize, expandTilde, targetFor, tokenize } from "../src/paths.js";

const t = { home: "/Users/sam", claudeDir: "/Users/sam/.claude" };

describe("targetFor", () => {
  it("resolves the default target and a config dir", () => {
    expect(targetFor({ home: "/h", platform: "linux" })).toEqual({
      home: "/h", claudeDir: "/h/.claude", claudeJson: "/h/.claude.json", configDir: null, platform: "linux",
    });
    expect(targetFor({ home: "/h", configDir: "/c", platform: "darwin" })).toMatchObject({
      claudeDir: "/c", claudeJson: "/c/.claude.json", configDir: "/c",
    });
  });
});

describe("path tokens", () => {
  it("replaces the Claude folder and home as whole path prefixes", () => {
    expect(tokenize("/Users/sam/.claude/hooks/x.sh --q", t)).toBe("{{claude}}/hooks/x.sh --q");
    expect(tokenize("cd /Users/sam && ls /Users/samantha /opt/Users/sam", t)).toBe("cd {{home}} && ls /Users/samantha /opt/Users/sam");
    expect(tokenize("PATH=/Users/sam/bin:/Users/sam/.local/bin", t)).toBe("PATH={{home}}/bin:{{home}}/.local/bin");
    expect(tokenize('"/Users/sam/.claude.json"', t)).toBe('"{{home}}/.claude.json"');
    expect(tokenize("file:///Users/sam/x", t)).toBe("file://{{home}}/x");
  });
  it("never touches a tilde the user wrote", () => {
    expect(tokenize("~/.claude/x.sh and ~", t)).toBe("~/.claude/x.sh and ~");
  });
  it("round-trips onto another machine", () => {
    const other = { home: "/home/bo", claudeDir: "/tmp/alt" };
    expect(detokenize(tokenize("/Users/sam/.claude/a /Users/sam/b", t), other)).toBe("/tmp/alt/a /home/bo/b");
  });
  it("expands only a leading tilde", () => {
    expect(expandTilde("~", "/Users/sam")).toBe("/Users/sam");
    expect(expandTilde("~/x", "/Users/sam")).toBe("/Users/sam/x");
    expect(expandTilde("a~/x", "/Users/sam")).toBe("a~/x");
    expect(expandTilde("~bob/x", "/Users/sam")).toBe("~bob/x");
  });
});

describe("deepMapStrings", () => {
  it("maps every string with its path", () => {
    expect(deepMapStrings({ a: ["x", { b: "y" }], n: 1 }, (s, p) => `${s}:${p.join(".")}`)).toEqual({ a: ["x:a.0", { b: "y:a.1.b" }], n: 1 });
  });
});
