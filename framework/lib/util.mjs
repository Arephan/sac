// Shared helpers: paths, YAML/JSON IO, secret detection, merging.
// Nothing in this file may reference a specific person, account or machine.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import YAML from 'yaml';

export const FRAMEWORK_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
export const OUT_ROOT = path.join(FRAMEWORK_ROOT, 'out');

// ---------- paths ----------
export function expandHome(p, home = process.env.HOME || os.homedir()) {
  if (typeof p !== 'string') return p;
  if (p === '~') return home;
  if (p.startsWith('~/')) return path.join(home, p.slice(2));
  return p.replace(/\$\{HOME\}|\$HOME/g, home);
}

// Turn an absolute path under $HOME into a portable "~/..." form.
export function tilde(p, home = process.env.HOME || os.homedir()) {
  if (typeof p !== 'string' || !home) return p;
  return p.split(home + '/').join('~/').replace(new RegExp(escapeRe(home) + '(?=$|[\\s"\'])', 'g'), '~');
}

// Deep-apply tilde() to every string in a value.
export function tildeDeep(v, home) {
  if (typeof v === 'string') return tilde(v, home);
  if (Array.isArray(v)) return v.map((x) => tildeDeep(x, home));
  if (v && typeof v === 'object') return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, tildeDeep(x, home)]));
  return v;
}

// Claude Code stores per-project state under a slug of the absolute project path.
export function claudeProjectSlug(absPath) {
  return absPath.replace(/[^A-Za-z0-9]/g, '-');
}

export function escapeRe(s) { return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'); }

export function slug(s, max = 60) {
  return String(s).normalize('NFKD').replace(/[\u0300-\u036f]/g, '').toLowerCase()
    .replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, max).replace(/-+$/, '') || 'item';
}

// Expand ${env:NAME} and ${vars.NAME} references inside a loaded setup.
export function resolveVars(v, vars = {}, env = process.env) {
  const sub = (s) => s.replace(/\$\{env:([A-Za-z_][A-Za-z0-9_]*)\}/g, (_, n) => env[n] ?? '')
    .replace(/\$\{vars\.([A-Za-z_][A-Za-z0-9_]*)\}/g, (_, n) => vars[n] ?? '');
  if (typeof v === 'string') return sub(v);
  if (Array.isArray(v)) return v.map((x) => resolveVars(x, vars, env));
  if (v && typeof v === 'object') return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, resolveVars(x, vars, env)]));
  return v;
}

// ---------- IO ----------
export function readYaml(file) {
  const text = fs.readFileSync(file, 'utf8');
  const doc = YAML.parseDocument(text, { prettyErrors: true });
  if (doc.errors.length) throw new Error(`${file}: ${doc.errors[0].message}`);
  return doc.toJS() ?? {};
}

export function toYaml(obj, header = '') {
  const body = YAML.stringify(obj, { lineWidth: 0, aliasDuplicateObjects: false });
  return (header ? header.trimEnd() + '\n' : '') + body;
}

export function fileExists(p) { try { fs.accessSync(p); return true; } catch { return false; } }

// ---------- secrets ----------
// Patterns for strings that look like credential VALUES. Names and file paths are fine.
export const SECRET_PATTERNS = [
  { id: 'slack-token', re: /\bxox[abposr]-[A-Za-z0-9-]{10,}/ },
  { id: 'slack-app-token', re: /\bxapp-\d-[A-Za-z0-9-]{10,}/ },
  { id: 'openai-style-key', re: /\bsk-(?:proj-|ant-)?[A-Za-z0-9_-]{20,}/ },
  { id: 'github-token', re: /\b(?:ghp|gho|ghu|ghs|ghr|github_pat)_[A-Za-z0-9_]{20,}/ },
  { id: 'aws-access-key', re: /\bAKIA[0-9A-Z]{16}\b/ },
  { id: 'google-api-key', re: /\bAIza[0-9A-Za-z_-]{35}\b/ },
  { id: 'google-oauth-token', re: /\bya29\.[0-9A-Za-z_-]{20,}/ },
  { id: 'stripe-key', re: /\b(?:sk|rk|pk)_(?:live|test)_[0-9A-Za-z]{16,}/ },
  { id: 'private-key-block', re: /-----BEGIN (?:RSA |EC |OPENSSH |PGP )?PRIVATE KEY-----/ },
  { id: 'jwt', re: /\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}/ },
  { id: 'bearer-header', re: /\bBearer\s+[A-Za-z0-9._~+/-]{20,}=*/ },
  { id: 'url-credentials', re: /\b[a-z][a-z0-9+.-]*:\/\/[^\s:/@]+:[^\s:/@]{6,}@/i },
  { id: 'generic-assignment', re: /\b(?:api[_-]?key|secret|token|password|passwd)\s*[:=]\s*["']?[A-Za-z0-9_\-/+=]{24,}/i },
];

export function findSecrets(text) {
  const hits = [];
  for (const { id, re } of SECRET_PATTERNS) {
    const m = String(text).match(re);
    if (m) hits.push({ id, sample: m[0].slice(0, 6) + '…' });
  }
  return hits;
}

// Files an importer must never open: they hold credential values.
const SECRET_FILE_RE = /(^|\/)(\.env(\..*)?|.*token.*|.*_key|.*\.key|.*\.pem|.*\.p12|.*\.p8|id_(rsa|ed25519|ecdsa)(\.pub)?|credentials(\.json)?|auth\.json|.*secret.*|.*\.keychain(-db)?|cookies(\.sqlite)?|login data)$/i;
export function isSecretFile(p) { return SECRET_FILE_RE.test(String(p)); }

// Keys whose values are dropped (names kept) when importing configs.
export const SECRET_KEY_RE = /^(env|headers|environment|environmentvariables|token|apikey|api_key|secret|password|authorization|auth|credentials|cookie)$/i;

// ---------- merge ----------
export function isPlainObject(v) { return v && typeof v === 'object' && !Array.isArray(v); }

// Merge two lists of objects by `id`; fields from `b` win. Non-object lists are unioned.
export function mergeById(a = [], b = []) {
  if (!a.every(isPlainObject) || !b.every(isPlainObject)) return [...new Set([...(a || []), ...(b || [])])];
  const out = a.map((x) => ({ ...x }));
  const idx = new Map(out.map((x, i) => [x.id, i]));
  for (const item of b) {
    if (idx.has(item.id)) out[idx.get(item.id)] = deepMerge(out[idx.get(item.id)], item);
    else { idx.set(item.id, out.length); out.push({ ...item }); }
  }
  return out;
}

// Deep merge: objects recurse, id-lists merge by id, scalar lists union, scalars from b win.
// A keyed map in `b` merged onto an id-list in `a` is treated as { id: fields }.
export function deepMerge(a, b) {
  if (b === undefined) return a;
  if (a === undefined) return b;
  if (Array.isArray(a) && isPlainObject(b)) {
    return mergeById(a, Object.entries(b).map(([id, v]) => ({ id, ...(isPlainObject(v) ? v : { value: v }) })));
  }
  if (Array.isArray(a) && Array.isArray(b)) return mergeById(a, b);
  if (isPlainObject(a) && isPlainObject(b)) {
    const out = { ...a };
    for (const [k, v] of Object.entries(b)) out[k] = deepMerge(a[k], v);
    return out;
  }
  return b;
}

export function getPath(obj, p) {
  return p.split('.').reduce((o, k) => (o == null ? undefined : o[k]), obj);
}

export function stableStringify(v) {
  if (Array.isArray(v)) return '[' + v.map(stableStringify).join(',') + ']';
  if (isPlainObject(v)) return '{' + Object.keys(v).sort().map((k) => JSON.stringify(k) + ':' + stableStringify(v[k])).join(',') + '}';
  return JSON.stringify(v);
}

// Glob with * only (used for tool / server name patterns).
export function globToRe(g) { return new RegExp('^' + g.split('*').map(escapeRe).join('.*') + '$', 'i'); }

export function today() { return new Date().toISOString().slice(0, 10); }
