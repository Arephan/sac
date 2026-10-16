#!/usr/bin/env bash
# Parity check between the Rust binary and the Node reference over the same synthetic home.
# Usage: scripts/parity.sh <home-dir> <instance-dir>   (needs node + the repo's node_modules)
set -euo pipefail
cd "$(dirname "$0")/.."
HOME_DIR="${1:?home dir}"; INST="${2:?instance dir}"
SAC="${SAC:-./target/release/sac}"; [ -x "$SAC" ] || SAC="./target/debug/sac"
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
cp -R "$INST" "$work/rs"; cp -R "$INST" "$work/js"
"$SAC" import -i "$work/rs" --home "$HOME_DIR" >/dev/null
# The Node prototype lives at tag v0.2.0. Use a checkout of it when this tree no longer has it.
REF="."
if [ ! -f setup.mjs ]; then
  REF="$work/ref"
  git worktree add -q "$REF" v0.2.0
  trap 'git worktree remove --force "$REF" 2>/dev/null; rm -rf "$work"' EXIT
  (cd "$REF" && npm install --silent --no-audit --no-fund >/dev/null)
fi
(cd "$REF" && node setup.mjs import -i "$work/js" --home "$HOME_DIR" >/dev/null)
NODE_PATH="$REF/node_modules" node --input-type=module - "$work/rs/setup.yaml" "$work/js/setup.yaml" <<'JS'
import fs from 'node:fs';
import { createRequire } from 'node:module';
const YAML = createRequire(process.env.NODE_PATH + '/')('yaml');
const [rsFile, jsFile] = process.argv.slice(2);
const rs = YAML.parse(fs.readFileSync(rsFile, 'utf8'));
const js = YAML.parse(fs.readFileSync(jsFile, 'utf8'));
const paths = ["identity.accounts","brain.runtimes","brain.roster","brain.gates","instructions.sources","instructions.rules",
 "tools.mcp_servers","tools.skills","tools.plugins","tools.hooks","tools.commands","tools.permissions.allow",
 "tools.permissions.deny","tools.permissions.ask","tools.denied_mcp_servers","loops","channels.slack","channels.email",
 "channels.social","channels.calendar","channels.todo","channels.journal","projects","business.lanes","data.state",
 "data.references","infra.clis","infra.repos","infra.services","safety.policies"];
const get = (d, p) => p.split('.').reduce((o, k) => (o == null ? undefined : o[k]), d) || [];
const ids = (l) => l.map((x) => (x && typeof x === 'object' ? x.id : x)).sort();
let bad = 0;
for (const p of paths) {
  const a = ids(get(rs, p)), b = ids(get(js, p));
  if (JSON.stringify(a) !== JSON.stringify(b)) {
    bad++;
    console.log(`DIFF ${p}: rust=${a.length} node=${b.length}`);
    console.log('  only rust:', a.filter((x) => !b.includes(x)).slice(0, 10));
    console.log('  only node:', b.filter((x) => !a.includes(x)).slice(0, 10));
  } else console.log(`ok   ${p}: ${a.length}`);
}
console.log('PARITY', bad ? 'FAILED' : 'OK', `(${bad} collection(s) differ)`);
process.exit(bad ? 1 : 0);
JS
