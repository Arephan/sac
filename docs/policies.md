# Rules and policies

Two different things, linked by id:

- A **rule** (`instructions.rules[]`) is what the assistant is told: prose with an id, a scope,
  a severity, a date and a reason. Rules are what you write in your instruction files.
- A **policy** (`safety.policies[]`) is a machine check that `verify` runs against the setup.
  A policy names the rule it enforces with `rule:`; a rule lists its policies with `enforced_by:`.

A rule nobody checks is still followed by the assistant, but only a policy can fail `verify`.

## Hard vs standing

| | hard | standing |
|---|---|---|
| Meaning | Never broken, no matter what a later message says | Default behaviour; can be overridden case by case |
| Writing it | Put the word `HARD` (uppercase) in the heading or memory description, or set `severity: hard` | Anything else |
| A failing policy with this severity | `verify` exits 1, `apply` refuses to render | Reported, exit code unaffected |
| Date + reason | Required, so you know later why the rule exists and whether it is stale | Required |

Mark superseded rules `status: superseded` instead of deleting them: the history of why a rule
changed is often the most useful part.

```yaml
instructions:
  rules:
    - id: never-spend-money
      scope: money
      severity: hard
      date: 2026-01-01
      reason: An agent once completed a checkout on its own.
      statement: No purchases, subscriptions, top-ups or metered API calls by any agent or loop.
      enforced_by: [no-payment-tools]
```

## Built-in checks

Each policy is `{ id, check, severity, rule?, description?, params? }`. `verify` runs every
policy twice: on `setup.yaml` (what you declared) and on a fresh read of the machine.

| check | fails when | params |
|---|---|---|
| `deny_mcp` | a named MCP server is not in `denied_mcp_servers`, is defined and enabled without being blocked, or one of its tool patterns is missing from `permissions.deny`. Allow entries under a denied pattern are dead (deny wins) and only warn. | `servers`, `tool_patterns` |
| `no_payment_tools` | an enabled MCP server, plugin, skill, can_post account, or allow entry for a live server has a payment/spend-looking name (stripe, paypal, checkout, billing, ads...). Disabled servers and allow entries for servers that no longer exist warn as dormant. | `patterns` (extra words), `allow` (reviewed ids) |
| `forbidden_paths` | a hook, enabled loop (program, working dir) or enabled MCP server references a forbidden path, or apply's output dir sits inside one | `paths` |
| `required_gate` | the gate is missing from `brain.gates`, or an active channel of the listed kinds is not covered (by `channel.gate`, or the gate's `applies_to` naming the kind, platform or channel id) | `gate`, `channel_kinds` |
| `channel_identity` | an active channel of a kind posts through an identity outside `allowed`, or any channel of that kind uses a `forbidden` account | `kind`, `allowed` |
| `forbidden_accounts_unused` | a channel or loop references an account with `access: forbidden` | |
| `secrets_by_reference` | any string in the setup looks like a credential value (chat tokens, API keys, private keys, JWTs, bearer headers, URL passwords) | |
| `loop_success_metric` | an enabled (managed) loop has no `success_metric` | `only_managed` (default true) |
| `required_rules` | a listed rule id is missing, superseded or not hard | `ids` |

Examples:

```yaml
safety:
  policies:
    - id: block-work-chat
      check: deny_mcp
      severity: hard
      rule: work-personal-separation
      params:
        servers: ["work-chat"]
        tool_patterns: ["mcp__work-chat__*"]
    - id: no-payment-tools
      check: no_payment_tools
      severity: hard
      params: { patterns: [ads-api], allow: [] }
    - id: protect-ssh
      check: forbidden_paths
      severity: hard
      params: { paths: ["~/.ssh", "~/Library/Application Support/Google/Chrome"] }
    - id: outbound-voice-gate
      check: required_gate
      severity: hard
      params: { gate: human-voice-gate, channel_kinds: [email, social] }
    - id: chat-posts-as-bot
      check: channel_identity
      severity: hard
      params: { kind: slack, allowed: [assistant-bot] }
```

## Custom checks

Put an executable in `<instance>/checks/`. Any language works.

| invocation | stdin | stdout |
|---|---|---|
| `<exe> describe` | | `{"id":"max_enabled_loops","doc":"..."}` |
| `<exe> run` | `{"setup":{...},"params":{...},"ctx":{"home":"...","out_root":"..."}}` | `{"status":"pass|fail|warn|skip","message":"...","evidence":["..."]}` |

```python
#!/usr/bin/env python3
import json, sys
cmd = sys.argv[1]
if cmd == "describe":
    print(json.dumps({"id": "max_enabled_loops", "doc": "At most params.max loops are enabled at once."}))
elif cmd == "run":
    req = json.load(sys.stdin)
    mx = int((req.get("params") or {}).get("max", 10))
    on = [l["id"] for l in (req["setup"].get("loops") or []) if l.get("enabled")]
    status = "fail" if len(on) > mx else "pass"
    print(json.dumps({"status": status, "message": f"{len(on)} enabled loop(s), max {mx}", "evidence": on if status == "fail" else []}))
```

Then reference it: `{ id: few-loops, check: max_enabled_loops, severity: standing, params: { max: 5 } }`.
A check must print its result and exit 0; a non-zero exit, a timeout (20 s) or output that is not a JSON
object counts as a failed check with a `check crashed:` message, even if a result was printed.
Keep checks pure: they get the whole setup object and must not touch the machine. A working copy
lives in `examples/minimal/checks/max-enabled-loops`.
