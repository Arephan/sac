#!/usr/bin/env node
// Stand-in for the `claude` CLI. State lives beside the Claude home it serves:
// $CLAUDE_CONFIG_DIR/.sac-fake-state.json, else $HOME/.sac-fake-state.json.
// MCP servers live where Claude Code keeps them: <config dir or HOME>/.claude.json.
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";

const argv = process.argv.slice(2);
const base = process.env.CLAUDE_CONFIG_DIR ?? process.env.HOME;
const stateFile = join(base, ".sac-fake-state.json");
const state = existsSync(stateFile)
  ? JSON.parse(readFileSync(stateFile, "utf8"))
  : { version: "2.1.301 (Claude Code)", marketplaces: [], plugins: [], configure: {}, mcpStatus: {} };
const save = () => writeFileSync(stateFile, JSON.stringify(state, null, 2));
const cj = join(base, ".claude.json");
const readCj = () => (existsSync(cj) ? JSON.parse(readFileSync(cj, "utf8")) : {});
const stdin = argv.includes("--values-stdin") ? readFileSync(0, "utf8") : undefined;
appendFileSync(join(process.env.HOME, ".sac-fake-log.jsonl"),
  JSON.stringify({ bin: "claude", argv, stdin, configDir: process.env.CLAUDE_CONFIG_DIR ?? null }) + "\n");
const out = (o) => process.stdout.write(typeof o === "string" ? o : JSON.stringify(o));
const pos = argv.filter((a, i) => !a.startsWith("-") && !["-s", "--scope"].includes(argv[i - 1]));
const [a0, a1, a2, a3] = pos;
const REGISTRY = {
  "anthropics/claude-plugins-official": "claude-plugins-official",
  "https://git.example.invalid/acme/plugins.git": "acme",
};

if (argv[0] === "--version") { out(`${state.version}\n`); process.exit(0); }
if (a0 === "plugin" && a1 === "list") {
  out(state.plugins.map((p) => ({ version: "1.0.0", installPath: "/x", ...p })));
  process.exit(0);
}
if (a0 === "plugin" && a1 === "marketplace" && a2 === "list") { out(state.marketplaces); process.exit(0); }
if (a0 === "plugin" && a1 === "marketplace" && a2 === "add") {
  const src = a3;
  let entry;
  if (src.startsWith("/")) {
    if (!existsSync(src)) { process.stderr.write(`Error: directory ${src} does not exist\n`); process.exit(1); }
    const manifest = join(src, ".claude-plugin", "marketplace.json");
    const name = existsSync(manifest) ? JSON.parse(readFileSync(manifest, "utf8")).name : basename(src);
    entry = { name, source: "directory", path: src };
  } else if (/^[\w.-]+\/[\w.-]+$/.test(src)) {
    entry = { name: REGISTRY[src] ?? src.split("/")[1], source: "github", repo: src };
  } else {
    entry = { name: REGISTRY[src] ?? basename(src).replace(/\.git$/, ""), source: "git", url: src };
  }
  if (!state.marketplaces.some((m) => m.name === entry.name)) state.marketplaces.push(entry);
  save();
  process.exit(0);
}
if (a0 === "plugin" && a1 === "install") {
  const id = a2;
  const mk = id.split("@")[1];
  if (!state.marketplaces.some((m) => m.name === mk)) { process.stderr.write(`Error: marketplace ${mk} not found\n`); process.exit(1); }
  if (!state.plugins.some((p) => p.id === id && p.scope === "user")) state.plugins.push({ id, scope: "user", enabled: true });
  save();
  process.exit(0);
}
if (a0 === "plugin" && (a1 === "enable" || a1 === "disable")) {
  const p = state.plugins.find((x) => x.id === a2 && x.scope === "user");
  if (!p) { process.stderr.write(`Error: plugin ${a2} is not installed\n`); process.exit(1); }
  p.enabled = a1 === "enable";
  save();
  process.exit(0);
}
if (a0 === "plugin" && a1 === "configure") {
  const id = a2;
  state.configure[id] ??= { pluginId: id, schema: {}, inputs: {} };
  if (stdin !== undefined) { Object.assign(state.configure[id].inputs, JSON.parse(stdin)); save(); process.exit(0); }
  out({ displayName: id, choices: {}, configured: [], unconfigured: [], ...state.configure[id] });
  process.exit(0);
}
if (a0 === "mcp" && a1 === "add-json") {
  const d = readCj();
  d.mcpServers ??= {};
  d.mcpServers[a2] = JSON.parse(a3);
  writeFileSync(cj, JSON.stringify(d, null, 2));
  process.exit(0);
}
if (a0 === "mcp" && a1 === "remove") {
  const d = readCj();
  if (!d.mcpServers?.[a2]) { process.stderr.write(`No MCP server named ${a2}\n`); process.exit(1); }
  delete d.mcpServers[a2];
  writeFileSync(cj, JSON.stringify(d, null, 2));
  process.exit(0);
}
if (a0 === "mcp" && a1 === "list") {
  const d = readCj();
  out("Checking MCP server health...\n\n");
  for (const [n, s] of Object.entries(d.mcpServers ?? {})) out(`${n}: ${s.command ?? s.url} - ${state.mcpStatus?.[n] ?? "✓ Connected"}\n`);
  process.exit(0);
}
process.stderr.write(`fake claude: unsupported ${argv.join(" ")}\n`);
process.exit(2);
