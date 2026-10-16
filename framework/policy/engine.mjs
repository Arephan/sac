// Policy engine: runs safety.policies[] against a setup object.
// Custom checks: <instance>/checks/<name>.mjs exporting default { id, doc, run(setup, params, ctx) }.
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { CHECKS } from './checks.mjs';

export async function loadChecks(instanceDir) {
  const checks = { ...CHECKS };
  const dir = instanceDir && path.join(instanceDir, 'checks');
  if (dir && fs.existsSync(dir)) {
    for (const f of fs.readdirSync(dir).filter((x) => x.endsWith('.mjs')).sort()) {
      const mod = (await import(pathToFileURL(path.join(dir, f)).href)).default;
      if (!mod?.id || typeof mod.run !== 'function') throw new Error(`custom check ${f}: needs { id, run }`);
      checks[mod.id] = mod;
    }
  }
  return checks;
}

export function runPolicies(setup, checks, ctx = {}) {
  const results = [];
  for (const p of setup.safety?.policies || []) {
    const check = checks[p.check];
    let r;
    if (!check) r = { status: 'fail', message: `unknown check "${p.check}"`, evidence: [] };
    else {
      try { r = check.run(setup, p.params || {}, ctx); } catch (e) { r = { status: 'fail', message: `check crashed: ${e.message}`, evidence: [] }; }
    }
    results.push({ id: p.id, check: p.check, severity: p.severity, rule: p.rule, ...r });
  }
  const hardFails = results.filter((r) => r.status === 'fail' && r.severity === 'hard');
  const softFails = results.filter((r) => r.status === 'fail' && r.severity !== 'hard');
  return { results, hardFails, softFails, ok: hardFails.length === 0 };
}

export function formatPolicyResults({ results }, { verbose = false } = {}) {
  const icon = { pass: 'PASS', fail: 'FAIL', warn: 'WARN', skip: 'SKIP' };
  const lines = [];
  for (const r of results) {
    lines.push(`  [${icon[r.status]}] ${r.severity.padEnd(8)} ${r.id} (${r.check}): ${r.message}`);
    const ev = r.status === 'fail' || r.status === 'warn' || verbose ? r.evidence : [];
    for (const e of ev.slice(0, verbose ? 50 : 8)) lines.push(`         - ${e}`);
    if (!verbose && ev.length > 8) lines.push(`         … ${ev.length - 8} more (use --verbose)`);
  }
  return lines.join('\n');
}
