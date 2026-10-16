// Built-in policy checks. Each check is a pure function (setup, params) -> result.
// result = { status: 'pass'|'fail'|'warn'|'skip', message, evidence: string[] }
// Add a check: export it in CHECKS with a doc entry; see docs/policies.md.
import { globToRe, findSecrets, tilde, expandHome } from '../lib/util.mjs';

const pass = (message, evidence = []) => ({ status: 'pass', message, evidence });
const fail = (message, evidence = []) => ({ status: 'fail', message, evidence });
const warn = (message, evidence = []) => ({ status: 'warn', message, evidence });
const skip = (message) => ({ status: 'skip', message, evidence: [] });

const INACTIVE_RE = /halt|dead|killed|blocked|frozen|forbidden|paused|stopped|removed|superseded/i;
export const isActiveChannel = (c) => c.active !== false && !INACTIVE_RE.test(String(c.status || '')) && !INACTIVE_RE.test(String(c.role || '').split(/[;,]/)[0]);

function matchesAnyGlob(value, globs) { return globs.some((g) => globToRe(g).test(value)); }

// Is `pattern` covered by some entry of `list`? (exact, or list entry is a broader glob)
function covered(pattern, list) {
  return list.some((e) => e === pattern || globToRe(e).test(pattern.replace(/\*/g, 'x')));
}

export const CHECKS = {
  deny_mcp: {
    doc: 'Named MCP servers must be blocked (tools.denied_mcp_servers) and their tool patterns denied (tools.permissions.deny), never allowed, and never defined as an enabled server unless blocked.',
    params: { servers: 'list of server names', tool_patterns: 'list of tool globs, e.g. mcp__name__*' },
    run(setup, { servers = [], tool_patterns = [] } = {}) {
      const denied = setup.tools?.denied_mcp_servers || [];
      const deny = setup.tools?.permissions?.deny || [];
      const allow = setup.tools?.permissions?.allow || [];
      const defined = setup.tools?.mcp_servers || [];
      const bad = [], notes = [];
      for (const s of servers) {
        if (!denied.includes(s)) bad.push(`server "${s}" not in denied_mcp_servers`);
        for (const d of defined.filter((x) => x.id === s || x.id.startsWith(s + '@'))) {
          if (!denied.includes(s) && d.enabled !== false) bad.push(`server "${s}" defined and enabled in ${d.source || 'setup'}`);
          else notes.push(`server "${s}" still defined in ${d.source || 'setup'} but blocked`);
        }
      }
      const dead = [];
      for (const p of tool_patterns) {
        const isDenied = covered(p, deny);
        if (!isDenied) bad.push(`tool pattern ${p} missing from permissions.deny`);
        const allowedHit = allow.filter((a) => globToRe(p).test(a) || (a.startsWith('mcp__') && globToRe(a).test(p.replace(/\*/g, 'x'))));
        // Claude Code evaluates deny before allow, so an allow entry under a denied pattern is dead.
        if (allowedHit.length && !isDenied) bad.push(`tool pattern ${p} allowed by: ${allowedHit.join(', ')}`);
        else if (allowedHit.length) dead.push(...allowedHit.map((a) => `dead allow entry (deny wins): ${a}`));
      }
      if (bad.length) return fail(`${bad.length} way(s) the denied MCP surface is reachable`, [...bad, ...notes, ...dead]);
      const msg = `${servers.length} server(s) blocked, ${tool_patterns.length} tool pattern(s) denied`;
      return dead.length ? warn(`${msg}; ${dead.length} dead allow entr${dead.length === 1 ? 'y' : 'ies'} to clean up`, [...dead, ...notes]) : pass(msg, notes);
    },
  },

  no_payment_tools: {
    doc: 'No enabled tool, server, plugin, skill, permission or can_post account whose name looks payment/spend related, unless listed in allow.',
    params: { patterns: 'extra words (defaults cover stripe, paypal, checkout, billing, ads...)', allow: 'ids that are acceptable' },
    run(setup, { patterns = [], allow = [] } = {}) {
      const words = ['stripe', 'paypal', 'checkout', 'purchase', 'payment', 'payments', 'billing', 'braintree', 'adyen', 'square', 'coinbase', 'venmo', 'revolut', 'invoice', 'google-ads', 'meta-ads', 'ads-manager', 'adspend', 'buy', 'shop-pay', 'top-up', ...patterns].map((w) => w.toLowerCase());
      const hit = (name) => {
        const n = String(name).toLowerCase();
        const toks = new Set(n.split(/[^a-z0-9]+/));
        return words.find((w) => (w.includes('-') ? n.includes(w) : toks.has(w)));
      };
      const t = setup.tools || {};
      const deniedServers = t.denied_mcp_servers || [];
      const deny = t.permissions?.deny || [];
      const found = [], dormant = [];
      const consider = (kind, id, name, live) => {
        if (allow.includes(id)) return;
        const w = hit(name);
        if (!w) return;
        (live === true ? found : dormant).push(`${kind} ${id.length > 120 ? id.slice(0, 117) + '...' : id} (matches "${w}"${live === true ? '' : `; ${live}`})`);
      };
      const liveServers = new Set((t.mcp_servers || []).filter((s) => s.enabled !== false && !deniedServers.includes(s.id)).map((s) => s.id.split('@')[0]));
      for (const s of t.mcp_servers || []) consider('mcp', s.id, `${s.id} ${s.command || ''} ${s.url || ''}`, s.enabled !== false && !deniedServers.includes(s.id) ? true : 'disabled');
      for (const p of t.plugins || []) consider('plugin', p.id, p.id, p.enabled !== false ? true : 'disabled');
      for (const s of t.skills || []) consider('skill', s.id, s.id, true);
      for (const a of t.permissions?.allow || []) {
        // mcp__<server>__<tool>: only live if that server is currently defined and enabled.
        const srv = a.match(/^mcp__(.+?)__/)?.[1];
        const state = covered(a, deny) ? 'denied' : srv && !liveServers.has(srv) && !liveServers.has(srv.replace(/_/g, '-')) ? 'server not defined' : /^Bash\(/.test(a) ? 'exact shell command allow' : true;
        consider('allow', a, a, state);
      }
      for (const acc of setup.identity?.accounts || []) consider('account', acc.id, `${acc.id} ${(acc.scopes || []).join(' ')}`, acc.access === 'can_post' ? true : acc.access);
      if (found.length) return fail(`${found.length} live payment-capable surface(s)`, [...found, ...dormant.map((d) => `dormant: ${d}`)]);
      return dormant.length ? warn(`no live payment-capable tools; ${dormant.length} dormant entr${dormant.length === 1 ? 'y' : 'ies'} to review`, dormant)
        : pass('no payment-capable tools');
    },
  },

  forbidden_paths: {
    doc: 'No hook, loop or MCP server command references a forbidden path, and the apply output dir is not inside one.',
    params: { paths: 'list of paths (~ allowed)' },
    run(setup, { paths = [] } = {}, ctx = {}) {
      const norm = (p) => tilde(expandHome(p, ctx.home), ctx.home).replace(/\/+$/, '');
      const forb = paths.map(norm);
      const bad = [];
      const scan = (where, s) => { const v = norm(String(s || '')); for (const f of forb) if (v === f || v.includes(f + '/') || v.includes(f + ' ') || v.endsWith(f)) bad.push(`${where} -> ${f}`); };
      for (const h of setup.tools?.hooks || []) scan(`hook ${h.id}`, h.command);
      for (const l of setup.loops || []) if (l.enabled) { for (const a of l.program || []) scan(`loop ${l.id}`, a); scan(`loop ${l.id} cwd`, l.working_dir); }
      for (const m of setup.tools?.mcp_servers || []) if (m.enabled !== false) { scan(`mcp ${m.id}`, m.command); for (const a of m.args || []) scan(`mcp ${m.id}`, a); }
      if (ctx.outRoot) scan('apply output dir', ctx.outRoot);
      return bad.length ? fail(`${bad.length} reference(s) to forbidden paths`, bad) : pass(`${forb.length} forbidden path(s) unreferenced`);
    },
  },

  required_gate: {
    doc: 'A named gate exists in brain.gates and covers every active channel of the given kinds (channel.gate, or gate.applies_to naming the kind, platform or channel id).',
    params: { gate: 'gate id', channel_kinds: 'e.g. [email, social]' },
    run(setup, { gate, channel_kinds = [] } = {}) {
      const g = (setup.brain?.gates || []).find((x) => x.id === gate);
      if (!g) return fail(`gate "${gate}" is not defined in brain.gates`);
      const applies = new Set((g.applies_to || []).map((x) => String(x).toLowerCase()));
      const missing = [];
      let n = 0;
      for (const kind of channel_kinds) {
        for (const c of setup.channels?.[kind] || []) {
          if (!isActiveChannel(c)) continue;
          n++;
          const plats = [].concat(c.platform || []).map((p) => String(p).toLowerCase());
          const ok = c.gate === gate || applies.has(kind) || applies.has(c.id) || plats.some((p) => applies.has(p));
          if (!ok) missing.push(`${kind}/${c.id}`);
        }
      }
      return missing.length ? fail(`${missing.length} active channel(s) bypass gate ${gate}`, missing) : pass(`gate ${gate} covers ${n} active channel(s)`);
    },
  },

  channel_identity: {
    doc: 'Every active channel of a kind posts only through allowed identities; no channel uses an account whose access is forbidden.',
    params: { kind: 'channel kind, e.g. slack', allowed: 'account ids allowed to post' },
    run(setup, { kind, allowed = [] } = {}) {
      const accounts = new Map((setup.identity?.accounts || []).map((a) => [a.id, a]));
      const bad = [], warns = [];
      for (const c of setup.channels?.[kind] || []) {
        if (!c.identity || c.identity === 'none') { if (isActiveChannel(c)) warns.push(`${c.id}: no identity declared`); continue; }
        const acc = accounts.get(c.identity);
        if (acc?.access === 'forbidden') bad.push(`${c.id} uses forbidden account ${c.identity}`);
        else if (isActiveChannel(c) && allowed.length && !allowed.includes(c.identity)) bad.push(`${c.id} posts as ${c.identity} (allowed: ${allowed.join(', ')})`);
        if (!acc) warns.push(`${c.id}: identity ${c.identity} not declared in identity.accounts`);
      }
      if (bad.length) return fail(`${bad.length} ${kind} channel(s) use a disallowed identity`, [...bad, ...warns]);
      return warns.length ? warn(`${kind} identities ok, ${warns.length} note(s)`, warns) : pass(`all ${kind} channels post through allowed identities`);
    },
  },

  secrets_by_reference: {
    doc: 'No secret-looking value anywhere in the setup; credentials appear only as references (file path, env var name, keychain item).',
    params: {},
    run(setup) {
      const hits = [];
      const walk = (v, p) => {
        if (typeof v === 'string') { for (const h of findSecrets(v)) hits.push(`${p}: ${h.id}`); }
        else if (Array.isArray(v)) v.forEach((x, i) => walk(x, `${p}[${i}]`));
        else if (v && typeof v === 'object') for (const [k, x] of Object.entries(v)) walk(x, p ? `${p}.${k}` : k);
      };
      walk(setup, '');
      return hits.length ? fail(`${hits.length} secret-looking value(s) in setup`, hits) : pass('no secret values; references only');
    },
  },

  loop_success_metric: {
    doc: 'Every enabled (managed) loop declares a success_metric.',
    params: { only_managed: 'default true' },
    run(setup, { only_managed = true } = {}) {
      const missing = (setup.loops || []).filter((l) => l.enabled && (!only_managed || l.managed !== false) && !l.success_metric).map((l) => l.id);
      return missing.length ? fail(`${missing.length} enabled loop(s) without a success metric`, missing) : pass('every enabled loop has a success metric');
    },
  },

  required_rules: {
    doc: 'Listed rule ids exist in instructions.rules, are active, and are hard.',
    params: { ids: 'rule ids' },
    run(setup, { ids = [] } = {}) {
      const rules = new Map((setup.instructions?.rules || []).map((r) => [r.id, r]));
      const bad = [];
      for (const id of ids) {
        const r = rules.get(id);
        if (!r) bad.push(`${id}: missing`);
        else if (r.status === 'superseded') bad.push(`${id}: superseded`);
        else if (r.severity !== 'hard') bad.push(`${id}: severity ${r.severity}, expected hard`);
      }
      return bad.length ? fail(`${bad.length} required rule(s) not in force`, bad) : pass(`${ids.length} required hard rule(s) in force`);
    },
  },

  forbidden_accounts_unused: {
    doc: 'Accounts with access: forbidden are not referenced by any channel, loop or gate.',
    params: {},
    run(setup) {
      const forbidden = (setup.identity?.accounts || []).filter((a) => a.access === 'forbidden').map((a) => a.id);
      if (!forbidden.length) return skip('no forbidden accounts declared');
      const refs = [];
      for (const [kind, list] of Object.entries(setup.channels || {})) for (const c of list || []) if (forbidden.includes(c.identity)) refs.push(`channel ${kind}/${c.id}`);
      for (const l of setup.loops || []) if (forbidden.includes(l.identity)) refs.push(`loop ${l.id}`);
      return refs.length ? fail(`${refs.length} reference(s) to forbidden accounts`, refs) : pass(`${forbidden.length} forbidden account(s) unreferenced`);
    },
  },
};
