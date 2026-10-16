# Layers

`setup.yaml` has eleven layers. Each is a top-level key. Lists of items are keyed by `id`;
`plan` diffs them item by item. The schema is `schema/setup.schema.json`; layer
definitions (which collections exist, which get diffed) are in `src/layers.rs`.

Conventions for every layer:

- **ids** are stable, unique within their list, and name a thing by role (`assistant-bot`,
  `outbound-mail`), never by a person.
- **paths** are `~/...` or `${env:HOME}/...`. `${env:NAME}` and `${vars.NAME}` are expanded at load,
  except inside `credential_ref` and other secret-named keys, which stay as written. Expanded values
  appear in `out/<instance>/setup.resolved.yaml`, so keep secrets out of `${env:}` entirely.
- **secrets** appear only as a location: `credential_ref: ~/.bot_token`, `env:API_KEY`,
  `keychain:item-name`, or `env_names: [API_KEY]`. Never a value (see docs/safety.md).
- Fields not listed below are allowed on most items (notes, extra metadata); `validate` keeps
  the required ones honest.

## identity

Who the assistant acts for and every account or credential it can use.

```yaml
identity:
  person: { role: owner, timezone: UTC, profile_refs: [~/notes/about-me.md] }
  accounts:
    - id: assistant-bot          # required
      kind: chat_bot             # required: chat_bot, mailbox, oauth_account, api_token, browser_profile, ...
      access: can_post           # required: can_post | read_only | forbidden | unknown
      principal: me@example.com  # which identity on the service (optional)
      workspace: personal-workspace
      credential_ref: ~/.assistant_bot_token
      scopes: [chat:write]
```

`access: forbidden` marks an account automation must never use; the `forbidden_accounts_unused`
and `channel_identity` checks enforce it.

## brain

Models, runtimes, the roster of other AI workers, quality gates and budgets.

```yaml
brain:
  claude_code: { model: opus, effort: high, permission_mode: default }   # imported from settings
  runtimes: [{ id: headless-claude, command: claude -p }]
  roster:   [{ id: second-opinion, runtime: other-cli exec, beat: bulk checks, billing: subscription }]
  gates:
    - id: human-voice-gate        # required
      applies_to: [email, social] # required: channel kinds, platforms or channel ids it covers
      model: opus
      threshold: "< 3/10"
  budgets: { money: 0 }
```

## instructions

Rules the assistant follows, and the files they come from.

```yaml
instructions:
  sources: [{ id: global, path: ~/.claude/CLAUDE.md, kind: global }]
  rules:
    - id: never-spend-money   # required
      scope: money            # required: free text; importers classify into money, email, chat, social, apps, ...
      severity: hard          # required: hard | standing
      date: 2026-01-01        # required: YYYY-MM-DD or null
      reason: why it exists   # required
      statement: what to do
      status: active          # active | superseded
      enforced_by: [no-payment-tools]   # policy ids that check it
```

See docs/policies.md for what hard vs standing means.

## tools

```yaml
tools:
  mcp_servers: [{ id: notes, transport: http, url: https://mcp.example.com, header_names: [Authorization], enabled: true, source: ~/.claude.json }]
  skills:   [{ id: morning-brief, description: ... }]
  plugins:  [{ id: name@marketplace, enabled: true, installed: true }]
  hooks:    [{ id: pre-bash, event: PreToolUse, matcher: Bash, command: ~/.claude/hooks/guard.sh }]
  commands: [{ id: report, source: ~/.claude/commands/report.md }]
  permissions: { default_mode: default, allow: [...], deny: [...], ask: [...] }
  denied_mcp_servers: [work-chat]
```

## loops

Every scheduled or background job, including disabled, parked and removed ones.

```yaml
loops:
  - id: com.example.morning-brief   # required (launchd label, cron id, ...)
    runner: launchd                 # required: launchd | cron | systemd | manual | cloud
    enabled: true                   # required
    status: loaded                  # imported: running, loaded, installed:not-loaded, parked, backup-only, removed:override-disabled ...
    managed: true                   # false = vendor job (not matched by managed_prefixes)
    schedule: { calendar: ["hour=8,minute=0"], every_seconds: 3600, keep_alive: true, run_at_load: false }
    program: [/bin/bash, ~/loops/brief.sh]
    working_dir: ~/loops
    log: ~/loops/brief.log
    env_names: [PATH]
    owner: personal
    success_metric: brief posted before 08:15
```

## channels

Where the assistant talks and writes. Six kinds, each a list: `slack` (any chat), `email`,
`social`, `calendar`, `todo`, `journal`.

```yaml
channels:
  slack:  [{ id: home-channel, role: every reply goes here, identity: assistant-bot }]
  email:  [{ id: outbound-mail, identity: personal-mail, gate: human-voice-gate }]
  social: [{ id: blog-x, platform: x, identity: browser-profile-1, status: halted }]
```

A channel whose `status`/`role` says halted, dead, killed, blocked, frozen, paused, removed or
forbidden (or `active: false`) is inactive: gates are not required on it.

## projects

```yaml
projects: [{ id: side-project, description: ..., status: active, path: ~/code/side-project, lane: newsletter }]
```

## business

```yaml
business:
  lanes:
    - id: newsletter
      status: active          # required: active | paused | halted | killed | blocked | planned
      goal: grow paid subscribers
      metric: paid subs per week
      loops: [com.example.morning-brief]   # validated against loops
      channels: [outbound-mail]
```

## data

State files by path; contents are never copied in.

```yaml
data:
  memory: { dir: ~/.claude/projects/<slug>/memory, files: 120, by_type: { feedback: 60, project: 40 } }
  state: [{ id: journal, path: ~/assistant/journal.jsonl, kind: journal, exists: true }]
  references: [{ id: api-gotchas, description: ..., source: ... }]
```

## infra

```yaml
infra:
  machine: { platform: darwin, arch: arm64, os: "15.0" }
  clis:     [{ id: gh, present: true, path: /opt/homebrew/bin/gh }]
  repos:    [{ id: side-project, path: ~/side-project, remote: https://github.com/you/side-project.git }]
  services: [{ id: hosting, tier: free only, account: hosting-token }]
```

## safety

Machine-checkable policies. They can also live in `policies.yaml` next to `setup.yaml`.

```yaml
safety:
  policies:
    - { id: no-payment-tools, check: no_payment_tools, severity: hard, rule: never-spend-money }
```

See docs/policies.md.
