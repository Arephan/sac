# Safety

## What the tool never does

- `import`, `plan`, `verify`, `apply` (without `--live`), `lint` and `redact` never write to live
  assistant config (`~/.claude`, `~/.claude.json`), `~/Library/LaunchAgents`,
  `~/Library/LaunchDaemons`, `/Library`, `/etc` or systemd user units. `apply` writes only below
  the out root and checks its resolved real path before every write; symlinks planted inside it
  are refused.
- Never loads, unloads, bootstraps, kicks or kills a job. The only commands the binary ever runs
  are `launchctl list`, `launchctl print-disabled`, `crontab -l`, `sw_vers` and `uname` (plus the
  plugins you install). Launchd plists and `.git/config` are parsed in-process. `apply --live`
  writes plist files and prints the `launchctl bootstrap` commands; it never runs them.
- Never opens a file whose name looks like a credential, and never keeps an env var or header
  value: those become `env_names` / `header_names`.
- Never calls a network API, a paid service, a chat platform or a mail server.

## `apply --live`

The one command that writes into a home. It needs `--yes`. Before the first write it copies every
file it is about to change to `~/.setup-as-code/backups/<UTC timestamp>/`. It merges, it does not
replace: `settings.json` keeps every key the manifest does not manage and every `env` value;
`~/.claude.json` keeps each existing server's `env` and `headers`; plists, `hooks` and
`deniedMcpServers` are only written when missing unless `--force`, because the manifest's model
of them is lossy and `--force` regenerates them. It refuses to touch a `settings.json` or `.claude.json` that is not valid
JSON, so a half-written file is never silently overwritten. Point it at a sandbox with `--home`
to see exactly what it would do.

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

1. Importers cannot read secret-named files and scrub secret-shaped strings (`Ctx::scrub`).
2. The `secrets_by_reference` policy fails `verify` if any value in the setup looks like a token,
   key, private key block, JWT, bearer header or URL password.
3. `sac lint` fails if a secret-shaped string appears in the shareable tree.

## The lint: keeping the framework person-free

`sac lint` scans the working directory (skipping `target/`, `node_modules/`, `out/`, `.git/`,
`instance/`, `.superpowers/` and `.worktrees/`), or the paths you give it, for:

- secret-shaped strings (same detectors as above);
- email addresses outside `example.com/org/net`;
- phone numbers;
- `/Users/<name>` or `/home/<name>` paths (other than placeholders such as `/Users/you` or
  `/Users/alex`);
- chat platform member/channel ids;
- **instance terms**: every `instance/<name>/lint-denylist.txt` (names, companies, handles,
  product names, family names) plus values pulled from each instance's `setup.yaml` and
  `overrides.yaml` (account principals and workspaces, channel handles, senders and ids, the home
  folder user name). The framework holds none of these terms itself; it reads them from the
  instances at lint time, so the check gets stronger as your instance grows.

An instance may list exact phrases in `lint-allow.txt` that are allowed despite containing a term
(for example a public handle in a clone URL). Allowed phrases are printed on every run.

Exit code 1 on any finding. Run it before sharing or committing; CI runs it on every push.

## Redacted examples

`sac redact -i <instance> --out <dir>` writes a shape-only copy: ids become placeholders
(`rule-001`, `loop-014`), every free-text field is dropped, and only enum-like fields survive
(kind, access, severity, scope, date, status, schedule, enabled, transport, event). It is how
`examples/large-redacted/` was made, and the lint runs over it.
