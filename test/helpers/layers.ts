import { setupToRepo, writeRepo } from "../../src/repo.js";
import type { SavedFile, Setup } from "../../src/types.js";

export const file = (path: string, text: string, mode = 0o644): SavedFile => ({ path, data: Buffer.from(text), mode });
export const settingsFile = (v: unknown) => file("settings.json", `${JSON.stringify(v, null, 2)}\n`);

/** Writes a setup repo folder from a partial setup. */
export function makeLayer(dir: string, s: Partial<Setup> & { name: string }): string {
  writeRepo(dir, setupToRepo({ extends: [], files: [], mcp: {}, marketplaces: [], plugins: [], jobs: [], ...s }, []));
  return dir;
}
