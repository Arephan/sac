// Plan: diff desired (setup.yaml) against live (fresh import). Pure; writes nothing.
import { LAYERS } from '../layers/index.mjs';
import { getPath, stableStringify, isPlainObject } from './util.mjs';

// Fields that change on their own and say nothing about configuration.
const VOLATILE = new Set(['generated_at', 'last_commit']);
const STATUS_EQUIV = { running: 'loaded' };

function clean(v) {
  if (Array.isArray(v)) return v.map(clean);
  if (isPlainObject(v)) {
    const out = {};
    for (const [k, x] of Object.entries(v)) {
      if (VOLATILE.has(k)) continue;
      out[k] = k === 'status' && STATUS_EQUIV[x] ? STATUS_EQUIV[x] : clean(x);
    }
    return out;
  }
  return v;
}

function changedFields(a, b) {
  const keys = new Set([...Object.keys(a || {}), ...Object.keys(b || {})]);
  return [...keys].filter((k) => stableStringify(clean(a?.[k])) !== stableStringify(clean(b?.[k])));
}

export function plan(desired, live) {
  const layers = [];
  for (const layer of LAYERS) {
    const changes = [];
    for (const c of layer.collections) {
      const d = getPath(desired, c.path) || [];
      const l = getPath(live, c.path) || [];
      if (!Array.isArray(d) || !Array.isArray(l)) continue;
      if (!c.key) {
        const ds = new Set(d), ls = new Set(l);
        for (const x of d) if (!ls.has(x)) changes.push({ op: '+', path: c.path, id: x });
        for (const x of l) if (!ds.has(x)) changes.push({ op: '-', path: c.path, id: x });
        continue;
      }
      const dm = new Map(d.map((x) => [x[c.key], x]));
      const lm = new Map(l.map((x) => [x[c.key], x]));
      for (const [id, x] of dm) {
        if (!lm.has(id)) changes.push({ op: '+', path: c.path, id });
        else {
          const f = changedFields(x, lm.get(id));
          if (f.length) changes.push({ op: '~', path: c.path, id, fields: f });
        }
      }
      for (const id of lm.keys()) if (!dm.has(id)) changes.push({ op: '-', path: c.path, id });
    }
    for (const s of layer.scalars || []) {
      const f = changedFields(getPath(desired, s) || {}, getPath(live, s) || {});
      if (f.length) changes.push({ op: '~', path: s, id: s.split('.').pop(), fields: f });
    }
    layers.push({ layer: layer.id, changes });
  }
  const all = layers.flatMap((l) => l.changes);
  return {
    layers,
    totals: { add: all.filter((c) => c.op === '+').length, change: all.filter((c) => c.op === '~').length, untracked: all.filter((c) => c.op === '-').length },
  };
}

export function formatPlan(p, { verbose = false, limit = 12 } = {}) {
  const lines = [];
  for (const { layer, changes } of p.layers) {
    if (!changes.length) { lines.push(`  ${layer.padEnd(13)} no changes`); continue; }
    const n = (op) => changes.filter((c) => c.op === op).length;
    lines.push(`  ${layer.padEnd(13)} +${n('+')} ~${n('~')} -${n('-')}`);
    const shown = verbose ? changes : changes.slice(0, limit);
    for (const c of shown) lines.push(`      ${c.op} ${c.path} ${c.id}${c.fields ? ` (${c.fields.join(', ')})` : ''}`);
    if (shown.length < changes.length) lines.push(`      … ${changes.length - shown.length} more (use --verbose)`);
  }
  lines.push('');
  lines.push(`  + = in desired, not on the machine   ~ = differs   - = on the machine, not in desired`);
  lines.push(`  Plan: ${p.totals.add} to add, ${p.totals.change} to change, ${p.totals.untracked} untracked. Nothing was written.`);
  return lines.join('\n');
}
