import { chmodSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { targetFor } from "../../src/paths.js";
import type { Target } from "../../src/types.js";
import { FAKE } from "./secrets.js";

const FIX = fileURLToPath(new URL("../fixtures/", import.meta.url));
const bin = (name: string) => fileURLToPath(new URL(`./${name}`, import.meta.url));

/** Replaces __HOME__ and __<FAKE NAME>__ markers in fixture text. */
export function fill(text: string, home: string): string {
  let t = text.split("__HOME__").join(home);
  for (const [k, v] of Object.entries(FAKE)) t = t.split(`__${k}__`).join(v);
  return t;
}

function fillTree(dir: string, home: string): void {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) fillTree(p, home);
    else if (e.isFile()) {
      const buf = readFileSync(p);
      if (buf.includes(0)) continue;
      const mode = statSync(p).mode;
      writeFileSync(p, fill(buf.toString("utf8"), home));
      chmodSync(p, mode);
    }
  }
}

export interface FakeCall { bin: "claude" | "crontab" | "launchctl"; argv: string[]; stdin?: string; configDir: string | null }

export interface FixtureHome {
  home: string;
  target: Target;
  /** The fake claude's state for this home, or for a config dir inside it. */
  state(configDir?: string): any;
  setState(change: (s: any) => void, configDir?: string): void;
  calls(): FakeCall[];
  clearCalls(): void;
  restore(): void;
}

const ENV_KEYS = ["HOME", "SAC_HOME", "SAC_CLAUDE_BIN", "SAC_CRONTAB_BIN", "SAC_LAUNCHCTL_BIN", "CLAUDE_CONFIG_DIR"] as const;

/** A scratch HOME, filled from a fixture (or empty), with fake claude, crontab and launchctl wired in. */
export function fixtureHome(name: string | null = "home-a"): FixtureHome {
  const home = realpathSync(mkdtempSync(join(tmpdir(), "sac-home-")));
  if (name) {
    cpSync(join(FIX, name), home, { recursive: true });
    fillTree(home, home);
    const state = join(FIX, `${name}.state.json`);
    if (existsSync(state)) writeFileSync(join(home, ".sac-fake-state.json"), fill(readFileSync(state, "utf8"), home));
  }
  mkdirSync(join(home, ".claude"), { recursive: true });
  const saved = Object.fromEntries(ENV_KEYS.map((k) => [k, process.env[k]]));
  Object.assign(process.env, {
    HOME: home,
    SAC_HOME: join(home, ".sac"),
    SAC_CLAUDE_BIN: bin("fake-claude.mjs"),
    SAC_CRONTAB_BIN: bin("fake-crontab.mjs"),
    SAC_LAUNCHCTL_BIN: bin("fake-launchctl.mjs"),
  });
  delete process.env.CLAUDE_CONFIG_DIR;
  const stateFile = (configDir?: string) => join(configDir ?? home, ".sac-fake-state.json");
  const log = join(home, ".sac-fake-log.jsonl");
  return {
    home,
    target: targetFor({ home }),
    state: (configDir) => (existsSync(stateFile(configDir)) ? JSON.parse(readFileSync(stateFile(configDir), "utf8")) : null),
    setState(change, configDir) {
      const f = stateFile(configDir);
      const s = existsSync(f) ? JSON.parse(readFileSync(f, "utf8")) : { version: "2.1.301 (Claude Code)", marketplaces: [], plugins: [], configure: {}, mcpStatus: {} };
      change(s);
      writeFileSync(f, JSON.stringify(s, null, 2));
    },
    calls: () => (existsSync(log) ? readFileSync(log, "utf8").trim().split("\n").filter(Boolean).map((l) => JSON.parse(l)) : []),
    clearCalls: () => rmSync(log, { force: true }),
    restore() {
      for (const [k, v] of Object.entries(saved)) {
        if (v === undefined) delete process.env[k];
        else process.env[k] = v;
      }
      rmSync(home, { recursive: true, force: true });
    },
  };
}
