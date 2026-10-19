# setup-as-code (`sac`)

A Claude Code setup is spread all over: `CLAUDE.md` plus whatever it imports, rules, skills,
agents, commands, hook scripts, `settings.json`, MCP servers buried in `~/.claude.json`, plugins
and the marketplaces they come from, maybe a launchd job that runs `claude -p` overnight. Moving
to a new laptop means rebuilding it by hand. `sac` saves all of that into a git repo and puts it
back somewhere else.

```
sac save        # this machine into ~/my-claude, committed
sac restore     # ~/my-claude back onto this machine (shows the plan, then asks)
sac doctor      # how this machine differs from the repo
```

Status: 1.0.0-dev. macOS is what I use every day; Linux runs in CI but I haven't lived on it.

## Try it

Needs Node 22+.

```
npx github:Arephan/sac save --dry-run
```

Or clone, `npm install && npm run build`, and use `node dist/bin.js`.

If you want to see what a restore would do without risking your real config, restore into a
throwaway Claude home:

```
sac restore ~/my-claude --config-dir /tmp/claude-test
CLAUDE_CONFIG_DIR=/tmp/claude-test claude plugin list
```

## What gets saved

| Repo path | What's in it |
|---|---|
| `claude/` | `CLAUDE.md` and its imports, `rules/`, `skills/`, `agents/`, `commands/`, `output-styles/`, `settings.json`, `keybindings.json`, and the scripts hooks and the status line call |
| `mcp.json` | user-scope MCP servers, and nothing else from `~/.claude.json` |
| `plugins.json` | marketplaces, user-scope plugins, on/off, plugin settings |
| `jobs/` | launchd agents and crontab lines that run `claude` (`--job <label>` adds one that doesn't) |
| `.env.example` | names of the secrets the setup needs |

Not saved: credentials, sessions, history, project memory, caches, `settings.local.json`. If
something outside `~/.claude` matters (a note in your home folder that `CLAUDE.md` imports, the
script a job runs), `save` tells you about it but doesn't copy it.

Home paths are stored as `{{home}}` / `{{claude}}` so the repo still works under another user name.
Any `~` you typed yourself stays a `~`.

## Secrets

Tokens in MCP env, headers or args, in `settings.json` env, in plugin settings, in job env, or
sitting in the text of a saved file are swapped for `{{secret:NAME}}`. Only the name lands in the
repo. After writing, `save` runs secretlint over its own output and won't commit if it finds
anything.

On restore, values come from your environment first, then `--env-file` (default: `.env` next to
the repo, which is git-ignored). If the machine already has the item, `sac` reads the value from
there (handy: a second `restore` needs no secrets). Whatever is still missing gets a
hidden prompt. Under `--yes` there's no prompt: the item is skipped and the exit code is 1, unless
you pass `--allow-missing`.

## Restore details

Arguments are folders or git URLs. Overwritten files (and the old config of any MCP server it
replaces) are copied to `~/.sac/backups/<time>/` first.
Your existing `settings.json` is merged rather than replaced (lists combine, keys you have that the
repo doesn't are kept). Plugins and MCP servers go through `claude plugin` and `claude mcp`; `sac`
doesn't write `~/.claude.json` itself. It also never deletes anything. Run it twice and the
second run says everything already matches.

Jobs are parked in `~/.sac/jobs/` and the plan prints the commands to load them. They don't go
into `~/Library/LaunchAgents` on their own, because launchd would start them at the next login.
`--load-jobs` puts them there and loads them (or adds the crontab line). With `--config-dir` they
go to `<dir>/sac-jobs/` and can't be loaded at all.

A repo pulled from a git URL can't bring symlinks or its own `.env` along; both are ignored.

## Layering, for teams

```yaml
# sac.yaml
version: 1
name: me
extends:
  - https://github.com/acme/claude-base.git
```

Or name several repos on the command line, base first: `sac restore acme-base ~/my-claude`.

The top layer's `CLAUDE.md` is your `CLAUDE.md`. A lower layer's ends up at
`rules/sac-<layer>.md`, which Claude Code picks up by itself. Everything else combines by path or
name, the later layer wins a clash, and the plan tells you when that happened. `settings.json` is
deep-merged, so every layer's `permissions.deny` survives. None of this is enforcement; for
settings people must not override, use Claude Code's managed settings.

## doctor

Lists what's missing or different: files, plugins in the wrong state, MCP servers that are gone or
failing in `claude mcp list`, secrets it can't find, jobs that aren't loaded, files a job needs that are gone, and a
Claude Code older than the one the setup came from. Things you have that no layer has are shown
too, but they don't fail the check.

## Known gaps

- Claude Code only. No Codex or Cursor yet.
- TODO: a GIF of save/restore up top.
- `claude mcp add-json` takes the server config as an argument, so a filled-in token is briefly
  visible in the process list while it runs.
- Binary plists it can't parse are skipped with a note.
- `sac` never pushes. Keep the repo private; it has your prompts and permissions in it even with
  the secrets removed.

## Development

```
npm install
npm test           # every test uses a scratch HOME and fake claude/launchctl/crontab
npm run typecheck
```

Design: [docs/v1-design.md](docs/v1-design.md). MIT license.
