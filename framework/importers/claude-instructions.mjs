// Global instruction files (~/.claude/CLAUDE.md and its @imports). Each "## " section of the
// main file becomes a rule; @imported files are recorded as sources.
import { slug } from '../lib/util.mjs';
import { classifyScope, severityOf, firstDate, oneLine } from '../lib/rules.mjs';

export function parseInstructionSections(text, { source, scopeKeywords, fallbackDate }) {
  const rules = [];
  const parts = text.split(/^## /m).slice(1);
  for (const part of parts) {
    const [headingLine, ...bodyLines] = part.split('\n');
    const heading = headingLine.trim();
    const body = bodyLines.join('\n').trim();
    const title = heading.split(/\s+[—–-]\s+|\s*\(/)[0];
    const quoted = heading.match(/["“]([^"”]+)["”]/);
    rules.push({
      id: `md-${slug(title, 48)}`,
      scope: classifyScope(`${heading} ${heading} ${heading} ${body.slice(0, 600)}`, scopeKeywords),
      severity: severityOf(heading),
      date: firstDate(heading, body) || fallbackDate,
      reason: oneLine(quoted ? quoted[1] : (heading.match(/\(([^)]*)\)/)?.[1] || title), 200),
      statement: oneLine(body.replace(/^[-*]\s+/gm, ''), 400),
      source,
      status: /SUPERSEDED/.test(heading) ? 'superseded' : 'active',
    });
  }
  return rules;
}

export default {
  id: 'claude-instructions',
  layers: ['instructions'],
  description: 'Global CLAUDE.md sections as rules; @imports as sources.',
  detect: (ctx) => ctx.exists('~/.claude/CLAUDE.md'),
  run(ctx, opts = {}) {
    const main = '~/.claude/CLAUDE.md';
    const text = ctx.readText(main);
    const st = ctx.stat(main);
    const sources = [{ id: 'global-claude-md', path: main, kind: 'global' }];
    for (const m of text.matchAll(/^@(\S+)\s*$/gm)) {
      const rel = m[1].startsWith('~') || m[1].startsWith('/') ? m[1] : `~/.claude/${m[1]}`;
      sources.push({ id: `import-${slug(m[1], 40)}`, path: ctx.tilde(ctx.p(rel)), kind: 'import' });
    }
    const rules = parseInstructionSections(text, {
      source: main,
      scopeKeywords: opts.scope_keywords,
      fallbackDate: st ? st.mtime.toISOString().slice(0, 10) : null,
    });
    return { instructions: { sources, rules }, _sources: [main] };
  },
};
