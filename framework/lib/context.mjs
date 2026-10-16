// The sandboxed context handed to every importer. Importers only ever see the machine
// through this object, which enforces the read-only + no-secret-values contract.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { expandHome, tilde, isSecretFile, SECRET_KEY_RE, findSecrets, isPlainObject } from './util.mjs';

// Read-only system commands importers may run. Anything that loads, unloads, writes or
// talks to the network is absent on purpose.
const EXEC_ALLOW = {
  plutil: (a) => a[0] === '-convert' && a[1] === 'json' && a[2] === '-o' && a[3] === '-',
  launchctl: (a) => ['list', 'print-disabled'].includes(a[0]),
  crontab: (a) => a.length === 1 && a[0] === '-l',
  sw_vers: () => true,
  uname: () => true,
  which: () => true,
  git: (a) => a[0] === '-C' && ['remote', 'rev-parse', 'log'].includes(a[2]) && !a.includes('set-url') && !a.includes('add') && !a.includes('remove'),
  systemctl: (a) => a[0] === '--user' && ['list-timers', 'list-unit-files'].includes(a[1]),
};

export function makeContext({ home, options = {}, log = () => {} } = {}) {
  const realHome = os.homedir();
  home = path.resolve(home || process.env.HOME || realHome);
  const warnings = [];
  const ctx = {
    home,
    // True only when importing the machine's real user. Machine-wide probes (launchctl,
    // crontab) are skipped for a sandboxed --home so a "new person" test sees nothing.
    isRealHome: home === realHome,
    platform: process.platform,
    options,
    warnings,
    warn(msg) { warnings.push(msg); log(`  ! ${msg}`); },
    p(rel) { return expandHome(rel, home); },
    tilde(p) { return tilde(p, home); },
    exists(p) { try { fs.accessSync(ctx.p(p)); return true; } catch { return false; } },
    isDir(p) { try { return fs.statSync(ctx.p(p)).isDirectory(); } catch { return false; } },
    list(p) { try { return fs.readdirSync(ctx.p(p)); } catch { return []; } },
    stat(p) { try { return fs.statSync(ctx.p(p)); } catch { return null; } },
    readText(p) {
      const abs = ctx.p(p);
      if (isSecretFile(abs)) throw new Error(`refused to read secret-looking file ${ctx.tilde(abs)}`);
      return fs.readFileSync(abs, 'utf8');
    },
    readJSON(p) { return JSON.parse(ctx.readText(p)); },
    tryJSON(p) { try { return ctx.readJSON(p); } catch { return null; } },
    exec(cmd, args = []) {
      const ok = EXEC_ALLOW[cmd];
      if (!ok || !ok(args)) throw new Error(`exec not allowed: ${cmd} ${args.join(' ')}`);
      return execFileSync(cmd, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'], timeout: 15000 });
    },
    tryExec(cmd, args) { try { return ctx.exec(cmd, args); } catch { return null; } },
    // Keep only the NAMES under secret-ish keys; scrub anything value-shaped elsewhere.
    namesOnly(obj) {
      if (!isPlainObject(obj)) return [];
      return Object.keys(obj).sort();
    },
    scrub(value) {
      if (typeof value === 'string') {
        let v = tilde(value, home);
        if (findSecrets(v).length) { ctx.warn(`dropped a secret-looking value (${findSecrets(v)[0].id})`); return '<redacted>'; }
        return v;
      }
      if (Array.isArray(value)) return value.map((x) => ctx.scrub(x));
      if (isPlainObject(value)) {
        const out = {};
        for (const [k, v] of Object.entries(value)) {
          if (SECRET_KEY_RE.test(k)) out[k.toLowerCase() + '_names'] = ctx.namesOnly(v);
          else out[k] = ctx.scrub(v);
        }
        return out;
      }
      return value;
    },
  };
  return ctx;
}
