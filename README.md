# setup-as-code

Terraform for a personal AI assistant. One YAML file describes everything your assistant is and
does (who it acts for, which models it uses, the rules it follows, its tools, its scheduled loops,
where it talks, what it works on, where its state lives, what it must never do), and a small CLI
keeps that file and your machine honest with each other.

```
import    read the machine (read-only)            -> setup.yaml
validate  schema, duplicate ids, cross-references
plan      setup.yaml vs machine, item by item     -> printed diff, writes nothing
verify    plan + safety policies (hard = exit 1)
apply     render config from setup.yaml           -> out/<instance>/ only
lint      prove the shareable parts hold no secrets or personal data
```

## Framework vs instance

The repository is split in two, and the split is enforced by a lint.

| | Framework (shareable) | Instance (yours, private) |
|---|---|---|
| Where | `setup.mjs`, `framework/`, `examples/`, `docs/` | `instance/<name>/` (git-ignored) or any folder you pass with `-i` |
| What | schema, layer definitions, importers, policy engine, templates | `setup.yaml`, `overrides.yaml`, `policies.yaml`, `lint-denylist.txt`, custom `importers/` and `checks/` |
| Holds personal data? | Never. `node setup.mjs lint` fails if it does. | Yes, by reference: paths, account roles, secret *locations* (never values). |

The framework knows nothing about any person. Paths are `~/...` or `${env:HOME}`; accounts are
named by role; machine-specific choices (which launchd labels are yours, which CLIs to probe)
are importer options set in the instance.

## 10-minute quickstart

You need Node 20+ and macOS or Linux.

```bash
# 1. get the framework (1 min)
git clone https://github.com/Arephan/setup-as-code.git && cd setup-as-code && npm install

# 2. scaffold your instance (1 min). Creates setup.yaml, policies.yaml, overrides.yaml, lint-denylist.txt
node setup.mjs init instance/me

# 3. check the starter is valid (10 s)
node setup.mjs validate -i instance/me

# 4. see what your machine actually has, without changing anything (1 min)
node setup.mjs import -i instance/me --dry-run      # counts only
node setup.mjs plan   -i instance/me                # starter file vs your machine

# 5. adopt the machine's real state into setup.yaml (1 min)
#    Edit instance/me/overrides.yaml first if you want to keep hand-written facts,
#    e.g. launchd label prefixes under import.options.launchd.managed_prefixes.
node setup.mjs import -i instance/me

# 6. write your rules and policies (5 min): see docs/policies.md
$EDITOR instance/me/policies.yaml
node setup.mjs verify -i instance/me                # exit 1 if a hard policy fails

# 7. render config to look at or copy by hand (10 s)
node setup.mjs apply -i instance/me && ls out/me
```

Put your name, company, handles and workspace names in `instance/me/lint-denylist.txt`; from then
on `node setup.mjs lint` proves none of them leak into the shareable framework.

Want to try it without touching your real home dir? Add `--home /tmp/empty-home` to any
import/plan/verify command and the importers read that folder instead.

## Commands

| command | reads | writes |
|---|---|---|
| `init <dir>` | templates | `<dir>/` (refuses to scaffold inside protected dirs) |
| `import [-i dir] [--dry-run]` | machine (read-only), `overrides.yaml` | `<dir>/setup.yaml` |
| `validate` | instance | nothing |
| `plan [--json] [-v]` | instance + machine | nothing |
| `verify [--fail-on-drift]` | instance + machine | nothing (exit 1 hard failure, 2 drift with the flag) |
| `apply` | instance | `out/<instance>/` only |
| `lint [paths]` | framework, examples, docs, every instance's denylist | nothing |
| `redact -i dir --out d` | instance | `d/setup.yaml`, a shape-only copy (see `examples/large-redacted`) |
| `layers`, `importers` | | |

`npm test` runs the self-test: every policy check must fail on a bad fixture and pass on a good
one, and the safety rails (secret-file guard, exec allowlist, apply/init guards) must hold.

## Safety contract

These are enforced in code, not just promised:

- Importers see the machine only through a context object that refuses to open secret-looking
  files (`*token*`, `*_key`, `.env`, `auth.json`, keys, cookies...) and only runs allowlisted
  read-only commands (`plutil -convert json -o -`, `launchctl list|print-disabled`, `crontab -l`,
  `which`, `git -C <repo> remote|log|rev-parse`, ...). There is no load, unload, bootstrap or kill.
- Env vars and HTTP headers of MCP servers and jobs are imported as names only.
- `apply` writes only below `out/` and refuses if `out/` resolves into `~/.claude`,
  `~/Library/LaunchAgents` or other live config. It never installs, loads or sends anything.
- Nothing in the framework calls a paid API, posts to a chat or sends email.

## Layout

```
setup.mjs                    CLI
framework/
  schema/setup.schema.json   the manifest schema (draft-07)
  layers/index.mjs           the 11 layers and the collections plan diffs
  importers/                 built-in importers (one file each) + registry
  policy/checks.mjs          built-in policy checks; engine.mjs runs them
  lib/                       core (load/import/validate), plan, apply, lint, redact, init
  templates/                 what `init` scaffolds
  selftest.mjs               npm test
examples/
  minimal/                   small hand-written setup with a custom importer and a custom check
  large-redacted/              shape-only redaction of a real, large setup (counts and kinds only)
docs/
  layers.md  importers.md  policies.md  safety.md
instance/                    your data (git-ignored)
out/                         apply output (git-ignored)
```

## Docs

- [docs/layers.md](docs/layers.md): every layer and its fields
- [docs/importers.md](docs/importers.md): built-in importers and how to write your own
- [docs/policies.md](docs/policies.md): hard vs standing rules, policy checks, custom checks
- [docs/safety.md](docs/safety.md): secrets, the lint, what is never touched

## Status

v0.2, early. Importers cover Claude Code today (settings, MCP, plugins, skills, CLAUDE.md, memory), plus launchd, cron and git repos. Adapters for other agents are planned. `apply` renders to `out/` only; it does not install anything yet.

## License

MIT.
