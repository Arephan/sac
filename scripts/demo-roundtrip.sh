#!/usr/bin/env bash
# Demo: capture a setup from one home, rebuild it in an empty one, prove they match.
# Everything happens under a temp directory; your real home is never touched.
set -euo pipefail
cd "$(dirname "$0")/.."
SAC="${SAC:-./target/release/sac}"
[ -x "$SAC" ] || SAC="./target/debug/sac"
[ -x "$SAC" ] || { echo "build first: cargo build --release" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
A="$work/home-a"; B="$work/home-b"; INST="$work/instance/demo"
mkdir -p "$A/.claude/skills/brief" "$A/Library/LaunchAgents" "$B"

cat > "$A/.claude/settings.json" <<'EOF'
{ "model": "opus", "permissions": { "allow": ["Read", "Bash(git status*)"], "deny": ["Bash(git push --force*)"], "ask": [] },
  "hooks": { "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "~/.claude/hooks/guard.sh" }] }] } }
EOF
cat > "$A/.claude.json" <<'EOF'
{ "mcpServers": { "notes": { "type": "http", "url": "https://mcp.example.com/notes", "headers": { "Authorization": "Bearer not-a-real-token-abcdefghijklmnop" } } } }
EOF
cat > "$A/Library/LaunchAgents/com.example.brief.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>com.example.brief</string>
<key>ProgramArguments</key><array><string>/bin/bash</string><string>$A/brief.sh</string></array>
<key>StartCalendarInterval</key><array><dict><key>Hour</key><integer>8</integer><key>Minute</key><integer>0</integer></dict></array>
<key>RunAtLoad</key><false/>
</dict></plist>
EOF
printf -- '---\ndescription: morning brief\n---\n' > "$A/.claude/skills/brief/SKILL.md"

echo "== 1. init an instance"
"$SAC" init "$INST" --name demo
printf 'import:\n  options:\n    launchd: { managed_prefixes: [com.example.] }\nloops:\n  com.example.brief: { owner: personal, success_metric: brief by 08:15 }\n' > "$INST/overrides.yaml"

echo; echo "== 2. import home A (read-only)"
time "$SAC" import -i "$INST" --home "$A"

echo; echo "== 3. plan against A: nothing should differ"
"$SAC" plan -i "$INST" --home "$A"

echo; echo "== 4. rebuild into empty home B"
"$SAC" apply -i "$INST" --home "$B" --out "$work/out" --live --yes

echo; echo "== 5. plan against B: settings, MCP servers and launchd jobs are back"
"$SAC" plan -i "$INST" --home "$B"
echo; echo "(skills and memory are files the manifest only points at; they are not recreated.)"
