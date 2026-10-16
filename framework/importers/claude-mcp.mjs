// MCP servers from every place Claude Code reads them: ~/.claude.json (user + per-project),
// ~/.claude/mcp.json. Only command/url/arg shape and the NAMES of env vars and headers are kept.
export default {
  id: 'claude-mcp',
  layers: ['tools'],
  description: 'MCP server definitions (user, per-project, mcp.json). Env/header values never kept.',
  detect: (ctx) => ctx.exists('~/.claude.json') || ctx.exists('~/.claude/mcp.json'),
  run(ctx) {
    const servers = [];
    const add = (name, def, source) => {
      const transport = def.type === 'http' || def.type === 'sse' ? def.type : def.url ? 'http' : def.command ? 'stdio' : 'unknown';
      servers.push(ctx.scrub({
        id: servers.some((s) => s.id === name) ? `${name}@${source}` : name,
        source,
        transport,
        ...(def.command ? { command: def.command } : {}),
        ...(def.args ? { args: def.args.map(String) } : {}),
        ...(def.url ? { url: def.url.replace(/\?.*$/, '') } : {}),
        env_names: Object.keys(def.env || {}).sort(),
        header_names: Object.keys(def.headers || {}).sort(),
        enabled: def.disabled ? false : true,
      }));
    };
    const cj = ctx.tryJSON('~/.claude.json');
    if (cj) {
      for (const [n, d] of Object.entries(cj.mcpServers || {})) add(n, d, '~/.claude.json');
      for (const [proj, p] of Object.entries(cj.projects || {})) {
        for (const [n, d] of Object.entries(p.mcpServers || {})) add(n, d, `~/.claude.json#projects/${ctx.tilde(proj)}`);
      }
    }
    const mj = ctx.tryJSON('~/.claude/mcp.json');
    if (mj) for (const [n, d] of Object.entries(mj.mcpServers || mj)) if (d && typeof d === 'object') add(n, d, '~/.claude/mcp.json');
    return { tools: { mcp_servers: servers }, _sources: ['~/.claude.json', '~/.claude/mcp.json'] };
  },
};
