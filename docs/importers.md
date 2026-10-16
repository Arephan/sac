# Importers

An importer discovers one part of a setup from the machine and returns a fragment of
`setup.yaml`. `import`, `plan`, `verify` and `apply` run all of them (or the list under
`import.importers`), deep-merge the fragments, then merge `overrides.yaml` on top.

## Built-in importers

| id | layers | reads |
|---|---|---|
| `claude-settings` | brain, tools | `~/.claude/settings.json`, `settings.local.json`: model, effort, permissions, denied MCP servers, hooks, enabled plugins, env var names |
| `claude-mcp` | tools | MCP servers in `~/.claude.json` (user and per-project) and `~/.claude/mcp.json`; env/header names only |
| `claude-plugins` | tools | `~/.claude/plugins/installed_plugins.json` |
| `claude-skills` | tools | `~/.claude/skills/*/SKILL.md` frontmatter, `~/.claude/commands/*.md` |
| `claude-instructions` | instructions | each `## ` section of `~/.claude/CLAUDE.md` becomes a rule; `@imports` become sources |
| `claude-memory` | instructions, projects, data, identity | auto-memory files: `feedback` = rules, `project` = projects, `reference` = references, `user` = profile refs (path only) |
| `launchd` | loops | `~/Library/LaunchAgents` (+ `disabled/`, parked `*.plist.*` copies) parsed in-process, `launchctl list`, `launchctl print-disabled` |
| `cron` | loops | `crontab -l` |
| `infra` | infra | OS version, which CLIs exist on `PATH`, git repos under the configured roots (origin URL from `.git/config`) |
| `file-state` | data | existence (only) of each `data.state[].path` you declared |

Options go under `import.options.<importer id>` (in `setup.yaml` or `overrides.yaml`):

```yaml
import:
  importers: all
  options:
    launchd: { managed_prefixes: [com.example.] }
    infra:   { clis: [my-cli], repo_roots: ["~", "~/code"] }
    claude-instructions: { scope_keywords: { health: [doctor, clinic] } }
    claude-memory: { dir: ~/somewhere/else/memory }
```

## Observed vs declared

Some things no importer can see (a quality gate, a business lane, why a rule exists). For every
collection that no importer returned, `import` keeps what `setup.yaml` already had and `plan`
treats it as matching, so hand-declared facts never show up as drift and are never wiped.
Facts you want merged onto discovered items (an owner and success metric for a launchd job)
belong in `overrides.yaml`; a map keyed by id merges onto the list item with that id:

```yaml
loops:
  com.example.morning-brief: { owner: personal, success_metric: brief by 08:15 }
```

## Writing a custom importer

Put an executable in `<instance>/importers/`. Any language works. It is loaded automatically; an
id equal to a built-in replaces the built-in. Add the id to `import.importers` if you list
importers explicitly.

The protocol has three subcommands:

| invocation | stdin | stdout | meaning |
|---|---|---|---|
| `<exe> describe` | | `{"id":"todo-md","layers":["channels"],"description":"..."}` | identity, read once at load |
| `<exe> detect` | | | exit 0 = applies to this machine, anything else = skipped |
| `<exe> run` | `import.options["todo-md"]` as JSON | a fragment of `setup.yaml` as JSON | the import |

Environment for `detect` and `run`: `SAC_HOME` (the home being read; a sandbox under `--home`)
and `SAC_REAL_HOME` (`1` when it is the real home, `0` otherwise). Machine-wide probes should
run only when `SAC_REAL_HOME` is `1`.

```python
#!/usr/bin/env python3
import json, os, re, sys
HOME = os.environ.get("SAC_HOME", os.path.expanduser("~"))
PATH = os.path.join(HOME, "assistant", "todo.md")
cmd = sys.argv[1]
if cmd == "describe":
    print(json.dumps({"id": "todo-md", "layers": ["channels"], "description": "Markdown todo file as channels.todo"}))
elif cmd == "detect":
    sys.exit(0 if os.path.exists(PATH) else 1)
elif cmd == "run":
    text = open(PATH).read()
    open_n = len(re.findall(r"^\s*- \[ \]", text, re.M))
    print(json.dumps({"channels": {"todo": [{"id": "todo", "path": "~/assistant/todo.md", "open": open_n}]}}))
```

A working copy lives in `examples/minimal/importers/todo-md`. Check it is picked up with
`sac importers -i <instance>`.

Rules for importers:

1. Read only. Never write, load, unload, start, stop or send.
2. Never return a secret value. Return where it lives, or its name.
3. Return `[]` for a collection you looked at and found empty (that marks it observed), and
   leave it out entirely if you did not look.
4. Give items stable ids so `plan` can diff them across runs.
5. Keep personal specifics out of the importer: take them from the options on stdin.
6. Use `~/...` paths in the fragment, never the absolute home.
7. Finish within 20 seconds. The timeout kills the plugin process only, so a wrapper script
   must `exec` its worker and must not leave background children holding stdout.
