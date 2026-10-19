import { existsSync, readFileSync } from "node:fs";
import { parse } from "dotenv";

/**
 * Values for the secrets a setup names, from the environment first and then the env file.
 * Only the names asked for are read, so unrelated variables never enter a run.
 */
export function secretsFrom(names: string[], opts: { env?: NodeJS.ProcessEnv; envFile?: string | null }): Record<string, string> {
  const env = opts.env ?? process.env;
  const file = opts.envFile && existsSync(opts.envFile) ? parse(readFileSync(opts.envFile, "utf8")) : {};
  const out: Record<string, string> = {};
  for (const n of names) {
    const v = env[n] || file[n];
    if (v) out[n] = v;
  }
  return out;
}
