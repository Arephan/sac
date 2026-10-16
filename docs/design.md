# Design notes

How `sac` is put together, what it promises, and where it deliberately departs from the Node
prototype it replaced (kept at tag `v0.2.0` for comparison).

## Shape

One crate, one binary. The data model is dynamic on purpose: a document is a
`serde_json::Value` with insertion-ordered maps, and `schema/setup.schema.json` (JSON Schema
draft-07) is the type system. Typed structs appear only where logic needs them: plan output,
policy results, plist rendering.

```
src/
  main.rs cli.rs             command dispatch, exit codes
  core.rs                    instance resolution, desired state, import orchestration
  ctx.rs                     read-only machine context: home, exec allowlist, scrubbing
  util.rs secrets.rs yaml.rs layers.rs schema.rs
  importers/                 trait + built-ins + executable plugins
  policy/                    check trait, engine, built-in checks, executable checks
  plan.rs                    desired vs live diff by layer collection
  apply.rs live.rs           render to out/, merge into a home with backups
  lint.rs redact.rs init.rs
```

## Import: observed versus declared

Every importer returns a fragment shaped like `setup.yaml`. A collection an importer returned,
even empty, counts as observed and replaces what the manifest had; a collection no importer
returned is carried over from the manifest, so hand-declared facts (gates, lanes, the reason a
rule exists) never show up as drift. `overrides.yaml` is deep-merged on top (lists merge by `id`;
a map keyed by id merges onto the matching items), then `policies.yaml`'s `safety` block. The
`import` block in `overrides.yaml` wins over the one in `setup.yaml`.

## Plan

Collections keyed by `id` diff item by item and report the changed field names; plain string
lists diff as sets; scalar objects (`brain.claude_code`, `data.memory`, `infra.machine`) diff
field by field. `generated_at` and `last_commit` are ignored, and a loop that is `running`
equals one that is `loaded`.

## Safety contract

Enforced in code and covered by tests:

- Importers reach the machine only through `Ctx`, which refuses to open files whose path looks
  like a credential (`*token*`, `*secret*`, `*_key`, `.env`, `auth.json`, keys, cookies) and
  runs only `launchctl list`, `launchctl print-disabled`, `crontab -l`, `sw_vers` and `uname`.
  Launchd plists and `.git/config` are parsed in-process. Nothing loads, bootstraps or kills a
  job anywhere in the binary.
- Env var and HTTP header values are imported as names only; secret-shaped strings are dropped
  with a warning. Free text (rules, descriptions, permission entries) is scrubbed too.
- `${env:NAME}` is never expanded inside keys that name a credential (`credential_ref`, `env`,
  `headers`, `token`, ...), and any secret-shaped value in the desired state becomes
  `<redacted>` before it can be rendered.
- `apply` writes only below the out root, refuses an out root or instance directory inside live
  config (`~/.claude`, `~/.claude.json`, `~/Library/LaunchAgents`, `~/Library/LaunchDaemons`,
  `/Library`, `/etc`, `/System`, `~/.config/systemd`), and refuses to write through symlinks.
  `import`, `init` and `redact` apply the same symlink and protected-path rules to their own
  outputs.
- `apply --live --yes` writes into one home and nowhere else: every target and the backup
  directory are resolved first and refused when they land outside the canonical home or inside
  the real home's protected config from a sandbox. Files that will change are copied to
  `~/.setup-as-code/backups/<UTC stamp>/` before the first write. Nothing is loaded; the
  `launchctl bootstrap` commands are printed.
- `sac lint` fails on any term derived from your instance, any email, phone number, home path
  or chat id in the shareable tree. CI runs it on every push.

## `apply --live`, exactly

- `settings.json`: `permissions.allow|deny|ask`, `defaultMode`, `enabledPlugins`, `model` and
  `effortLevel` are written from the manifest. `hooks` and `deniedMcpServers` are written only
  when the file lacks them or with `--force`, because the manifest's model of them is lossy (a
  hook keeps only its command, a denial only its name). Every other key, and every `env` value,
  is kept.
- `~/.claude.json`: missing user-scoped MCP servers are added. An existing server keeps its
  `env` and `headers`; command, args and URL change only with `--force`. Denied, disabled,
  project-scoped and `name@source` duplicate servers are never added.
- `~/Library/LaunchAgents/<label>.plist`: written when missing, regenerated from the manifest
  only with `--force` (keys the manifest does not carry are dropped then); disabled loops go
  under `disabled/`.
- `~/.claude/setup-as-code/RULES.md`, `setup.lock.json`, `report.md`: always written.
- A file whose content would contain `<redacted>` is skipped and reported.
- Quit Claude Code before a live run; it writes `~/.claude.json` itself.

## Plugins

Any executable in `<instance>/importers/` or `<instance>/checks/`. Importers answer `describe`
(`{id, layers, description}`), `detect` (exit 0 = applies; env `SAC_HOME`, `SAC_REAL_HOME`) and
`run` (options JSON on stdin, fragment on stdout). Checks answer `describe` (`{id, doc}`) and
`run` (`{setup, params, ctx}` on stdin, `{status, message, evidence}` on stdout, exit 0 after
printing). Twenty-second limit per call; the limit kills the plugin process only, so wrappers
must `exec` their worker. Unix only for now.

## Differences from the Node prototype

- Plugins are executables, not JavaScript modules.
- `infra.repos[].remote` is read from `.git/config` (`[remote "origin"] url`, credentials
  stripped); `insteadOf` rewrites and global git config are not applied; there is no
  `last_commit`; `infra.machine` has no `node` field; on Linux `os` is `PRETTY_NAME`.
- `managed_prefixes` and `repo_roots` accept a single string as well as a list.
- `schedule.watch_paths` is written with `~`; `schedule.calendar` keeps the key order of the
  plist file.
- The launchd importer runs on any OS when the home is a sandbox; the `launchctl` probes run
  only on a real macOS home. Dotfiles in `LaunchAgents` are skipped.
- Cron loop ids are a slug of the raw command line, as before.
- Permission entries: the `//<home>/` form becomes `~/`, which Claude Code reads identically;
  single-slash absolute paths and `Bash(...)` rules stay exactly as written.
- Free text is scrubbed (paths to `~`, secret shapes to `<redacted>`); truncation counts
  Unicode scalar values rather than UTF-16 units.
- Secret patterns use ASCII word boundaries, as JavaScript's `\b` does; files are decoded
  lossily.
- Policy parameters: a scalar where a list is expected counts as a one-item list; a null list
  parameter fails the check closed with a `check crashed:` message.
- The planner applies its volatile-key and running/loaded rules to an item's own fields.
- `out/` is `./out` relative to the working directory, or `--out`; `lint` scans the working
  directory minus `target/`, `node_modules/`, `out/`, `.git/`, `instance/`, `.superpowers/`
  and `.worktrees/`.
- `render` resolves protected paths under both the given home and the real home; on a
  case-insensitive volume a not yet existing case variant of a protected path is created
  before it is refused.
- `apply --live` exists.
