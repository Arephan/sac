# v1 acceptance, 2026-10-19

Run on the author's Mac (macOS, Claude Code 2.1.303). All writes went to a scratch folder; the
real `~/.claude` was only read.

1. **Test suite:** 75 tests in 10 files, all passing. Typecheck and build clean.
2. **Saving the real setup:** `sac save --dry-run` and then `sac save <scratch>/repo` captured
   872 files, 2 MCP servers, 3 marketplaces, 8 plugins and 2 launchd jobs. No secret placeholders
   were needed. secretlint over the 875 text files in the repo found nothing, and none of the
   known secret values (including the Slack bot token file) appear anywhere in it. Three
   third-party LaunchAgents that could not be parsed were skipped with a note.
3. **Restore into a separate Claude home:** `sac restore <scratch>/repo --config-dir
   <scratch>/home2 --yes` applied 887 changes with none skipped or failed, in 51 s (most of it
   marketplace clones and plugin installs). `CLAUDE_CONFIG_DIR=<scratch>/home2 claude plugin
   list` shows the same 8 user plugins in the same on/off state as the real home; `claude mcp
   list` shows both servers. `sac doctor --config-dir <scratch>/home2` passes (one note: an HTTP
   server needs authentication). Hashes of the real `settings.json`, `installed_plugins.json` and
   `known_marketplaces.json` were unchanged afterwards. A second restore printed "Everything
   already matches" in 1 s.
4. **Layering:** an `org` repo (CLAUDE.md, `rules/review.md`, deny `Bash(curl:*)`, MCP server
   `docs`) extended by a `me` repo (its own CLAUDE.md, deny `Bash(rm -rf:*)`, `model: opus`, a
   different `docs` server). Restored into a separate home: `CLAUDE.md` is `me`'s, `rules/` holds
   `review.md` and `sac-org.md`, both deny rules are present, `model` is `opus`, and `docs` is
   `me`'s version; the plan reported the clash.
5. **Jobs:** both jobs were captured as definitions with no secret values. A restore with `HOME`
   set to a scratch folder (job dependencies stubbed there) parked both plists in its
   `.sac/jobs/`, left `Library/LaunchAgents` alone, and printed the commands that install and
   load them. `launchctl` and
   `crontab` were replaced with a logging stub for the run, which recorded zero calls.

## Review

A fresh whole-branch review found nine problems; all are fixed with tests where a test can show
them: setup repos are read without following symlinks; unloaded launchd jobs wait in
`~/.sac/jobs/` instead of LaunchAgents (launchd loads that folder at login); settings diffs mask
every env, header and credential-named value, old and new; an MCP update backs up the old config
and puts it back if the new one fails; git layers are cached under a hashed name and cloned with
`--`; `--yes` never prompts; `settings.json` keeps its mode (new files with secrets are 0600);
`doctor` fails on jobs that are not loaded or not in the crontab; a `.env` inside a repo pulled
from a git URL is ignored. After the fixes, steps 3 and 5 above were run again with the same
results (37 s for the jobs run).
