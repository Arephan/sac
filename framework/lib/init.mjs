// init: scaffold a new person's instance folder from the commented templates.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { FRAMEWORK_ROOT, expandHome, slug } from './util.mjs';

const T = (f) => path.join(FRAMEWORK_ROOT, 'framework/templates', f);
const PROTECTED = ['~/.claude', '~/Library/LaunchAgents', '~/Library/LaunchDaemons', '~/.ssh'];

export function init(dir, { name, force = false } = {}) {
  const abs = path.resolve(dir);
  for (const p of PROTECTED) {
    const prot = path.resolve(expandHome(p, os.homedir()));
    if (abs === prot || abs.startsWith(prot + path.sep)) throw new Error(`refusing to scaffold inside ${p}`);
  }
  const instance = slug(name || path.basename(abs), 40);
  const files = {
    'setup.yaml': fs.readFileSync(T('setup.template.yaml'), 'utf8').replace('{{instance}}', instance),
    'policies.yaml': fs.readFileSync(T('policies.template.yaml'), 'utf8'),
    'overrides.yaml': fs.readFileSync(T('overrides.template.yaml'), 'utf8'),
    'lint-denylist.txt': fs.readFileSync(T('lint-denylist.template.txt'), 'utf8'),
  };
  fs.mkdirSync(abs, { recursive: true });
  const clash = Object.keys(files).filter((f) => fs.existsSync(path.join(abs, f)));
  if (clash.length && !force) throw new Error(`${abs} already has ${clash.join(', ')} (use --force to overwrite)`);
  for (const [f, c] of Object.entries(files)) fs.writeFileSync(path.join(abs, f), c);
  fs.mkdirSync(path.join(abs, 'importers'), { recursive: true });
  fs.mkdirSync(path.join(abs, 'checks'), { recursive: true });
  return { dir: abs, instance, files: Object.keys(files) };
}
