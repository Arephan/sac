# Importers

An importer discovers one part of a setup from the machine and returns a fragment of
`setup.yaml`. `import`, `plan` and `verify` run all of them (or the list under
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
| `launchd` | loops | `~/Library/LaunchAgents` (+ `disabled/`, parked `*.plist.*` copies), `launchctl list`, `launchctl print-disabled` |
| `cron` | loops | `crontab -l` |
| `infra` | infra | OS version, which CLIs exist, git repos under `~` |
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

Put a `.mjs` file in `<instance>/importers/`. It is loaded automatically; an id equal to a
built-in replaces the built-in. Add the id to `import.importers` if you list importers explicitly.

```js
// <instance>/importers/todo-md.mjs
export default {
  id: 'todo-md',                       // kebab-case, unique
  layers: ['channels'],                // which layers the fragment touches
  description: 'Markdown todo file as channels.todo with counts.',
  detect: (ctx) => ctx.exists('~/assistant/todo.md'),   // optional; false = skipped
  run(ctx, options) {                  // options = import.options['todo-md']
    const text = ctx.readText('~/assistant/todo.md');
    const open = (text.match(/^\s*- \[ \]/gm) || []).length;
    return { channels: { todo: [{ id: 'todo', path: '~/assistant/todo.md', open }] } };
  },
};
```

A working copy lives in `examples/minimal/importers/todo-md.mjs`. Check it is picked up with
`node setup.mjs importers -i <instance>`.

### The `ctx` object

Importers must use `ctx` for all machine access. It is what keeps import read-only.

| member | does |
|---|---|
| `ctx.home`, `ctx.isRealHome` | the home being read (`--home` overrides); machine-wide probes should run only when `isRealHome` |
| `ctx.p(path)`, `ctx.tilde(abs)` | expand `~`; turn an absolute path back into `~/...` |
| `ctx.exists`, `ctx.isDir`, `ctx.list`, `ctx.stat` | filesystem metadata |
| `ctx.readText`, `ctx.readJSON`, `ctx.tryJSON` | read a file; **throws** on secret-looking names (`*token*`, `*_key`, `.env`, `auth.json`, keys, cookies) |
| `ctx.exec(cmd, args)`, `ctx.tryExec` | run an allowlisted read-only command; anything else throws |
| `ctx.scrub(value)` | replace `~`, turn `env`/`headers`/`token`/... keys into `<key>_names: [...]`, drop secret-shaped strings |
| `ctx.warn(msg)` | record a warning shown by `import -v` |

Rules for importers:

1. Read only. Never write, load, unload, start, stop or send.
2. Never return a secret value. Return where it lives, or its name.
3. Return `[]` for a collection you looked at and found empty (that marks it observed), and
   leave it out entirely if you did not look.
4. Give items stable ids so `plan` can diff them across runs.
5. Keep personal specifics out of the importer: take them from `options`.
