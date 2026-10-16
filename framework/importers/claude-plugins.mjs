// Installed Claude Code plugins (installed_plugins.json). Enabled flags come from claude-settings.
export default {
  id: 'claude-plugins',
  layers: ['tools'],
  description: 'Installed plugins and marketplaces under ~/.claude/plugins.',
  detect: (ctx) => ctx.exists('~/.claude/plugins/installed_plugins.json'),
  run(ctx) {
    const j = ctx.tryJSON('~/.claude/plugins/installed_plugins.json') || {};
    const entries = j.plugins || {};
    const plugins = Object.entries(entries).map(([id, v]) => {
      const first = Array.isArray(v) ? v[0] : v;
      return { id, installed: true, ...(first?.version ? { version: String(first.version) } : {}) };
    });
    return { tools: { plugins }, _sources: ['~/.claude/plugins/installed_plugins.json'] };
  },
};
