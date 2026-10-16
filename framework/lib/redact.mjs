// Redact an instance into a shareable example that keeps SHAPE (layers, counts, kinds,
// severities, scopes, schedules, enabled flags) and drops every free-text value.
// Safe by construction: only allowlisted enum-like fields survive; ids become placeholders.
import { LAYERS } from '../layers/index.mjs';
import { getPath } from './util.mjs';

const KEEP = {
  'identity.accounts': ['kind', 'access'],
  'brain.runtimes': [],
  'brain.roster': [],
  'brain.gates': ['threshold'],
  'instructions.sources': ['kind'],
  'instructions.rules': ['scope', 'severity', 'date', 'status'],
  'tools.mcp_servers': ['transport', 'enabled'],
  'tools.skills': [],
  'tools.plugins': ['enabled', 'installed'],
  'tools.hooks': ['event'],
  'tools.commands': [],
  'loops': ['runner', 'enabled', 'managed', 'schedule'],
  'projects': ['status'],
  'business.lanes': ['status'],
  'data.state': ['kind'],
  'data.references': [],
  'infra.clis': ['present'],
  'infra.repos': [],
  'infra.services': [],
  'safety.policies': ['check', 'severity'],
};
const PREFIX = {
  'identity.accounts': 'account', 'instructions.rules': 'rule', 'instructions.sources': 'source', 'tools.mcp_servers': 'mcp',
  'tools.skills': 'skill', 'tools.plugins': 'plugin', 'tools.hooks': 'hook', 'tools.commands': 'command', loops: 'loop',
  projects: 'project', 'business.lanes': 'lane', 'data.state': 'state', 'data.references': 'ref', 'infra.repos': 'repo',
  'infra.services': 'service', 'brain.runtimes': 'runtime', 'brain.roster': 'worker', 'brain.gates': 'gate', 'safety.policies': 'policy',
};
const GENERIC_CLIS = /^(claude|node|npm|python3|git|gh|brew|docker|codex|gemini|wrangler|gcloud|firebase|eas|xcodebuild|ffmpeg|rtk|uv|bun|deno)$/;
const STATUS_WORDS = ['live', 'active', 'halted', 'killed', 'blocked', 'superseded', 'paused', 'planned', 'installed', 'running', 'loaded', 'parked', 'removed', 'not-loaded', 'backup-only', 'dead', 'frozen'];

function cleanStatus(s) {
  const t = String(s || '').toLowerCase();
  return STATUS_WORDS.find((w) => t.includes(w)) || (s ? 'other' : undefined);
}

function setPath(obj, p, v) {
  const ks = p.split('.');
  let o = obj;
  for (const k of ks.slice(0, -1)) o = o[k] ||= {};
  o[ks.at(-1)] = v;
}

export function redact(setup) {
  const out = { version: 1, meta: { instance: 'redacted', description: 'Shape-only redaction of a real setup: counts, kinds, severities and schedules kept; every name, path and text replaced.' } };
  for (const layer of LAYERS) {
    for (const c of layer.collections) {
      const list = getPath(setup, c.path);
      if (!Array.isArray(list) || !list.length) continue;
      if (!c.key) { setPath(out, c.path, list.map((_, i) => `${c.path.split('.').pop()}-entry-${String(i + 1).padStart(3, '0')}`)); continue; }
      const keep = KEEP[c.path] || [];
      const pre = PREFIX[c.path] || 'item';
      setPath(out, c.path, list.map((it, i) => {
        const r = { id: c.path === 'infra.clis' && GENERIC_CLIS.test(it.id) ? it.id : `${pre}-${String(i + 1).padStart(3, '0')}` };
        for (const k of keep) if (it[k] !== undefined) r[k] = k === 'schedule' ? redactSchedule(it[k]) : it[k];
        if (it.status !== undefined && c.path !== 'instructions.rules') r.status = cleanStatus(it.status);
        // Fill required fields with neutral placeholders so the example validates.
        if (c.path === 'instructions.rules') { r.reason = 'redacted'; r.scope ??= 'global'; r.severity ??= 'standing'; r.date ??= null; }
        if (c.path === 'identity.accounts') { r.kind ??= 'unknown'; r.access ??= 'unknown'; }
        if (c.path === 'tools.hooks') { r.command = 'redacted'; r.event ??= 'PreToolUse'; }
        if (c.path === 'brain.gates') r.applies_to = (it.applies_to || []).map(() => 'channel-kind');
        if (c.path === 'data.state') { r.path = '~/redacted'; r.kind ??= 'file'; }
        if (c.path === 'instructions.sources') r.path = '~/redacted';
        if (c.path === 'business.lanes') r.status = ['active', 'paused', 'halted', 'killed', 'blocked', 'planned'].includes(it.status) ? it.status : 'active';
        if (c.path === 'safety.policies') { r.check ??= 'custom'; r.severity ??= 'standing'; }
        if (c.path === 'loops') { r.runner ??= 'manual'; r.enabled ??= false; }
        return r;
      }));
    }
  }
  // channels: keep kind + active flag only
  for (const [kind, list] of Object.entries(setup.channels || {})) {
    if (!Array.isArray(list) || !list.length) continue;
    out.channels ||= {};
    out.channels[kind] = list.map((c, i) => ({ id: `${kind}-${String(i + 1).padStart(2, '0')}`, ...(c.status ? { status: cleanStatus(c.status) } : {}) }));
  }
  if (setup.data?.memory) out.data = { ...(out.data || {}), memory: { files: setup.data.memory.files, by_type: setup.data.memory.by_type } };
  return out;
}

function redactSchedule(s) {
  if (!s || typeof s !== 'object') return {};
  const r = {};
  if (s.every_seconds) r.every_seconds = s.every_seconds;
  if (s.calendar) r.calendar = s.calendar;
  if (s.keep_alive) r.keep_alive = true;
  if (s.run_at_load) r.run_at_load = true;
  if (s.cron) r.cron = s.cron;
  return r;
}
