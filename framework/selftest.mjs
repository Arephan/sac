// Self-test: every built-in check must FAIL on a bad fixture and PASS on a good one, and the
// safety rails (secret-file guard, exec allowlist, out/ write guard, init guard) must hold.
// Run: node framework/selftest.mjs   (no network, no writes outside a temp dir and out/)
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
import { CHECKS } from './policy/checks.mjs';
import { makeContext } from './lib/context.mjs';
import { validate, normalize } from './lib/core.mjs';
import { plan } from './lib/plan.mjs';
import { init } from './lib/init.mjs';
import { lint } from './lib/lint.mjs';
import { deepMerge } from './lib/util.mjs';

// Canary values are assembled at run time so this file itself passes the lint.
const FAKE_TOKEN = ['xox', 'b-1234567890-abcdefghijklmnop'].join('');
const FAKE_EMAIL = ['someone', 'gmail.com'].join('@');

let n = 0;
const t = (name, fn) => { try { fn(); n++; process.stdout.write(`  ok   ${name}\n`); } catch (e) { process.stdout.write(`  FAIL ${name}: ${e.message}\n`); process.exitCode = 1; } };
const run = (check, setup, params, ctx) => CHECKS[check].run(setup, params, ctx).status;

const base = () => ({
  version: 1, meta: { instance: 'fixture' },
  identity: { accounts: [{ id: 'bot', kind: 'chat_bot', access: 'can_post' }, { id: 'work', kind: 'chat_user', access: 'forbidden' }] },
  brain: { gates: [{ id: 'gate', applies_to: ['email'] }] },
  tools: { mcp_servers: [], plugins: [], skills: [], permissions: { allow: [], deny: ['mcp__work__*'], ask: [] }, denied_mcp_servers: ['work'], hooks: [] },
  loops: [], channels: { slack: [{ id: 'home', identity: 'bot' }], email: [{ id: 'mail', identity: 'bot' }], social: [] },
  instructions: { rules: [{ id: 'money', scope: 'money', severity: 'hard', date: '2026-01-01', reason: 'x' }] },
});

t('deny_mcp passes when blocked + denied', () => assert.equal(run('deny_mcp', base(), { servers: ['work'], tool_patterns: ['mcp__work__*'] }), 'pass'));
t('deny_mcp fails when server not blocked', () => { const s = base(); s.tools.denied_mcp_servers = []; assert.equal(run('deny_mcp', s, { servers: ['work'] }), 'fail'); });
t('deny_mcp fails when tool allowed and not denied', () => { const s = base(); s.tools.permissions.deny = []; s.tools.permissions.allow = ['mcp__work__send']; assert.equal(run('deny_mcp', s, { servers: ['work'], tool_patterns: ['mcp__work__*'] }), 'fail'); });
t('deny_mcp warns on dead allow under deny', () => { const s = base(); s.tools.permissions.allow = ['mcp__work__send']; assert.equal(run('deny_mcp', s, { servers: ['work'], tool_patterns: ['mcp__work__*'] }), 'warn'); });

t('no_payment_tools fails on enabled stripe server', () => { const s = base(); s.tools.mcp_servers = [{ id: 'stripe', enabled: true }]; assert.equal(run('no_payment_tools', s, {}), 'fail'); });
t('no_payment_tools fails on live allow for a defined pay server', () => { const s = base(); s.tools.mcp_servers = [{ id: 'billing', enabled: true }]; s.tools.permissions.allow = ['mcp__billing__charge']; assert.equal(run('no_payment_tools', s, {}), 'fail'); });
t('no_payment_tools warns on disabled server', () => { const s = base(); s.tools.mcp_servers = [{ id: 'stripe', enabled: false }]; assert.equal(run('no_payment_tools', s, {}), 'warn'); });
t('no_payment_tools respects allow', () => { const s = base(); s.tools.mcp_servers = [{ id: 'stripe', enabled: true }]; assert.equal(run('no_payment_tools', s, { allow: ['stripe'] }), 'pass'); });
t('no_payment_tools passes clean', () => assert.equal(run('no_payment_tools', base(), {}), 'pass'));

t('forbidden_paths fails on hook pointing at path', () => { const s = base(); s.tools.hooks = [{ id: 'h', event: 'PreToolUse', command: '~/.ssh/run.sh' }]; assert.equal(run('forbidden_paths', s, { paths: ['~/.ssh'] }, { home: '/home/user' }), 'fail'); });
t('forbidden_paths fails on enabled loop cwd', () => { const s = base(); s.loops = [{ id: 'l', runner: 'cron', enabled: true, working_dir: '/home/user/secret-dir' }]; assert.equal(run('forbidden_paths', s, { paths: ['~/secret-dir'] }, { home: '/home/user' }), 'fail'); });
t('forbidden_paths passes clean', () => assert.equal(run('forbidden_paths', base(), { paths: ['~/.ssh'] }, { home: '/home/user' }), 'pass'));

t('required_gate passes when gate covers kind', () => assert.equal(run('required_gate', base(), { gate: 'gate', channel_kinds: ['email'] }), 'pass'));
t('required_gate fails on ungated social', () => { const s = base(); s.channels.social = [{ id: 'x', platform: 'x' }]; assert.equal(run('required_gate', s, { gate: 'gate', channel_kinds: ['email', 'social'] }), 'fail'); });
t('required_gate ignores halted channels', () => { const s = base(); s.channels.social = [{ id: 'x', status: 'HALTED' }]; assert.equal(run('required_gate', s, { gate: 'gate', channel_kinds: ['social'] }), 'pass'); });
t('required_gate fails when gate missing', () => assert.equal(run('required_gate', base(), { gate: 'nope', channel_kinds: ['email'] }), 'fail'));

t('channel_identity fails on wrong identity', () => { const s = base(); s.channels.slack.push({ id: 'b', identity: 'other' }); assert.equal(run('channel_identity', s, { kind: 'slack', allowed: ['bot'] }), 'fail'); });
t('channel_identity fails on forbidden account', () => { const s = base(); s.channels.slack.push({ id: 'w', identity: 'work', status: 'forbidden' }); assert.equal(run('channel_identity', s, { kind: 'slack', allowed: ['bot'] }), 'fail'); });
t('channel_identity passes clean', () => assert.equal(run('channel_identity', base(), { kind: 'slack', allowed: ['bot'] }), 'pass'));

t('secrets_by_reference fails on token value', () => { const s = base(); s.identity.accounts[0].note = FAKE_TOKEN; assert.equal(run('secrets_by_reference', s, {}), 'fail'); });
t('secrets_by_reference passes on path refs', () => { const s = base(); s.identity.accounts[0].credential_ref = '~/.bot_token'; assert.equal(run('secrets_by_reference', s, {}), 'pass'); });

t('loop_success_metric fails on enabled loop without metric', () => { const s = base(); s.loops = [{ id: 'l', runner: 'cron', enabled: true }]; assert.equal(run('loop_success_metric', s, {}), 'fail'); });
t('loop_success_metric ignores disabled', () => { const s = base(); s.loops = [{ id: 'l', runner: 'cron', enabled: false }]; assert.equal(run('loop_success_metric', s, {}), 'pass'); });

t('required_rules fails on missing/soft rule', () => { const s = base(); assert.equal(run('required_rules', s, { ids: ['money', 'nope'] }), 'fail'); s.instructions.rules[0].severity = 'standing'; assert.equal(run('required_rules', s, { ids: ['money'] }), 'fail'); });
t('required_rules passes', () => assert.equal(run('required_rules', base(), { ids: ['money'] }), 'pass'));
t('forbidden_accounts_unused fails when referenced', () => { const s = base(); s.channels.email.push({ id: 'w', identity: 'work' }); assert.equal(run('forbidden_accounts_unused', s, {}), 'fail'); });

// ---- safety rails
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'sac-selftest-'));
t('importer ctx refuses secret files', () => { fs.writeFileSync(path.join(tmp, '.bot_token'), 'x'); const ctx = makeContext({ home: tmp }); assert.throws(() => ctx.readText('~/.bot_token'), /refused/); });
t('importer ctx refuses launchctl load', () => { const ctx = makeContext({ home: tmp }); assert.throws(() => ctx.exec('launchctl', ['load', 'x.plist']), /not allowed/); assert.throws(() => ctx.exec('launchctl', ['bootstrap', 'gui/1']), /not allowed/); });
t('importer ctx scrubs env values to names', () => { const ctx = makeContext({ home: tmp }); const r = ctx.scrub({ env: { API_KEY: 'v' }, command: 'x' }); assert.deepEqual(r.env_names, ['API_KEY']); assert.ok(!JSON.stringify(r).includes('"v"')); });
t('init refuses protected dirs', () => assert.throws(() => init(path.join(os.homedir(), '.claude', 'x')), /refusing/));
t('init scaffolds a valid setup', () => { const r = init(path.join(tmp, 'p')); const v = validate(normalize(JSON.parse(JSON.stringify(require_yaml(path.join(r.dir, 'setup.yaml')))))); assert.ok(v.ok, v.errors.join('; ')); });
t('schema rejects bad severity', () => { const s = base(); s.instructions.rules[0].severity = 'soft'; assert.equal(validate(s).ok, false); });
t('schema rejects duplicate ids', () => { const s = base(); s.identity.accounts.push({ id: 'bot', kind: 'x', access: 'unknown' }); assert.equal(validate(s).ok, false); });
t('plan sees add/change/untracked', () => { const d = base(); const l = deepMerge(base(), {}); l.identity.accounts = [{ id: 'bot', kind: 'other', access: 'can_post' }, { id: 'extra', kind: 'x', access: 'unknown' }]; const p = plan(d, l); assert.equal(p.totals.add, 1); assert.equal(p.totals.change, 1); assert.equal(p.totals.untracked, 1); });
t('lint flags a canary', () => { const dir = path.join(tmp, 'lintroot'); fs.mkdirSync(dir); fs.writeFileSync(path.join(dir, 'a.md'), `contact: ${FAKE_EMAIL} token ${FAKE_TOKEN}`); const r = lint({ targets: ['.'], root: dir, terms: new Map([['someone', 'test']]) }); assert.ok(r.findings.length >= 3); });

import YAML from 'yaml';
function require_yaml(f) { return YAML.parse(fs.readFileSync(f, 'utf8')); }

fs.rmSync(tmp, { recursive: true, force: true });
process.stdout.write(`${n} test(s) passed${process.exitCode ? ', SOME FAILED' : ''}\n`);
