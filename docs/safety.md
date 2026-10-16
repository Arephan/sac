# Safety

## What the tool never does

- Never writes to live assistant config (`~/.claude`, `~/.claude.json`), `~/Library/LaunchAgents`,
  `~/Library/LaunchDaemons`, `/Library`, `/etc` or systemd user units. `apply` writes only below
  `out/` and checks the resolved real path of `out/` before every write; symlinks planted inside
  `out/` are refused.
- Never loads, unloads, bootstraps, kicks or kills a job. The launchd importer runs only
  `plutil -convert json -o -`, `launchctl list` and `launchctl print-disabled`.
- Never opens a file whose name looks like a credential, and never keeps an env var or header
  value: those become `env_names` / `header_names`.
- Never calls a network API, a paid service, a chat platform or a mail server.

## Secrets: names and scopes only

Write where a credential lives, never what it is:

```yaml
credential_ref: ~/.bot_token          # a file path
credential_ref: env:MAIL_APP_PASSWORD # an env var name
credential_ref: keychain:assistant-bot
env_names: [API_KEY]
scopes: [chat:write, files:read]
```

Three layers of defence:

1. Importers cannot read secret-named files and scrub secret-shaped strings (`ctx.scrub`).
2. The `secrets_by_reference` policy fails `verify` if any value in the setup looks like a token,
   key, private key block, JWT, bearer header or URL password.
3. `node setup.mjs lint` fails if a secret-shaped string appears in the shareable framework.

## The lint: keeping the framework person-free

`node setup.mjs lint` scans `framework/`, `examples/`, `docs/`, `README.md`, `setup.mjs`,
`package.json` and `.gitignore` for:

- secret-shaped strings (same detectors as above);
- email addresses outside `example.com/org/net`;
- phone numbers;
- `/Users/<name>` or `/home/<name>` paths (other than placeholders such as `/Users/you`);
- chat platform member/channel ids;
- **instance terms**: every `instance/<name>/lint-denylist.txt` (names, companies, handles,
  product names, family names) plus values pulled from each instance's `setup.yaml` and
  `overrides.yaml` (account principals and workspaces, channel handles, senders and ids, the home
  folder user name). The framework holds none of these terms itself; it reads them from the
  instances at lint time, so the check gets stronger as your instance grows.

An instance may list exact phrases in `lint-allow.txt` that are allowed despite containing a term
(for example a folder name chosen on purpose). Allowed phrases are printed on every run.

Exit code 1 on any finding. Run it before sharing or committing the framework.

## Redacted examples

`node setup.mjs redact -i <instance> --out <dir>` writes a shape-only copy: ids become
placeholders (`rule-001`, `loop-014`), every free-text field is dropped, and only enum-like
fields survive (kind, access, severity, scope, date, status, schedule, enabled, transport, event).
It is how `examples/large-redacted/` was made, and the lint runs over it.
