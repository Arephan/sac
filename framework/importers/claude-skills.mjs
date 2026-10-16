// Skills (~/.claude/skills/<name>/SKILL.md) and slash commands (~/.claude/commands/*.md).
import YAML from 'yaml';

function frontmatter(text) {
  const m = text.match(/^---\n([\s\S]*?)\n---/);
  if (!m) return {};
  try { return YAML.parse(m[1]) || {}; } catch { return {}; }
}

export default {
  id: 'claude-skills',
  layers: ['tools'],
  description: 'User skills and slash commands.',
  detect: (ctx) => ctx.isDir('~/.claude/skills') || ctx.isDir('~/.claude/commands'),
  run(ctx) {
    const skills = [];
    for (const name of ctx.list('~/.claude/skills')) {
      const file = `~/.claude/skills/${name}/SKILL.md`;
      if (!ctx.exists(file)) continue;
      const fm = frontmatter(ctx.readText(file));
      skills.push({ id: name, source: ctx.tilde(ctx.p(file)), description: String(fm.description || '').replace(/\s+/g, ' ').trim().slice(0, 200) });
    }
    const commands = ctx.list('~/.claude/commands').filter((f) => f.endsWith('.md'))
      .map((f) => ({ id: f.replace(/\.md$/, ''), source: `~/.claude/commands/${f}` }));
    return { tools: { skills, commands }, _sources: ['~/.claude/skills', '~/.claude/commands'] };
  },
};
