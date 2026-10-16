// Layer definitions. A layer is a top-level key of setup.yaml plus the collections inside it
// that the planner diffs item by item (keyed by `id`, or by the value itself for string lists).
// Adding a layer = add an entry here + a block in setup.schema.json + docs/layers.md.

export const LAYERS = [
  {
    id: 'identity',
    title: 'Identity',
    summary: 'Who the assistant acts for, and every account/credential it can use, by role. Names of secrets only.',
    collections: [{ path: 'identity.accounts', key: 'id' }],
  },
  {
    id: 'brain',
    title: 'Brain',
    summary: 'Models, runtimes, the roster of other AI workers, quality gates and budgets.',
    collections: [
      { path: 'brain.runtimes', key: 'id' },
      { path: 'brain.roster', key: 'id' },
      { path: 'brain.gates', key: 'id' },
    ],
    scalars: ['brain.claude_code'],
  },
  {
    id: 'instructions',
    title: 'Instructions',
    summary: 'Rules the assistant follows: id, scope, hard|standing, date, reason. Plus the files they come from.',
    collections: [
      { path: 'instructions.sources', key: 'id' },
      { path: 'instructions.rules', key: 'id' },
    ],
  },
  {
    id: 'tools',
    title: 'Tools',
    summary: 'MCP servers, skills, plugins, hooks, slash commands and the permission allow/deny/ask lists.',
    collections: [
      { path: 'tools.mcp_servers', key: 'id' },
      { path: 'tools.skills', key: 'id' },
      { path: 'tools.plugins', key: 'id' },
      { path: 'tools.hooks', key: 'id' },
      { path: 'tools.commands', key: 'id' },
      { path: 'tools.permissions.allow', key: null },
      { path: 'tools.permissions.deny', key: null },
      { path: 'tools.permissions.ask', key: null },
      { path: 'tools.denied_mcp_servers', key: null },
    ],
  },
  {
    id: 'loops',
    title: 'Loops',
    summary: 'Every scheduled/background job (launchd, cron, ...) including disabled and removed ones, with schedule, owner and success metric.',
    collections: [{ path: 'loops', key: 'id' }],
  },
  {
    id: 'channels',
    title: 'Channels',
    summary: 'Where the assistant talks and writes: chat, email, social, calendar, todo, journal.',
    collections: ['slack', 'email', 'social', 'calendar', 'todo', 'journal'].map((c) => ({ path: `channels.${c}`, key: 'id' })),
  },
  {
    id: 'projects',
    title: 'Projects',
    summary: 'Ongoing work the assistant knows about: products, repos, side projects, with status.',
    collections: [{ path: 'projects', key: 'id' }],
  },
  {
    id: 'business',
    title: 'Business lanes',
    summary: 'Revenue or outreach lanes: goal, metric, status, and which loops/channels serve them.',
    collections: [{ path: 'business.lanes', key: 'id' }],
  },
  {
    id: 'data',
    title: 'Data & state',
    summary: 'Memory stores, ledgers, logs and other state files, by path. Contents are never copied in.',
    collections: [
      { path: 'data.state', key: 'id' },
      { path: 'data.references', key: 'id' },
    ],
    scalars: ['data.memory'],
  },
  {
    id: 'infra',
    title: 'Infra',
    summary: 'Machine, installed CLIs, local repos and hosted services the setup depends on.',
    collections: [
      { path: 'infra.clis', key: 'id' },
      { path: 'infra.repos', key: 'id' },
      { path: 'infra.services', key: 'id' },
    ],
    scalars: ['infra.machine'],
  },
  {
    id: 'safety',
    title: 'Safety policy',
    summary: 'Machine-checkable policies (deny_mcp, no_payment_tools, forbidden_paths, required_gate, ...).',
    collections: [{ path: 'safety.policies', key: 'id' }],
  },
];

export const LAYER_IDS = LAYERS.map((l) => l.id);

import { getPath } from '../lib/util.mjs';

// Count items per layer (sum of its collections) and per collection.
export function countLayers(setup) {
  const out = {};
  for (const layer of LAYERS) {
    const per = {};
    let total = 0;
    for (const c of layer.collections) {
      const v = getPath(setup, c.path);
      const n = Array.isArray(v) ? v.length : 0;
      per[c.path.split('.').slice(1).join('.') || c.path] = n;
      total += n;
    }
    out[layer.id] = { total, ...per };
  }
  return out;
}
