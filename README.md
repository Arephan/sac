# setup-as-code

Terraform for a personal AI assistant. One YAML file describes everything your assistant is and
does: who it acts for, which models it uses, the rules it follows, its tools, its scheduled loops,
where it talks, what it works on, where its state lives, and what it must never do. A single
binary, `sac`, keeps that file and your machine honest with each other.

```
sac import    read the machine (read-only)              -> setup.yaml
sac validate  schema, duplicate ids, cross-references
sac plan      setup.yaml vs machine, item by item        -> printed diff, writes nothing
sac verify    plan + safety policies (hard = exit 1)
sac apply     render config from setup.yaml             -> out/<instance>/ only
sac apply --live --yes   rebuild a home from setup.yaml, with backups, nothing loaded
sac lint      prove the shareable parts hold no secrets or personal data
```

No runtime, no daemon, no network. Claude Code is the first agent with importers; the manifest
itself is agent-neutral.

Fast enough to run on every prompt: on a real setup with 216 rules, about 500 memory files, 60
launchd jobs and 10 MCP servers, `sac import --dry-run` takes 0.1 s warm (0.5 s cold) and
`verify` 0.3 s, against 1.8 s for the Node prototype it replaced.

## Why

Everyone who runs an assistant for real ends up with a pile of `CLAUDE.md` files, hooks, skills,
MCP configs, memory directories and launchd jobs that nobody can reproduce, share or audit. Rules
that matter ("never spend money", "never post from the work account") live in prose with nothing
checking them. `sac` makes the setup a file, diffs the file against reality, and turns the rules
that matter into checks that fail a build.

## Install

```bash
# from source (needs Rust, https://rustup.rs)
git clone https://github.com/Arephan/setup-as-code.git && cd setup-as-code
cargo install --path .            # puts `sac` in ~/.cargo/bin
# or: scripts/install.sh           # builds and copies to ~/.local/bin
```

Release binaries for macOS (Apple silicon and Intel) and Linux x86_64 are attached to each
GitHub release.

## Ten-minute quickstart

```bash
# 1. scaffold your instance: setup.yaml, policies.yaml, overrides.yaml, lint-denylist.txt
sac init instance/me

# 2. check the starter is valid
sac validate -i instance/me

# 3. see what your machine actually has, without changing anything
sac import -i instance/me --dry-run      # counts only
sac plan   -i instance/me                # starter file vs your machine

# 4. adopt the machine's real state into setup.yaml
#    Put hand-written facts in instance/me/overrides.yaml first, e.g. which launchd label
#    prefixes are yours under import.options.launchd.managed_prefixes.
sac import -i instance/me

# 5. write your rules and policies, then verify (exit 1 if a hard policy fails)
$EDITOR instance/me/policies.yaml
sac verify -i instance/me

# 6. render config to look at or copy by hand
sac apply -i instance/me && ls out/me
```

Want to try it without touching your real home? Add `--home /tmp/empty-home` to any import, plan,
verify or apply command and the importers read that folder instead. `scripts/demo-roundtrip.sh`
does the whole loop in a temp directory: capture home A, rebuild it into empty home B, show that
B now matches.

## Framework vs instance

The repository is split in two, and the split is enforced by the lint.

| | Framework (shareable) | Instance (yours, private) |
|---|---|---|
| Where | `src/`, `schema/`, `templates/`, `examples/`, `docs/` | `instance/<name>/` (git-ignored) or any folder you pass with `-i` |
| What | the binary, schema, layer definitions, importers, policy checks, templates | `setup.yaml`, `overrides.yaml`, `policies.yaml`, `lint-denylist.txt`, custom `importers/` and `checks/` |
| Holds personal data? | Never. `sac lint` fails if it does. | Yes, by reference: paths, account roles, secret *locations* (never values). |

The framework knows nothing about any person. Paths are `~/...` or `${env:HOME}`; accounts are
named by role; machine-specific choices (which launchd labels are yours, which CLIs to probe)
are importer options set in the instance.

## Commands

| command | reads | writes | exit code |
|---|---|---|---|
| `init <dir> [--name n] [--force]` | templates | `<dir>/` (refuses to scaffold inside live config) | 1 on clash |
| `import [-i dir] [--home h] [--dry-run]` | machine (read-only), `overrides.yaml` | `<dir>/setup.yaml` | 1 if the result fails the schema |
| `validate [-i dir]` | instance | nothing | 1 if invalid |
| `plan [-i dir] [--home h] [--json] [-v]` | instance + machine | nothing | 0 |
| `verify [-i dir] [--home h] [--fail-on-drift] [--json]` | instance + machine | nothing | 1 hard failure, 2 drift with the flag |
| `apply [-i dir] [--out o]` | instance | `<out>/<instance>/` only | 1 if invalid or a hard policy fails |
| `apply --live --yes [--home h] [--force]` | instance | the home's live config, after backups | 1 on refusal |
| `lint [paths...] [--instance-root dir]` | files | nothing | 1 on any finding |
| `redact -i dir --out d` | instance | `d/setup.yaml`, a shape-only copy | 1 if the result is invalid |
| `layers`, `importers` | | | 0 |

Instance resolution: `-i`, then `$SAC_INSTANCE`, then `./setup.yaml`, then the only folder under
`./instance/`.

## What `apply --live` does, exactly

Rendering to `out/` never touches live config. `--live` additionally merges into a home:

- `~/.claude/settings.json`: managed keys only (`permissions.allow|deny|ask`, `defaultMode`,
  `deniedMcpServers`, `hooks`, `enabledPlugins`, `model`, `effortLevel`). Every other key and
  every `env` value is kept.
- `~/.claude.json`: missing MCP servers are added. An existing server keeps its `env` and
  `headers` values; its command, args and URL change only with `--force`. Denied or disabled
  servers are never added.
- `~/Library/LaunchAgents/<label>.plist` for every managed launchd loop with a program: written
  when missing, overwritten only with `--force`; disabled loops go under `disabled/`. Nothing is
  ever loaded. The `launchctl bootstrap` commands are printed for you to run.
- `~/.claude/setup-as-code/RULES.md`, `setup.lock.json`, `report.md`.
- Before the first write, every file that will change is copied to
  `~/.setup-as-code/backups/<timestamp>/`.

Skills, memory files and plugin installs are things the manifest points at, not things it can
recreate; they stay where they are.

## Plugins: importers and checks in any language

Any executable in `<instance>/importers/` or `<instance>/checks/` is a plugin.

An importer answers `describe` (prints `{"id","layers","description"}`), `detect` (exit 0 when it
applies; env `SAC_HOME`, `SAC_REAL_HOME`) and `run` (reads its options as JSON on stdin, prints a
fragment of `setup.yaml`). A check answers `describe` (`{"id","doc"}`) and `run` (reads
`{"setup","params","ctx"}` on stdin, prints `{"status","message","evidence"}`). Working examples:
`examples/minimal/importers/todo-md` and `examples/minimal/checks/max-enabled-loops`.

## Safety contract

Enforced in code and covered by tests, not just promised:

- Importers see the machine only through a context that refuses to open secret-looking files
  (`*token*`, `*_key`, `.env`, `auth.json`, keys, cookies) and runs only allowlisted read-only
  commands (`launchctl list|print-disabled`, `crontab -l`, `sw_vers`, `uname`, `git remote|log`).
  There is no load, unload, bootstrap or kill anywhere in the binary.
- Env vars and HTTP headers are imported as names only. Secret-shaped strings are dropped.
- `apply` without `--live` writes only below the out root and refuses an out root inside
  `~/.claude`, `~/Library/LaunchAgents` or other live config.
- Nothing calls a paid API, posts to a chat or sends email.
- `sac lint` fails if any term from your instance (names, handles, workspaces, the home user),
  any email, phone number, home path or chat id appears in the shareable tree.

`scripts/parity.sh` compares imports against the Node prototype this binary replaced (tag
`v0.2.0`); all thirty collections agreed on the last run. `cargo test` runs the unit suite plus end-to-end tests over a synthetic home: import counts,
drift detection, exit codes, rendered plists checked with `plutil -lint`, the lint against
planted canaries, and a full round trip through `apply --live` into an empty home.

## Layout

```
src/
  main.rs cli.rs             the binary
  core.rs                    instance resolution, desired state, import orchestration
  ctx.rs                     read-only machine context, exec allowlist, scrubbing
  importers/                 claude-settings, claude-mcp, claude-plugins, claude-skills,
                             claude-instructions, claude-memory, launchd, cron, infra,
                             file-state, external (executable plugins)
  policy/                    check engine, nine built-in checks, external checks
  plan.rs apply.rs live.rs   diff, render, live merge with backups
  lint.rs redact.rs init.rs
schema/setup.schema.json     the manifest schema (draft-07)
templates/                   what `init` scaffolds
examples/minimal             a small hand-written setup with one importer and one check plugin
examples/large-redacted      shape-only redaction of a real, large setup (counts and kinds only)
docs/                        layers.md importers.md policies.md safety.md
instance/                    your data (git-ignored)
```

## Docs

- [docs/layers.md](docs/layers.md): every layer and its fields
- [docs/importers.md](docs/importers.md): built-in importers and the plugin protocol
- [docs/policies.md](docs/policies.md): hard vs standing rules, policy checks, custom checks
- [docs/safety.md](docs/safety.md): secrets, the lint, what is never touched

## Status

v0.3, early. Importers cover Claude Code today, plus launchd, cron and git repos. Adapters for
other agents are the obvious next step; the manifest and the policy checks do not depend on the
agent. Issues and PRs welcome.

## License

MIT.
