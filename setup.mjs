#!/usr/bin/env node
// setup-as-code CLI: init | import | validate | plan | verify | apply | lint | redact | layers | importers
//
// Safety contract (enforced in code, not just documented):
//   - import / plan / verify only READ the machine (allowlisted read-only commands, no secret files).
//   - apply writes only under <this repo>/out/<instance>/.
//   - nothing here loads/unloads jobs, edits live assistant config, spends money or sends messages.
import path from 'node:path';
import fs from 'node:fs';
import { FRAMEWORK_ROOT, toYaml, readYaml, fileExists } from './framework/lib/util.mjs';
import { resolveInstance, loadInstance, desiredState, importLive, validate } from './framework/lib/core.mjs';
import { plan, formatPlan } from './framework/lib/plan.mjs';
import { apply } from './framework/lib/apply.mjs';
import { init } from './framework/lib/init.mjs';
import { lint, formatLint, instanceTerms, DEFAULT_LINT_TARGETS } from './framework/lib/lint.mjs';
import { redact } from './framework/lib/redact.mjs';
import { loadChecks, runPolicies, formatPolicyResults } from './framework/policy/engine.mjs';
import { LAYERS, countLayers } from './framework/layers/index.mjs';
import { loadImporters } from './framework/importers/index.mjs';
import { OUT_ROOT } from './framework/lib/util.mjs';

const HELP = `usage: node setup.mjs <command> [options]

commands
  init <dir> [--name n] [--force]   scaffold a new person's setup (setup.yaml, policies.yaml, overrides.yaml)
  import   [-i dir] [--dry-run]     read the machine (read-only) + overrides.yaml, rewrite <dir>/setup.yaml
  validate [-i dir]                 schema + duplicate-id + cross-reference checks
  plan     [-i dir]                 diff setup.yaml vs a fresh read of the machine; writes nothing
  verify   [-i dir] [--fail-on-drift]  plan + safety policies on desired and live; exit 1 on a hard failure
  apply    [-i dir]                 render config into out/<instance>/ (only there)
  lint     [paths...]               secret + PII lint over framework/, examples/, docs/ (instance-derived terms)
  redact   [-i dir] --out <dir>     write a shape-only, lint-clean copy of an instance
  layers | importers                list layer definitions / available importers

options
  -i, --instance <dir>   instance folder (or setup.yaml path). Default: $SETUP_INSTANCE, ./setup.yaml, or the only folder in instance/
  --home <dir>           read the machine as if this were $HOME (sandbox a "new person")
  --json                 machine-readable output
  -v, --verbose          show every change / every piece of evidence`;

function parseArgs(argv) {
  const a = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const x = argv[i];
    if (x === '-i' || x === '--instance') a.instance = argv[++i];
    else if (x === '--home') a.home = argv[++i];
    else if (x === '--name') a.name = argv[++i];
    else if (x === '--out') a.out = argv[++i];
    else if (x === '--force') a.force = true;
    else if (x === '--json') a.json = true;
    else if (x === '--dry-run') a.dryRun = true;
    else if (x === '--fail-on-drift') a.failOnDrift = true;
    else if (x === '-v' || x === '--verbose') a.verbose = true;
    else if (x === '-h' || x === '--help') a.help = true;
    else a._.push(x);
  }
  return a;
}

const out = (s = '') => process.stdout.write(s + '\n');
const rel = (p) => path.relative(process.cwd(), p) || '.';

function printCounts(setup, title) {
  const c = countLayers(setup);
  out(title);
  for (const [k, v] of Object.entries(c)) {
    const parts = Object.entries(v).filter(([kk]) => kk !== 'total').map(([kk, n]) => `${kk} ${n}`).join(', ');
    out(`  ${k.padEnd(13)} ${String(v.total).padStart(4)}   ${parts}`);
  }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const cmd = args._[0];
  if (!cmd || args.help) { out(HELP); return 0; }

  if (cmd === 'init') {
    const dir = args._[1];
    if (!dir) throw new Error('init needs a target directory');
    const r = init(dir, { name: args.name, force: args.force });
    out(`initialized instance "${r.instance}" in ${rel(r.dir)}: ${r.files.join(', ')}`);
    out(`next: node ${rel(path.join(FRAMEWORK_ROOT, 'setup.mjs'))} validate -i ${rel(r.dir)}`);
    return 0;
  }

  if (cmd === 'layers') { for (const l of LAYERS) out(`${l.id.padEnd(13)} ${l.summary}`); return 0; }
  if (cmd === 'importers') {
    let dir; try { dir = resolveInstance(args.instance).dir; } catch { dir = null; }
    for (const i of await loadImporters(dir)) out(`${i.id.padEnd(20)} [${i.layers.join(',')}] ${i.description || ''}`);
    return 0;
  }

  if (cmd === 'lint') {
    const r = lint({ targets: args._.length > 1 ? args._.slice(1) : DEFAULT_LINT_TARGETS });
    if (args.json) out(JSON.stringify(r, null, 2)); else out(formatLint(r));
    return r.findings.length ? 1 : 0;
  }

  const inst = resolveInstance(args.instance);

  if (cmd === 'import') {
    const existing = fileExists(inst.file) ? readYaml(inst.file) : {};
    const overrides = fileExists(path.join(inst.dir, 'overrides.yaml')) ? readYaml(path.join(inst.dir, 'overrides.yaml')) : {};
    out(`import: reading machine (read-only) for instance ${rel(inst.dir)}`);
    const { live, ran, warnings } = await importLive({ inst, setup: existing, overrides, home: args.home, log: args.verbose ? out : () => {} });
    for (const r of ran) out(`  ${r.status.padEnd(12)} ${r.id}${r.error ? ': ' + r.error : ''}`);
    if (warnings.length) out(`  ${warnings.length} warning(s)${args.verbose ? '' : ' (use -v)'}`);
    live.meta = { ...live.meta, generated_at: new Date().toISOString(), generated_by: 'setup-as-code import' };
    const v = validate(live);
    printCounts(live, 'counts per layer:');
    if (!v.ok) { out('imported state fails the schema:'); for (const e of v.errors.slice(0, 30)) out('  ' + e); return 1; }
    if (args.dryRun) { out('dry run: nothing written'); return 0; }
    const header = `# Generated by \`setup.mjs import\` on ${new Date().toISOString().slice(0, 10)}. Re-running import rewrites this file.\n# Hand-curated facts belong in overrides.yaml; policies in policies.yaml.`;
    fs.writeFileSync(inst.file, toYaml(live, header));
    out(`wrote ${rel(inst.file)}`);
    return 0;
  }

  const loaded = loadInstance(inst);
  const desired = desiredState(loaded);

  if (cmd === 'validate') {
    const v = validate(desired);
    printCounts(desired, `validate ${rel(inst.file)}`);
    for (const w of v.warnings) out(`  warn: ${w}`);
    for (const e of v.errors) out(`  error: ${e}`);
    out(v.ok ? '  VALID' : `  INVALID (${v.errors.length} error(s))`);
    return v.ok ? 0 : 1;
  }

  if (cmd === 'redact') {
    if (!args.out) throw new Error('redact needs --out <dir>');
    const r = redact(desired);
    const v = validate(r);
    if (!v.ok) { for (const e of v.errors) out('  error: ' + e); return 1; }
    fs.mkdirSync(args.out, { recursive: true });
    const f = path.join(args.out, 'setup.yaml');
    fs.writeFileSync(f, toYaml(r, '# Shape-only redaction generated by `setup.mjs redact`. Every name, path, handle and text is replaced;\n# only counts, kinds, severities, scopes, dates, schedules and on/off flags are real.'));
    printCounts(r, `wrote ${rel(f)}`);
    return 0;
  }

  // plan / verify / apply all need live state.
  const { live, warnings } = await importLive({ inst, setup: loaded.setup, overrides: loaded.overrides, policies: loaded.policies, home: args.home, baseline: desired });
  const p = plan(desired, live);

  if (cmd === 'plan') {
    if (args.json) out(JSON.stringify(p, null, 2));
    else { out(`plan: ${rel(inst.file)} (desired) vs machine (live${args.home ? `, home=${args.home}` : ''})`); out(formatPlan(p, { verbose: args.verbose })); }
    return 0;
  }

  const checks = await loadChecks(inst.dir);
  const ctx = { home: args.home || process.env.HOME, outRoot: OUT_ROOT };

  if (cmd === 'verify') {
    const v = validate(desired);
    const pd = runPolicies(desired, checks, ctx);
    const pl = runPolicies(live, checks, ctx);
    if (args.json) { out(JSON.stringify({ schema: v, plan: p.totals, desired: pd, live: pl }, null, 2)); }
    else {
      out(`verify ${rel(inst.file)}`);
      out(`schema: ${v.ok ? 'valid' : `INVALID (${v.errors.length})`}`);
      for (const e of v.errors.slice(0, 20)) out(`  error: ${e}`);
      out(`drift: ${p.totals.add} to add, ${p.totals.change} changed, ${p.totals.untracked} untracked${p.totals.add + p.totals.change + p.totals.untracked ? ' (run `plan` for detail, `import` to adopt live state)' : ''}`);
      out(`policies on desired (setup.yaml):`); out(formatPolicyResults(pd, { verbose: args.verbose }));
      out(`policies on live (machine now):`); out(formatPolicyResults(pl, { verbose: args.verbose }));
      const hard = new Set([...pd.hardFails, ...pl.hardFails].map((r) => r.id));
      out(hard.size ? `VERIFY FAILED: hard policy failure(s): ${[...hard].join(', ')}` : `VERIFY OK: ${pd.results.length} policies, 0 hard failures${warnings.length ? `, ${warnings.length} import warning(s)` : ''}`);
    }
    const drift = p.totals.add + p.totals.change + p.totals.untracked;
    if (!v.ok || !pd.ok || !pl.ok) return 1;
    if (args.failOnDrift && drift) return 2;
    return 0;
  }

  if (cmd === 'apply') {
    const v = validate(desired);
    if (!v.ok) { out('refusing to apply an invalid setup:'); for (const e of v.errors) out('  ' + e); return 1; }
    const pd = runPolicies(desired, checks, ctx);
    if (!pd.ok) { out('refusing to apply: hard policy failures on desired state'); out(formatPolicyResults(pd)); return 1; }
    const r = apply(desired, { policyReport: pd, planReport: formatPlan(p), home: args.home || process.env.HOME });
    out(`apply: rendered ${r.written.length} file(s) into ${rel(r.outDir)} (nothing installed or loaded)`);
    for (const f of r.written) out(`  out/${f}`);
    return 0;
  }

  throw new Error(`unknown command "${cmd}"\n\n${HELP}`);
}

main().then((code) => process.exit(code ?? 0), (e) => { process.stderr.write(`error: ${e.message}\n`); process.exit(1); });
