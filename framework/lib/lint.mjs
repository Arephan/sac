// PII + secret lint for everything that is meant to be shareable (framework, examples, docs).
// Two kinds of detectors:
//  1. Generic: secret-shaped strings, real-looking emails, phone numbers, user home paths,
//     chat-platform member/channel ids.
//  2. Instance-derived: every instance/<name>/ contributes terms that identify its owner:
//     lint-denylist.txt (one term per line) + values pulled from its setup.yaml
//     (account principals/workspaces, channel handles/senders/ids, the home dir user name).
//     An instance may also list exact phrases in lint-allow.txt that are allowed despite
//     containing a term (e.g. a folder name it chose on purpose); they are blanked before matching.
// The framework itself never contains those terms; it reads them from the instances at run time.
import fs from 'node:fs';
import path from 'node:path';
import { FRAMEWORK_ROOT, findSecrets, readYaml, escapeRe, fileExists } from './util.mjs';

export const DEFAULT_LINT_TARGETS = ['framework', 'examples', 'docs', 'README.md', 'setup.mjs', 'package.json', 'instance/README.md', '.gitignore'];
const SKIP_DIRS = new Set(['node_modules', 'out', '.git']);
const TEXT_EXT = /\.(mjs|js|cjs|json|ya?ml|md|txt|sh|toml|ts)$|^[^.]+$|^\.gitignore$/;

const EMAIL_RE = /\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b/g;
const SAFE_EMAIL_DOMAIN = /@(example\.(com|org|net)|users\.noreply\.github\.com|anthropic\.com)$/i;
const PHONE_RE = /(?<![\w.-])(?:\+?\d{1,2}[\s.-])?\(?\d{3}\)?[\s.-]\d{3}[\s.-]\d{4}(?![\w-])/g;
const HOME_PATH_RE = /\/(?:Users|home)\/([A-Za-z0-9._-]+)/g;
const SAFE_HOME_USERS = new Set(['you', 'me', 'user', 'username', 'example', 'alice', 'bob', 'shared', '<you>', 'runner']);
const CHAT_ID_RE = /\b[CDGUW][A-Z0-9]{8,11}\b/g;

function walk(p, out) {
  if (!fileExists(p)) return;
  const st = fs.statSync(p);
  if (st.isDirectory()) {
    if (SKIP_DIRS.has(path.basename(p))) return;
    for (const f of fs.readdirSync(p)) walk(path.join(p, f), out);
  } else if (TEXT_EXT.test(path.basename(p)) && st.size < 5_000_000) out.push(p);
}

export function instanceAllow(instanceRoot = path.join(FRAMEWORK_ROOT, 'instance')) {
  const allow = [];
  if (!fileExists(instanceRoot)) return allow;
  for (const name of fs.readdirSync(instanceRoot)) {
    const f = path.join(instanceRoot, name, 'lint-allow.txt');
    if (fileExists(f)) for (const line of fs.readFileSync(f, 'utf8').split('\n')) { const t = line.replace(/#.*/, '').trim(); if (t) allow.push(t); }
  }
  return allow;
}

export function instanceTerms(instanceRoot = path.join(FRAMEWORK_ROOT, 'instance')) {
  const terms = new Map(); // term -> origin
  if (!fileExists(instanceRoot)) return terms;
  const add = (t, origin) => {
    t = String(t || '').trim();
    if (t.length < 3 || /^(true|false|none|null|unknown|n\/a)$/i.test(t)) return;
    if (!terms.has(t.toLowerCase())) terms.set(t.toLowerCase(), origin);
  };
  for (const name of fs.readdirSync(instanceRoot)) {
    const dir = path.join(instanceRoot, name);
    if (!fs.statSync(dir).isDirectory()) continue;
    const deny = path.join(dir, 'lint-denylist.txt');
    if (fileExists(deny)) {
      for (const line of fs.readFileSync(deny, 'utf8').split('\n')) { const t = line.replace(/#.*/, '').trim(); if (t) add(t, `${name}/lint-denylist.txt`); }
    }
    for (const file of ['setup.yaml', 'overrides.yaml']) {
      const f = path.join(dir, file);
      if (!fileExists(f)) continue;
      let s; try { s = readYaml(f); } catch { continue; }
      const origin = `${name}/${file}`;
      for (const a of s.identity?.accounts || []) {
        for (const v of [a.principal, a.workspace]) for (const piece of String(v || '').split(/[,+]| and /)) {
          for (const tok of piece.trim().split(/\s+/)) if (/[@.]|^[A-Z0-9]{9,}$|^u\//.test(tok)) add(tok.replace(/[(),]/g, ''), origin);
          if (a.workspace && piece.trim() === a.workspace) add(piece.trim(), origin);
        }
      }
      for (const list of Object.values(s.channels || {})) {
        for (const c of [].concat(list || [])) {
          for (const v of [c.handle, c.sender]) if (v) add(v, origin);
          if (/^[CDGUW][A-Z0-9]{8,11}$/.test(String(c.id))) add(c.id, origin);
        }
      }
      for (const v of JSON.stringify(s).match(/\/(?:Users|home)\/([A-Za-z0-9._-]+)/g) || []) add(v.split('/')[2], origin);
    }
  }
  return terms;
}

export function lint({ targets = DEFAULT_LINT_TARGETS, root = FRAMEWORK_ROOT, terms = instanceTerms(), allow = instanceAllow() } = {}) {
  const allowRes = allow.map((a) => new RegExp(escapeRe(a), 'gi'));
  const files = [];
  for (const t of targets) walk(path.resolve(root, t), files);
  const findings = [];
  const termRes = [...terms.entries()].map(([t, origin]) => ({ t, origin, re: new RegExp(`(?<![A-Za-z0-9])${escapeRe(t)}(?![A-Za-z0-9])`, 'i') }));
  for (const f of files) {
    const rel = path.relative(root, f);
    const lines = fs.readFileSync(f, 'utf8').split('\n');
    lines.forEach((raw, i) => {
      const at = `${rel}:${i + 1}`;
      const line = allowRes.reduce((l, re) => l.replace(re, ' '), raw);
      for (const s of findSecrets(line)) findings.push({ at, kind: 'secret', detail: s.id });
      for (const m of line.matchAll(EMAIL_RE)) if (!SAFE_EMAIL_DOMAIN.test(m[0]) && !/^[\w.-]+@[\w.-]+\.(mjs|js|json|md)$/.test(m[0])) findings.push({ at, kind: 'email', detail: m[0] });
      for (const m of line.matchAll(PHONE_RE)) findings.push({ at, kind: 'phone', detail: m[0] });
      for (const m of line.matchAll(HOME_PATH_RE)) if (!SAFE_HOME_USERS.has(m[1].toLowerCase())) findings.push({ at, kind: 'home-path', detail: m[0] });
      for (const m of line.matchAll(CHAT_ID_RE)) if ((m[0].match(/\d/g) || []).length >= 2) findings.push({ at, kind: 'chat-id', detail: m[0] });
      for (const { t, origin, re } of termRes) if (re.test(line)) findings.push({ at, kind: 'instance-term', detail: `"${t}" (from ${origin})` });
    });
  }
  return { files: files.length, terms: terms.size, allowed: allow, findings };
}

export function formatLint(r) {
  const lines = [`  scanned ${r.files} file(s) against ${r.terms} instance-derived term(s) + generic secret/PII detectors`];
  if (r.allowed?.length) lines.push(`  allowlisted phrase(s) from instance lint-allow.txt: ${r.allowed.join(', ')}`);
  for (const f of r.findings.slice(0, 200)) lines.push(`  ${f.kind.padEnd(14)} ${f.at}  ${f.detail}`);
  if (r.findings.length > 200) lines.push(`  … ${r.findings.length - 200} more`);
  lines.push(r.findings.length ? `  LINT FAILED: ${r.findings.length} finding(s)` : '  LINT PASSED: 0 findings');
  return lines.join('\n');
}
