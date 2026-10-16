# setup-as-code in Rust: design

Date: 2026-10-15 (overnight build). Owner's instruction, verbatim: "overnight, i want you to
create this agent setup as code thing. i know u made the github thing but it hasn't been tested.
it needs to be fast. so maybe just make it in rust. and make sure it works by you testing it. i
will test it myself at 10 am." The owner is asleep; this spec records the decisions taken on his
behalf so he can rework any of them in the morning.

## Goal

One static binary, `sac`, that does what the Node reference (`setup.mjs` + `framework/`) does,
faster and with no runtime dependency, plus the one feature the reference lacks: `apply --live`,
which can rebuild a setup on a fresh machine from `setup.yaml`. Same manifest format, same JSON
schema, same safety contract. The Node implementation is the behavioural reference for parity and
is removed from the repository once the Rust binary passes every gate below.

## Non-goals tonight

- Adapters for agents other than Claude Code (Codex, Cursor). The manifest is agent-neutral; only
  Claude Code importers and renderers ship.
- Installing tools (Node, rtk, Playwright) or resolving secrets from a keychain.
- Running `apply --live` against the owner's real home. It is tested only in sandbox homes.
- Linux execution testing on this laptop (no Docker daemon). CI on GitHub covers Linux.

## Architecture

A single crate, package `setup-as-code`, binary `sac`. The core data model is dynamic:
`type Doc = serde_json::Value` with insertion-ordered maps. The JSON schema (unchanged from the
reference) is the type system; small typed structs appear only where logic needs them (plist
rendering, policy results, plan output). This mirrors the reference exactly, which keeps parity
testable, and avoids a brittle hand-written type hierarchy for a schema full of free-form
objects.

```
src/
  main.rs          clap dispatch, exit codes
  cli.rs           argument structs
  lib.rs           module list, `Doc` alias
  util.rs          paths, slug, deep_merge, get/set path, stable string, glob, vars
  secrets.rs       secret patterns, secret-file names, secret key names
  yaml.rs          YAML <-> Doc, frontmatter
  layers.rs        the 11 layers, collections, scalars, counts
  schema.rs        schema validation + duplicate ids + cross references
  ctx.rs           read-only machine context: home, allowlisted exec, scrub
  importers/       trait + 10 built-ins + external executable importers
  core.rs          instance resolution, desired state, import orchestration, normalize
  plan.rs          desired vs live diff
  policy/          check trait, engine, 9 built-in checks, external executable checks
  lint.rs          secret + PII lint over shareable files
  redact.rs        shape-only copy
  init.rs          scaffold from templates/
  apply.rs         render to out/<instance>/
  live.rs          apply --live: merge into a real home with backups
  output.rs        text formatting, colours when stdout is a TTY
schema/setup.schema.json   unchanged
templates/                 moved from framework/templates
tests/                     integration tests over synthetic homes
fixtures/                  synthetic files (never derived from the owner's machine)
```

## Commands and exit codes (parity with the reference)

| command | reads | writes | exit |
|---|---|---|---|
| `init <dir> [--name] [--force]` | templates | `<dir>/` | 1 on clash or protected dir |
| `import [-i] [--home] [--dry-run]` | machine (read-only) + overrides | `<dir>/setup.yaml` | 1 if imported state fails schema |
| `validate [-i]` | instance | nothing | 1 if invalid |
| `plan [-i] [--home] [--json] [-v]` | instance + machine | nothing | 0 |
| `verify [-i] [--home] [--fail-on-drift] [--json]` | instance + machine | nothing | 1 hard policy failure or invalid schema; 2 drift with the flag |
| `apply [-i] [--out dir]` | instance | `<out>/<instance>/` | 1 if invalid or hard failure |
| `apply --live --yes [--home dir] [--force]` | instance | the home's live config, with backups | 1 on refusal |
| `lint [paths...] [--instance-root dir]` | files | nothing | 1 if findings |
| `redact -i dir --out dir` | instance | `<out>/setup.yaml` | 1 if result invalid |
| `layers`, `importers [-i]` | | | 0 |

Instance resolution: `-i/--instance`, then `$SAC_INSTANCE`, then `./setup.yaml`, then the only
directory under `./instance/`. `--home` makes every importer read that directory as if it were
`$HOME`; `is_real_home` is false there, so machine-wide probes (`launchctl`, `crontab`) are
skipped and sandbox "installed" means enabled.

## Plugin protocol (replaces `.mjs` plugins)

Any executable in `<instance>/importers/` or `<instance>/checks/` is a plugin.

Importer executable:
- `<exe> describe` prints JSON `{"id":"todo-md","layers":["channels"],"description":"..."}`.
- `<exe> detect` exits 0 when the importer applies. Env: `SAC_HOME`, `SAC_REAL_HOME` (`0`/`1`).
- `<exe> run` reads the importer's options as JSON on stdin and prints a JSON fragment of
  `setup.yaml` on stdout. Same env.
An importer whose id equals a built-in replaces it.

Check executable:
- `<exe> describe` prints `{"id":"max_enabled_loops","doc":"..."}`.
- `<exe> run` reads `{"setup":{...},"params":{...},"ctx":{"home":"...","out_root":"..."}}` on
  stdin and prints `{"status":"pass|fail|warn|skip","message":"...","evidence":[...]}`.

`examples/minimal/importers/todo-md` and `examples/minimal/checks/max-enabled-loops` are
rewritten as executables (`#!/usr/bin/env python3`) that implement this protocol.

## `apply --live`

Rendering to `out/` is unchanged. `--live` additionally merges into a home directory:

- `~/.claude/settings.json`: managed keys only, `permissions.allow|deny|ask`, `defaultMode`,
  `deniedMcpServers`, `hooks`, `enabledPlugins`, `model`, `effortLevel`. Every other key, and
  every `env` value, is preserved from the existing file.
- `~/.claude.json`: `mcpServers` entries are added when missing. An existing entry keeps its
  `env` and `headers` values; its `command`, `args`, `url`, `type` are rewritten only with `--force`.
  Servers listed in `tools.denied_mcp_servers` or `enabled: false` are never added.
- `~/Library/LaunchAgents/<label>.plist` for every launchd loop with `managed != false` and a
  program: written when missing, overwritten only with `--force`. Disabled loops go to
  `~/Library/LaunchAgents/disabled/`. Nothing is ever loaded; the `launchctl bootstrap` commands
  are printed for the person to run.
- `~/.claude/setup-as-code/RULES.md`, `setup.lock.json`, `report.md`: always written.
- Before the first write, every file that will change is copied to
  `<home>/.setup-as-code/backups/<UTC timestamp>/<path relative to home>`.
- Refuses without `--yes`. Refuses if the desired state is invalid or a hard policy fails.

## Performance target

`sac import --dry-run` on the owner's machine (about 490 memory files, 39 plists, 10 MCP servers,
a 5,000-line manifest) in under one second wall clock, and `plan` likewise. Achieved by parsing
plists in-process (`plist` crate), scanning `PATH` in-process instead of spawning `which`, and
reading `.git/config` instead of spawning `git`. Only `launchctl list`, `launchctl print-disabled`
and `crontab -l` are spawned, once each, and only for the real home.

## Known differences from the reference (deliberate)

- `infra.repos[].last_commit` is dropped (it needed one `git` spawn per repo and the planner
  already ignored it as volatile).
- Plugins are executables, not `.mjs` modules.
- `out/` is `./out` relative to the working directory, or `--out`.
- `lint` scans the working directory by default, skipping `target/`, `node_modules/`, `out/`,
  `.git/` and `instance/`.
- `apply --live` exists.
- `overrides.yaml`'s `import` block merges over `setup.yaml`'s (overrides win), so a freshly
  scaffolded instance can set `managed_prefixes` and `repo_roots` before the first import.
- `init` protects the `--home` sandbox's live-config paths, not only the real home's.
- `claude/mcp-servers.json` (render) and `apply --live` skip MCP servers whose id contains `@`
  (project-scoped or source-suffixed duplicates), so a project server never overwrites the
  user-scoped entry of the same name. Live also skips servers with neither command nor url.
- `apply --live` never writes `settings.json` `env` values (the manifest holds names only); the
  names that need values are listed in the live `report.md`. `hooks` is replaced as a whole.
- launchd `schedule.watch_paths` is written with `~` paths; `schedule.calendar` keeps the key
  order of the plist file, where the reference used the order `plutil` happened to emit.
- External plugin output is scrubbed like built-in output; a fragment that is not a JSON
  object is rejected; dotfiles and `name~` backups in `importers/` and `checks/` are skipped.
- `managed_prefixes` and `repo_roots` accept a single string as well as a list.
- `infra.repos[].remote` comes from `.git/config`'s `[remote "origin"] url` only; `insteadOf`
  rewrites, includes and global git config are not applied. `infra.machine.node` is dropped;
  on Linux `os` is `PRETTY_NAME` from `/etc/os-release`.
- Plugin timeouts (20 s) kill the plugin process, not its process group: a wrapper script must
  `exec` its worker and must not leave children holding stdout.
- Cron loop ids are, as in the reference, a slug of the raw command line.

## Safety contract (unchanged, enforced in code and tests)

- Importers see the machine only through `Ctx`, which refuses secret-looking files and runs only
  allowlisted read-only commands. No `launchctl load|bootstrap|kickstart|kill`.
- Env var and header values become names. Secret-shaped strings are dropped with a warning.
- `apply` without `--live` writes only below the out root and refuses an out root inside
  `~/.claude`, `~/Library/LaunchAgents`, `~/Library/LaunchDaemons`, `/Library`, `/etc`,
  `/System`, `~/.config/systemd`.
- Nothing calls a network API, posts to a chat or sends email.
- The lint fails if any instance-derived term, email, phone number, home path or chat id appears
  in the shareable tree; the test suite plants canaries and asserts the lint catches them.

## Acceptance gates (all must pass before the Node implementation is deleted)

1. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` green.
2. Integration tests over a synthetic home: init, validate, import counts, plan detects an added
   skill and a removed loop, verify exits 1 on a hard failure, apply renders plists that pass
   `plutil -lint`, lint catches canaries, redact validates.
3. Round trip: import from synthetic home A, `apply --live --yes --home B` into an empty home B,
   import from B, plan A-desired vs B-live shows zero drift on the managed collections.
4. Parity: Rust and Node imports of the same synthetic home agree on every layer count and on
   every item id (script `scripts/parity.sh`, run once, output kept in the handoff).
5. Read-only run on the owner's machine: `import --dry-run`, `plan`, `verify` complete, counts
   match the Node report within the known differences, wall clock under one second.
6. No file under `~/.claude`, `~/.claude.json` or `~/Library/LaunchAgents` has a newer mtime
   than the build start.
