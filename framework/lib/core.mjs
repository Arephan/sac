// Core: locate an instance, load desired state, import live state, normalize, validate.
import fs from 'node:fs';
import path from 'node:path';
import Ajv from 'ajv';
import { FRAMEWORK_ROOT, readYaml, deepMerge, fileExists, resolveVars, isPlainObject, getPath } from './util.mjs';
import { makeContext } from './context.mjs';
import { loadImporters } from '../importers/index.mjs';
import { LAYERS } from '../layers/index.mjs';

const SCHEMA = JSON.parse(fs.readFileSync(path.join(FRAMEWORK_ROOT, 'framework/schema/setup.schema.json'), 'utf8'));

// ---------- instance ----------
export function resolveInstance(flag) {
  const cands = [];
  if (flag) cands.push(path.resolve(flag));
  else if (process.env.SETUP_INSTANCE) cands.push(path.resolve(process.env.SETUP_INSTANCE));
  else {
    if (fileExists(path.join(process.cwd(), 'setup.yaml'))) cands.push(process.cwd());
    const idir = path.join(FRAMEWORK_ROOT, 'instance');
    const subs = fileExists(idir) ? fs.readdirSync(idir).filter((d) => fs.statSync(path.join(idir, d)).isDirectory()) : [];
    if (subs.length === 1) cands.push(path.join(idir, subs[0]));
  }
  const dir = cands[0];
  if (!dir) throw new Error('no instance found: pass --instance <dir> (a folder holding setup.yaml), or run `init <dir>` first');
  // A path to setup.yaml itself is accepted too.
  if (dir.endsWith('.yaml') && fileExists(dir)) return { dir: path.dirname(dir), file: dir };
  return { dir, file: path.join(dir, 'setup.yaml') };
}

function optionalYaml(file) { return fileExists(file) ? readYaml(file) : {}; }

export function loadInstance(inst) {
  if (!fileExists(inst.file)) throw new Error(`${inst.file} not found (run \`import\` or \`init\`)`);
  const setup = readYaml(inst.file);
  const overrides = optionalYaml(path.join(inst.dir, 'overrides.yaml'));
  const policies = optionalYaml(path.join(inst.dir, 'policies.yaml'));
  return { setup, overrides, policies };
}

// Desired = setup.yaml + policies.yaml, with ${env:X}/${vars.X} resolved.
export function desiredState({ setup, policies }) {
  const merged = deepMerge(setup, policies.safety ? { safety: policies.safety } : {});
  return normalize(resolveVars(merged, merged.vars || {}));
}

// ---------- import ----------
// Collection/scalar paths an importer fragment actually observed (arrays/objects present).
function observedPaths(frag) {
  const seen = new Set();
  for (const layer of LAYERS) {
    for (const c of layer.collections) if (Array.isArray(getPath(frag, c.path))) seen.add(c.path);
    for (const s of layer.scalars || []) if (getPath(frag, s) !== undefined) seen.add(s);
  }
  return seen;
}

function setPath(obj, p, v) {
  const ks = p.split('.');
  let o = obj;
  for (const k of ks.slice(0, -1)) { if (!isPlainObject(o[k])) o[k] = {}; o = o[k]; }
  o[ks.at(-1)] = v;
}

// Read the machine. `baseline` supplies collections no importer could observe (gates,
// hand-written rules, lanes...): those are copied through, so they never show as drift and
// `import` never wipes them.
export async function importLive({ inst, setup = {}, overrides = {}, policies = {}, home, baseline = setup, log = () => {} }) {
  const cfg = setup.import || overrides.import || {};
  const options = cfg.options || {};
  const ctx = makeContext({ home, options, log });
  const importers = await loadImporters(inst?.dir);
  const wanted = !cfg.importers || cfg.importers === 'all' ? importers.map((i) => i.id) : cfg.importers;
  let live = {};
  const ran = [];
  const observed = new Set();
  for (const imp of importers) {
    if (!wanted.includes(imp.id)) continue;
    const opts = { ...(options[imp.id] || {}) };
    if (imp.id === 'file-state') opts.declared = overrides.data?.state || setup.data?.state || [];
    let detected = false;
    try { detected = imp.detect ? await imp.detect(ctx) : true; } catch { detected = false; }
    if (!detected) { ran.push({ id: imp.id, status: 'not-detected' }); continue; }
    try {
      const frag = await imp.run(ctx, opts);
      delete frag._sources;
      for (const p of observedPaths(frag)) observed.add(p);
      live = deepMerge(live, frag);
      ran.push({ id: imp.id, status: 'ok' });
      log(`  ok  ${imp.id}`);
    } catch (e) {
      ran.push({ id: imp.id, status: 'error', error: e.message });
      ctx.warn(`${imp.id}: ${e.message}`);
    }
  }
  for (const layer of LAYERS) {
    for (const p of [...layer.collections.map((c) => c.path), ...(layer.scalars || [])]) {
      if (observed.has(p)) continue;
      const v = getPath(baseline, p);
      if (v !== undefined) setPath(live, p, structuredClone(v));
    }
  }
  for (const k of ['identity', 'brain', 'data', 'infra']) {
    // carry unobserved non-collection keys (e.g. identity.person, brain.budgets)
    for (const [kk, v] of Object.entries(baseline[k] || {})) if (live[k]?.[kk] === undefined && !Array.isArray(v)) setPath(live, `${k}.${kk}`, structuredClone(v));
  }
  // file-state reports existence for declared items; overrides supply the rest of each item.
  let merged = deepMerge(live, stripImportOnly(overrides));
  merged = deepMerge(merged, policies.safety ? { safety: policies.safety } : {});
  const head = {
    version: 1,
    meta: { instance: setup.meta?.instance || path.basename(inst?.dir || 'instance'), ...(setup.meta || {}) },
    ...(setup.vars ? { vars: setup.vars } : {}),
    ...(cfg && Object.keys(cfg).length ? { import: cfg } : {}),
  };
  return { live: normalize({ ...head, ...merged, ...head }), ran, warnings: ctx.warnings, observed: [...observed], ctx };
}

function stripImportOnly(o) { const { import: _i, meta: _m, version: _v, ...rest } = o || {}; return rest; }

// ---------- normalize ----------
// Fill required defaults so imported + hand-written data always meets the schema shape.
export function normalize(s) {
  s = { ...s };
  if (Array.isArray(s.loops)) {
    s.loops = s.loops.map((l) => ({
      runner: 'manual', enabled: false, ...l,
      ...(l.runner ? {} : { status: l.status || 'declared-only' }),
    }));
  }
  if (s.channels) {
    const ch = { ...s.channels };
    for (const k of Object.keys(ch)) {
      if (isPlainObject(ch[k])) ch[k] = [{ id: k, ...ch[k] }];
    }
    s.channels = ch;
  }
  if (s.instructions?.rules) {
    s.instructions = { ...s.instructions, rules: s.instructions.rules.map((r) => ({ scope: 'global', severity: 'standing', date: null, reason: '', status: 'active', ...r })) };
  }
  return s;
}

// ---------- validate ----------
export function validate(setup) {
  const ajv = new Ajv({ allErrors: true, strict: false });
  const ok = ajv.validate(SCHEMA, setup);
  const errors = ok ? [] : ajv.errors.map((e) => `${e.instancePath || '/'} ${e.message}${e.params?.allowedValues ? ' ' + JSON.stringify(e.params.allowedValues) : ''}${e.params?.additionalProperty ? ` (${e.params.additionalProperty})` : ''}`);
  const warnings = [];
  // duplicate ids within each collection
  for (const layer of LAYERS) {
    for (const c of layer.collections) {
      if (!c.key) continue;
      const list = c.path.split('.').reduce((o, k) => o?.[k], setup);
      if (!Array.isArray(list)) continue;
      const seen = new Set();
      for (const it of list) {
        if (seen.has(it?.id)) errors.push(`${c.path}: duplicate id "${it.id}"`);
        seen.add(it?.id);
      }
    }
  }
  // cross references
  const loopIds = new Set((setup.loops || []).map((l) => l.id));
  const ruleIds = new Set((setup.instructions?.rules || []).map((r) => r.id));
  const accIds = new Set((setup.identity?.accounts || []).map((a) => a.id));
  for (const lane of setup.business?.lanes || []) for (const l of lane.loops || []) if (!loopIds.has(l)) warnings.push(`business.lanes.${lane.id}: loop "${l}" not in loops`);
  for (const p of setup.safety?.policies || []) if (p.rule && !ruleIds.has(p.rule)) warnings.push(`safety.policies.${p.id}: rule "${p.rule}" not in instructions.rules`);
  for (const [k, list] of Object.entries(setup.channels || {})) for (const c of list || []) if (c.identity && c.identity !== 'none' && !accIds.has(c.identity)) warnings.push(`channels.${k}.${c.id}: identity "${c.identity}" not in identity.accounts`);
  return { ok: errors.length === 0, errors, warnings };
}
