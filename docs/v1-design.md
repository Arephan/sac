# setup-as-code v1 (`sac`): design

Date: 2026-10-16. Status: draft for review.

## 1. What it is

Your Claude setup as code. `sac save` captures a person's Claude Code setup into a git repo.
`sac restore` brings it back on any machine in minutes. Organizations put a team base under each
person's setup, so a new machine or a new hire gets a working Claude fast.

### Goals

- **Easy:** three commands (`save`, `restore`, `doctor`), defaults that need no config file, one
  prompt per missing secret, a plan shown before anything changes.
- **Native:** the repo mirrors Claude Code's own files. Plugins, marketplaces and MCP servers are
  installed through Claude Code's own CLI, never by editing its internal files.
- **Safe:** secret values are never saved; restore backs up what it overwrites, deletes nothing,
  and never touches credentials, sessions or history.
- **Fast:** restoring a typical setup takes one command and finishes in minutes; non-interactive
  restore works for provisioning many machines.
- **Organizations:** a setup can `extend` a team base; base rules and deny lists come first and a
  person cannot remove a base deny rule.

### Not in v1

- Memory notes (`~/.claude/projects/*/memory`), project-level `.claude` folders (they live in their
  own repos), Claude login and credentials, Windows, a hosted service, publishing to npm.
- Slack bots (designed earlier, parked), v0.3.0's 11-layer schema and policy engine.

### Decisions

- v1 replaces the v0.3.0 Rust tree. It is built on branch `v1`; `main` fast-forwards to it once
  acceptance passes, with no force push, and the `v0.3.0` tag stays.
- TypeScript on Node 22. Run with `npx setup-as-code <command>`, or `sac` once installed.
- Everything protocol-shaped comes from a library (§9).

## 2. What a setup is

| Item | Read from | Saved as | Restored by |
|---|---|---|---|
| Instructions | `~/.claude/CLAUDE.md` and every file it imports with `@path` that lives under `~/.claude` | `claude/CLAUDE.md`, imports at the same relative paths | file write |
| Rules | `~/.claude/rules/**/*.md` | `claude/rules/` | file write |
| Settings | `~/.claude/settings.json` | `claude/settings.json` | merge into the target's settings |
| Keybindings | `~/.claude/keybindings.json` | `claude/keybindings.json` | file write |
| Skills, agents, commands, output styles | `~/.claude/{skills,agents,commands,output-styles}/` | `claude/<same>/` | file write |
| Hook and statusline scripts | scripts named in `settings.json` (`hooks`, `statusLine`) that live under `~/.claude` | `claude/<same relative path>` | file write, executable bit kept |
| MCP servers (user scope) | `mcpServers` in `~/.claude.json`, read only | `mcp.json` | `claude mcp add-json --scope user` |
| Marketplaces and plugins | `claude plugin marketplace list` and `claude plugin list` (JSON output), falling back to reading `~/.claude/plugins/*.json` read only | `plugins.json` | `claude plugin marketplace add`, `claude plugin install`, `claude plugin enable/disable` |
| Plugin settings | `claude plugin configure <plugin>` output | `plugins.json` (`settings` per plugin) | `claude plugin configure <plugin> --values-stdin` |
| Background jobs | macOS: `~/Library/LaunchAgents/*.plist` whose program or arguments run `claude`; Linux: `crontab -l` lines that run `claude` | `jobs/<label>.yaml` | write the plist or crontab line; never loaded without `--load-jobs` |

Never read or saved: credentials (keychain or credential files), sessions, history, `projects/`,
caches, telemetry, shell snapshots, todos, `settings.local.json`, the plugin cache (plugins are
reinstalled), and everything else in `~/.claude.json`.

Files a job or hook script runs that live outside `~/.claude` are not copied. `save` lists them as
dependencies, and `doctor` checks they exist.

## 3. The repo

```
my-claude/
  sac.yaml          name, when and from which Claude version it was captured, optional extends
  claude/           mirror of the saved ~/.claude items
  mcp.json          { "mcpServers": { name: { …, "env": { "API_KEY": "{{secret:API_KEY}}" } } } }
  plugins.json      { "marketplaces": [{ name, source }], "plugins": [{ id, enabled, settings }] }
  jobs/<label>.yaml label, program, schedule, working directory, env (secrets as placeholders), log paths
  .env.example      one line per secret: name, where it is used, a blank value
  .gitignore        .env
```

```yaml
# sac.yaml
version: 1
name: my-claude
captured: { at: "2026-10-16T14:00:00Z", claude: "2.1.301", os: darwin }
extends: ["https://github.com/acme/claude-base.git"]   # optional; base layers first
```

Paths under the Claude folder and the home directory are saved as `{{claude}}` and `{{home}}`;
restore turns them into the new machine's paths. A `~` you wrote yourself is left alone. Secret
values are replaced with `{{secret:NAME}}`; a `${VAR}` you wrote yourself (shell or MCP
environment expansion) is never treated as one.

## 4. `sac save [dir]`

1. Read the items of §2 from the current machine (or from `--config-dir`).
2. Find secrets: `secretlint` (recommended preset) over every saved text, plus values under keys
   that name a credential (`env`, `headers`, and any key containing `token`, `secret`, `key`,
   `password` or `auth`). Each one becomes `{{secret:NAME}}` (the env var name where there is one,
   otherwise a name derived from where it was found) and a line in `.env.example`. In a text file
   only the detected token is replaced.
3. Replace the Claude folder and home paths with `{{claude}}` and `{{home}}`.
4. Write the repo (default `~/my-claude`), `git init` it if needed, and commit locally as
   `sac save <date>`. Never push.
5. Print a summary: what was saved, which secrets became placeholders, dependencies outside
   `~/.claude`, MCP servers that need a login after restore.

`--dry-run` prints the diff against the repo's current contents and writes nothing.

## 5. `sac restore <repo> [repo…]`

1. **Load layers:** each argument is a git URL (cloned into `~/.sac/cache/`) or a local folder;
   `extends` is followed depth first (base before the setup that names it, cycles refused,
   at most 5 levels). Later layers override earlier ones (§6).
2. **Plan:** compute what would change on the target and print it: file diffs, settings changes,
   marketplaces, plugins and MCP servers to add, plugin settings to apply, jobs to write. Nothing
   that already matches is touched.
3. **Secrets:** every `{{secret:NAME}}` the plan needs is taken from the environment, then from `--env-file`
   (default: `.env` beside the top layer), then from the machine itself when an item is already
   there, then asked for with a hidden prompt. With `--yes`,
   missing secrets skip the items that need them and the run exits 1 at the end (`--allow-missing`
   makes that exit 0).
4. **Apply**, after confirmation (or `--yes`): back up every file it will overwrite to
   `~/.sac/backups/<UTC stamp>/`; write files; merge `settings.json`; add marketplaces; install,
   enable or disable plugins; apply plugin settings; add MCP servers; write job files.
5. **Report** what changed, what was skipped and why, and the commands that load the jobs.

Flags: `--dry-run` (plan only), `--yes`, `--config-dir <dir>` (restore into a separate Claude home:
files go there and every `claude` CLI call gets `CLAUDE_CONFIG_DIR=<dir>`, for testing and for
fresh machines; jobs are then written under `<dir>/sac-jobs/`, never into the real LaunchAgents),
`--load-jobs` (load written jobs with `launchctl bootstrap` or `crontab`; refused with `--config-dir`),
`--env-file <file>`.

Restore never deletes an item that is on the machine but not in the repo; `doctor` lists those.

## 6. Layering

| Item | Rule |
|---|---|
| `CLAUDE.md` | the top layer's file becomes `~/.claude/CLAUDE.md`; each lower layer's file is written to `~/.claude/rules/sac-<layer name>.md`, which Claude Code loads natively |
| Rules, skills, agents, commands, output styles, hook scripts | combined by path; on a clash the later layer wins and restore warns |
| `settings.json` | deep merge: lists are combined without duplicates; `permissions.deny` from any layer always stays; other values: later layer wins; on the machine, values not in any layer are kept |
| MCP servers, marketplaces, plugins | combined by name; later layer wins on a clash |
| Plugin settings, jobs | combined by plugin and by label; later layer wins |

Hard enforcement (settings a person must not change) is out of scope for `sac`; organizations
use Claude Code's managed settings for that, and `sac` documents how.

## 7. `sac doctor [repo…]`

Compares the machine to the layers and reports: files that differ, plugins missing or in the wrong
enabled state, MCP servers missing or failing (`claude mcp list` status), secrets referenced but
not set, job files missing or not loaded, dependencies outside `~/.claude` missing, items on the
machine that no layer has, and a Claude Code version older than the one the setup was captured
with. Exit 1 when anything is missing or failing.

## 8. Safety rules

1. Secret values are never written into a setup repo; `save` runs `secretlint` over its own output
   and refuses to commit if anything is found.
2. `~/.claude.json` is only read; MCP servers change only through `claude mcp`.
3. Credentials, sessions, history and caches are never read or written.
4. Restore shows the plan first, backs up before overwriting, and deletes nothing.
5. Jobs are never loaded without `--load-jobs`.
6. `sac` never pushes; the user decides where the repo lives and keeps it private.
7. Tests never touch the real home: they use a scratch `HOME` and `--config-dir`.

## 9. Libraries

| Concern | Library |
|---|---|
| CLI | `commander` |
| Prompts (confirm, hidden secret input) | `@inquirer/prompts` |
| Running `claude`, `git`, `launchctl`, `crontab` | `execa` |
| Secret detection | `secretlint`, `@secretlint/secretlint-rule-preset-recommend` |
| Diffs in the plan | `diff` |
| YAML | `yaml` |
| launchd plists | `plist` |
| Settings merge | `deepmerge` |
| File lists | `fast-glob` |
| Tests | `vitest` |

`sac`'s own code: the capture list, placeholder rules, layer merge rules, plan and apply order,
`doctor`'s checks.

## 10. Testing

- Every test runs with a scratch `HOME` built from fixtures, and a fake `claude` executable on
  `PATH` that records each call and answers `plugin list`, `plugin marketplace list`, `mcp list`
  and `plugin configure` with fixture JSON.
- Round trip: save a fixture home, restore it into an empty home, `doctor` passes, saving the
  restored home gives a repo identical to the first one.
- Secrets: fixture secrets of every kind (MCP env and headers, settings env, plugin settings, job
  env) never appear in the saved repo, and restore puts them back from `.env`.
- Layering: an org base plus a personal repo restore to the merged result of §6.
- Jobs: a fixture launchd plist and crontab line round-trip; nothing is loaded without `--load-jobs`.

## 11. Acceptance

1. **Round trip** on the fixture home, as in §10, all green.
2. **The author's real setup, read only:** `sac save --dry-run` on the real home shows what would
   be saved and which secrets become placeholders; then `sac save /tmp/<scratch>` writes a repo,
   and `secretlint` and a search for the real secret values find nothing in it. The real home is
   not written.
3. **Restore into a separate Claude home:** `sac restore /tmp/<scratch> --config-dir /tmp/<home2>
   --yes --env-file <test values>`; then `CLAUDE_CONFIG_DIR=/tmp/<home2> claude plugin list` and
   `claude mcp list` show the plugins and servers, and `sac doctor` passes. Timed.
4. **Org layering:** a base repo with a rule file, a deny list and an MCP server, extended by a
   personal repo; restore into a separate home shows the base rules in `rules/`, both deny lists,
   and the personal values winning clashes.
5. **Jobs:** the author's claude jobs are captured as definitions with secrets as names; restore
   into a scratch home writes them and loads none.

## 12. Build order

1. Package, CLI skeleton, scratch-home helpers, fake `claude`.
2. Reading the file items (instructions and imports, rules, settings, keybindings, folders, scripts).
3. Secret placeholders and home-path rewriting.
4. MCP servers, marketplaces, plugins and plugin settings.
5. Background jobs (launchd and crontab).
6. `sac save`.
7. Layer loading and merging.
8. `sac restore`: plan, secrets, apply.
9. `sac doctor`.
10. Replace the v0.3.0 tree on `v1` (new README through the AI-smell gate), acceptance runs, report.
