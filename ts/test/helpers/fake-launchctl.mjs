#!/usr/bin/env node
// Stand-in for `launchctl`: loaded labels live in $HOME/.sac-test-loaded, one per line.
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";

const argv = process.argv.slice(2);
const file = join(process.env.HOME, ".sac-test-loaded");
appendFileSync(join(process.env.HOME, ".sac-fake-log.jsonl"), JSON.stringify({ bin: "launchctl", argv, configDir: null }) + "\n");
const loaded = existsSync(file) ? readFileSync(file, "utf8").split("\n").filter(Boolean) : [];
const save = (l) => writeFileSync(file, l.join("\n") + "\n");
const [cmd, a, b] = argv;
const labelOf = (domainOrService, plistPath) => (plistPath ? basename(plistPath, ".plist") : domainOrService.split("/").slice(2).join("/"));
if (cmd === "print") process.exit(loaded.includes(labelOf(a)) ? 0 : 113);
if (cmd === "bootstrap") { save([...new Set([...loaded, labelOf(a, b)])]); process.exit(0); }
if (cmd === "bootout") {
  const l = labelOf(a, b);
  if (!loaded.includes(l)) process.exit(3);
  save(loaded.filter((x) => x !== l));
  process.exit(0);
}
process.exit(2);
