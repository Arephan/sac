// Apply: render desired state into files under <framework>/out/<instance>/ and NOWHERE else.
// Nothing is installed, loaded, enabled or sent. A human copies what they want from out/.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import { OUT_ROOT, expandHome, stableStringify, toYaml, today } from './util.mjs';
import { countLayers } from '../layers/index.mjs';
import { formatPolicyResults } from '../policy/engine.mjs';

// Paths apply may never write into, whatever the configuration says.
const PROTECTED = ['~/.claude', '~/.claude.json', '~/Library/LaunchAgents', '~/Library/LaunchDaemons', '/Library', '/etc', '/System', '~/.config/systemd'];

export function assertSafeOutRoot() {
  fs.mkdirSync(OUT_ROOT, { recursive: true });
  const real = fs.realpathSync(OUT_ROOT);
  const home = os.homedir();
  for (const p of PROTECTED) {
    const abs = path.resolve(expandHome(p, home));
    if (real === abs || real.startsWith(abs + path.sep)) throw new Error(`refusing: out/ resolves into protected path ${p}`);
  }
  return real;
}

function writer(outDir) {
  const realRoot = assertSafeOutRoot();
  const written = [];
  return {
    written,
    write(rel, content) {
      const abs = path.resolve(outDir, rel);
      if (!abs.startsWith(realRoot + path.sep)) throw new Error(`refusing to write outside out/: ${abs}`);
      fs.mkdirSync(path.dirname(abs), { recursive: true });
      // Never follow a symlink planted inside out/.
      try { if (fs.lstatSync(abs).isSymbolicLink()) throw new Error(`refusing to overwrite symlink ${abs}`); } catch (e) { if (e.code !== 'ENOENT') throw e; }
      fs.writeFileSync(abs, content);
      written.push(path.relative(OUT_ROOT, abs));
    },
  };
}

const xml = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

function plist(loop, home) {
  const lines = ['<?xml version="1.0" encoding="UTF-8"?>', '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">', '<plist version="1.0">', '<dict>'];
  const kv = (k, v) => lines.push(`  <key>${xml(k)}</key>`, v);
  kv('Label', `  <string>${xml(loop.id)}</string>`);
  if (loop.program?.length) kv('ProgramArguments', ['  <array>', ...loop.program.map((a) => `    <string>${xml(expandHome(a, home))}</string>`), '  </array>'].join('\n'));
  if (loop.working_dir) kv('WorkingDirectory', `  <string>${xml(expandHome(loop.working_dir, home))}</string>`);
  if (loop.log) { kv('StandardOutPath', `  <string>${xml(expandHome(loop.log, home))}</string>`); kv('StandardErrorPath', `  <string>${xml(expandHome(loop.log, home))}</string>`); }
  const s = loop.schedule || {};
  if (s.every_seconds) kv('StartInterval', `  <integer>${Number(s.every_seconds)}</integer>`);
  if (s.calendar?.length) {
    const dicts = s.calendar.map((c) => ['    <dict>', ...c.split(',').map((p) => { const [k, v] = p.split('='); return `      <key>${k[0].toUpperCase() + k.slice(1)}</key>\n      <integer>${Number(v)}</integer>`; }), '    </dict>'].join('\n'));
    kv('StartCalendarInterval', ['  <array>', ...dicts, '  </array>'].join('\n'));
  }
  if (s.keep_alive) kv('KeepAlive', '  <true/>');
  kv('RunAtLoad', s.run_at_load ? '  <true/>' : '  <false/>');
  if (loop.env_names?.length) kv('EnvironmentVariables', ['  <dict>', ...loop.env_names.map((n) => `    <key>${xml(n)}</key>\n    <string>\${${xml(n)}}</string>`), '  </dict>'].join('\n'));
  lines.push('</dict>', '</plist>', '');
  return lines.join('\n');
}

function claudeSettings(d) {
  const t = d.tools || {};
  const hooks = {};
  for (const h of t.hooks || []) {
    hooks[h.event] ||= [];
    let g = hooks[h.event].find((x) => x.matcher === h.matcher);
    if (!g) { g = { matcher: h.matcher, hooks: [] }; hooks[h.event].push(g); }
    g.hooks.push({ type: 'command', command: h.command });
  }
  const cc = d.brain?.claude_code || {};
  return {
    ...(cc.model ? { model: cc.model } : {}),
    ...(cc.effort ? { effortLevel: cc.effort } : {}),
    ...(cc.env_names?.length ? { env: Object.fromEntries(cc.env_names.map((n) => [n, `\${${n}}`])) } : {}),
    permissions: {
      ...(t.permissions?.default_mode ? { defaultMode: t.permissions.default_mode } : {}),
      allow: t.permissions?.allow || [], deny: t.permissions?.deny || [], ask: t.permissions?.ask || [],
    },
    deniedMcpServers: (t.denied_mcp_servers || []).map((serverName) => ({ serverName })),
    hooks,
    enabledPlugins: Object.fromEntries((t.plugins || []).filter((p) => p.enabled !== undefined).map((p) => [p.id, p.enabled])),
  };
}

function mcpServers(d) {
  const out = {};
  for (const s of d.tools?.mcp_servers || []) {
    if (s.enabled === false || (d.tools?.denied_mcp_servers || []).includes(s.id)) continue;
    out[s.id.split('@')[0]] = {
      ...(s.transport === 'http' || s.transport === 'sse' ? { type: s.transport, url: s.url } : { command: s.command, args: s.args || [] }),
      ...(s.env_names?.length ? { env: Object.fromEntries(s.env_names.map((n) => [n, `\${${n}}`])) } : {}),
      ...(s.header_names?.length ? { headers: Object.fromEntries(s.header_names.map((n) => [n, `\${${n.toUpperCase().replace(/-/g, '_')}}`])) } : {}),
    };
  }
  return { mcpServers: out };
}

function rulesMd(d) {
  const rules = (d.instructions?.rules || []).filter((r) => r.status !== 'superseded');
  const lines = [`# Rules (${d.meta.instance})`, '', `Rendered by setup-as-code on ${today()} from setup.yaml. ${rules.length} active rules.`, ''];
  for (const sev of ['hard', 'standing']) {
    const list = rules.filter((r) => r.severity === sev);
    lines.push(`## ${sev === 'hard' ? 'Hard rules' : 'Standing rules'} (${list.length})`, '');
    const scopes = [...new Set(list.map((r) => r.scope))].sort();
    for (const sc of scopes) {
      lines.push(`### ${sc}`, '');
      for (const r of list.filter((x) => x.scope === sc)) lines.push(`- **${r.id}** (${r.date || 'undated'}): ${r.statement || r.reason}${r.reason && r.statement && r.reason !== r.statement ? ` _Why: ${r.reason}_` : ''}`);
      lines.push('');
    }
  }
  return lines.join('\n');
}

export function apply(desired, { policyReport, planReport, home } = {}) {
  const name = desired.meta.instance;
  const outDir = path.join(OUT_ROOT, name);
  const w = writer(outDir);
  w.write('claude/settings.json', JSON.stringify(claudeSettings(desired), null, 2) + '\n');
  w.write('claude/mcp-servers.json', JSON.stringify(mcpServers(desired), null, 2) + '\n');
  w.write('claude/RULES.md', rulesMd(desired));
  let nPlist = 0;
  for (const l of desired.loops || []) {
    if (l.runner !== 'launchd' || l.managed === false || !l.program?.length) continue;
    w.write(`launchd/${l.enabled ? '' : 'disabled/'}${l.id}.plist`, plist(l, home));
    nPlist++;
  }
  const counts = countLayers(desired);
  const lock = { instance: name, rendered_at: new Date().toISOString(), sha256: crypto.createHash('sha256').update(stableStringify(desired)).digest('hex'), counts };
  w.write('setup.lock.json', JSON.stringify(lock, null, 2) + '\n');
  w.write('setup.resolved.yaml', toYaml(desired, `# Fully resolved desired state for ${name}. Generated; do not edit.`));
  const report = [
    `# Apply report: ${name}`, '', `Rendered ${new Date().toISOString()}. Nothing was installed, loaded or sent.`, '',
    '## Counts per layer', '', '| layer | items |', '|---|---|', ...Object.entries(counts).map(([k, v]) => `| ${k} | ${v.total} |`), '',
    '## Files', '', ...w.written.map((f) => `- out/${f}`), `- out/${name}/report.md`, '',
    `launchd plists rendered: ${nPlist} (enabled at top level, disabled under launchd/disabled/). Install by hand if wanted.`, '',
    ...(planReport ? ['## Plan at apply time', '', '```', planReport, '```', ''] : []),
    ...(policyReport ? ['## Policy results', '', '```', formatPolicyResults(policyReport), '```', ''] : []),
  ].join('\n');
  w.write('report.md', report);
  return { outDir, written: w.written, nPlist, lock };
}
