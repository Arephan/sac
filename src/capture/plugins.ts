import { listMarketplaces, listPlugins, pluginOptions, type ClaudeCli, type KnownMarketplace } from "../claude-cli.js";
import type { Marketplace, PluginEntry } from "../types.js";

/** The source `claude plugin marketplace add` takes for a known marketplace. */
export function marketplaceSource(m: KnownMarketplace): { source: string; kind: Marketplace["kind"] } {
  if (m.source === "github" && m.repo) return { source: m.repo, kind: "github" };
  if (m.source === "git" && m.url) return { source: m.url, kind: "git" };
  if (m.source === "directory" && m.path) return { source: m.path, kind: "directory" };
  return { source: m.url ?? m.repo ?? m.path ?? m.source, kind: "other" };
}

export interface CapturedPlugins {
  marketplaces: Marketplace[];
  plugins: PluginEntry[];
  /** plugin id -> option keys its manifest marks sensitive */
  sensitive: Record<string, string[]>;
}

/** User-scope plugins only: project, local and synced plugins belong to other places. */
export async function readPlugins(cli: ClaudeCli): Promise<CapturedPlugins> {
  const marketplaces = (await listMarketplaces(cli)).map((m) => ({ name: m.name, ...marketplaceSource(m) }));
  const plugins: PluginEntry[] = [];
  const sensitive: Record<string, string[]> = {};
  for (const p of (await listPlugins(cli)).filter((x) => x.scope === "user")) {
    const opts = await pluginOptions(cli, p.id);
    const settings: Record<string, string> = {};
    for (const [k, v] of Object.entries(opts.inputs)) {
      if (v === undefined || v === null) continue;
      settings[k] = typeof v === "string" ? v : JSON.stringify(v);
    }
    const keys = Object.entries(opts.schema).filter(([, s]) => s?.sensitive).map(([k]) => k);
    if (keys.length) sensitive[p.id] = keys;
    plugins.push({ id: p.id, enabled: p.enabled, settings });
  }
  return { marketplaces, plugins, sensitive };
}
