// Claude Code user settings: model/effort, permissions, denied MCP servers, hooks, enabled plugins.
// Reads ~/.claude/settings.json and ~/.claude/settings.local.json. Env var VALUES are dropped.
import { slug } from '../lib/util.mjs';

export default {
  id: 'claude-settings',
  layers: ['brain', 'tools'],
  description: 'Claude Code settings.json + settings.local.json (permissions, hooks, plugins, model).',
  detect: (ctx) => ctx.exists('~/.claude/settings.json'),
  run(ctx) {
    const s = ctx.tryJSON('~/.claude/settings.json') || {};
    const local = ctx.tryJSON('~/.claude/settings.local.json') || {};
    const perms = s.permissions || {};
    const localPerms = local.permissions || {};
    const uniq = (...ls) => [...new Set(ls.flat().filter(Boolean))];

    const hooks = [];
    for (const [event, groups] of Object.entries(s.hooks || {})) {
      for (const g of groups || []) {
        for (const h of g.hooks || []) {
          const command = ctx.scrub(String(h.command ?? h.url ?? h.type));
          hooks.push({ id: `${slug(event)}-${slug(g.matcher || 'any', 24)}-${slug(command.split('/').pop(), 30)}`, event, matcher: g.matcher || '*', command });
        }
      }
    }

    const plugins = Object.entries(s.enabledPlugins || {}).map(([id, enabled]) => ({ id, enabled: !!enabled }));

    return {
      brain: {
        claude_code: ctx.scrub({
          model: s.model,
          effort: s.effortLevel,
          per_model: s.modelSettings,
          permission_mode: perms.defaultMode,
          env_names: Object.keys(s.env || {}),
          marketplaces: Object.keys(s.extraKnownMarketplaces || {}),
        }),
      },
      tools: {
        permissions: {
          ...(perms.defaultMode ? { default_mode: perms.defaultMode } : {}),
          allow: uniq(perms.allow || [], localPerms.allow || []).map((x) => ctx.scrub(x)),
          deny: uniq(perms.deny || [], localPerms.deny || []),
          ask: uniq(perms.ask || [], localPerms.ask || []),
        },
        denied_mcp_servers: (s.deniedMcpServers || []).map((d) => d.serverName || d.serverUrl || String(d)),
        hooks,
        plugins,
      },
      _sources: ['~/.claude/settings.json', '~/.claude/settings.local.json'],
    };
  },
};
