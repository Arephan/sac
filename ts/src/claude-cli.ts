import { execa } from "execa";
import type { Target } from "./types.js";

export interface RunResult {
  code: number;
  stdout: string;
  stderr: string;
}

export interface ClaudeCli {
  run(args: string[], input?: string): Promise<RunResult>;
}

/**
 * Runs a program against a target: HOME is the target's home, and CLAUDE_CONFIG_DIR is set only for
 * a config-dir target. Never throws on a failed command; a missing program is exit code 127.
 */
export function tool(t: Target, bin: string, timeout = 300_000): ClaudeCli {
  return {
    async run(args, input) {
      const env: Record<string, string | undefined> = { ...process.env, HOME: t.home };
      if (t.configDir) env.CLAUDE_CONFIG_DIR = t.configDir;
      else delete env.CLAUDE_CONFIG_DIR;
      const r = await execa(bin, args, { env, extendEnv: false, input: input ?? "", reject: false, timeout });
      const code = typeof r.exitCode === "number" ? r.exitCode : (r as { code?: string }).code === "ENOENT" ? 127 : 1;
      return { code, stdout: String(r.stdout ?? ""), stderr: String(r.stderr ?? "") };
    },
  };
}

export function claudeCli(t: Target, bin = process.env.SAC_CLAUDE_BIN ?? "claude"): ClaudeCli {
  return tool(t, bin);
}

async function json<T>(cli: ClaudeCli, args: string[], fallback: T): Promise<T> {
  const r = await cli.run(args);
  if (r.code !== 0) return fallback;
  try {
    return JSON.parse(r.stdout) as T;
  } catch {
    return fallback;
  }
}

export interface InstalledPlugin {
  id: string;
  scope: string;
  enabled: boolean;
}

export async function listPlugins(cli: ClaudeCli): Promise<InstalledPlugin[]> {
  const raw = await json<Array<{ id: string; scope?: string; enabled?: boolean }>>(cli, ["plugin", "list", "--json"], []);
  return raw.map((p) => ({ id: p.id, scope: p.scope ?? "user", enabled: p.enabled !== false }));
}

export interface KnownMarketplace {
  name: string;
  source: string;
  repo?: string;
  url?: string;
  path?: string;
}

export async function listMarketplaces(cli: ClaudeCli): Promise<KnownMarketplace[]> {
  return json<KnownMarketplace[]>(cli, ["plugin", "marketplace", "list", "--json"], []);
}

export interface PluginOptions {
  schema: Record<string, { sensitive?: boolean; [k: string]: unknown }>;
  inputs: Record<string, unknown>;
}

export async function pluginOptions(cli: ClaudeCli, id: string): Promise<PluginOptions> {
  const r = await json<Partial<PluginOptions>>(cli, ["plugin", "configure", id, "--json"], {});
  return { schema: r.schema ?? {}, inputs: r.inputs ?? {} };
}

export async function claudeVersion(cli: ClaudeCli): Promise<string | null> {
  const r = await cli.run(["--version"]);
  return r.code === 0 ? (/(\d+\.\d+\.\d+)/.exec(r.stdout)?.[1] ?? null) : null;
}
