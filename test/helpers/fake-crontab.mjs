#!/usr/bin/env node
// Stand-in for `crontab`: the table lives in $HOME/.sac-test-crontab.
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const argv = process.argv.slice(2);
const file = join(process.env.HOME, ".sac-test-crontab");
const stdin = argv[0] === "-" ? readFileSync(0, "utf8") : undefined;
appendFileSync(join(process.env.HOME, ".sac-fake-log.jsonl"), JSON.stringify({ bin: "crontab", argv, stdin, configDir: null }) + "\n");
if (argv[0] === "-l") {
  if (!existsSync(file)) { process.stderr.write("crontab: no crontab for test\n"); process.exit(1); }
  process.stdout.write(readFileSync(file, "utf8"));
  process.exit(0);
}
if (argv[0] === "-") { writeFileSync(file, stdin); process.exit(0); }
process.exit(2);
