// Claude Code auto-memory: one markdown file per fact with frontmatter
// (name, description, metadata.type = user|feedback|project|reference).
//   feedback  -> instructions.rules   (HARD in description = hard; else standing)
//   project   -> projects
//   reference -> data.references
//   user      -> identity.person.profile_refs (file names only, contents never copied)
// The memory dir is found from $HOME, so nothing about the user is hardcoded.
import YAML from 'yaml';
import { claudeProjectSlug } from '../lib/util.mjs';
import { classifyScope, severityOf, firstDate, oneLine } from '../lib/rules.mjs';

function parse(text) {
  const m = text.match(/^---\n([\s\S]*?)\n---\n?([\s\S]*)$/);
  if (!m) return { fm: {}, body: text };
  let fm = {};
  try { fm = YAML.parse(m[1]) || {}; } catch {
    for (const line of m[1].split('\n')) { const kv = line.match(/^\s*(\w+):\s*(.*)$/); if (kv) fm[kv[1]] = kv[2].replace(/^"|"$/g, ''); }
  }
  return { fm, body: m[2] };
}

function statusOf(desc) {
  const d = desc.toUpperCase();
  if (/KILLED|DEAD\b|NEVER REBUILD/.test(d)) return 'killed';
  if (/SUPERSEDED/.test(d)) return 'superseded';
  if (/HALT|FROZEN|PAUSED|STOPPED/.test(d)) return 'halted';
  if (/BLOCKED|WALL\b|PENDING/.test(d)) return 'blocked';
  if (/\bLIVE\b|SHIPPED|PUBLIC\b/.test(d)) return 'live';
  return 'active';
}

export default {
  id: 'claude-memory',
  layers: ['instructions', 'projects', 'data', 'identity'],
  description: 'Auto-memory files: feedback=rules, project=projects, reference=references.',
  detect: (ctx) => ctx.isDir(`~/.claude/projects/${claudeProjectSlug(ctx.home)}/memory`),
  run(ctx, opts = {}) {
    const dir = opts.dir || `~/.claude/projects/${claudeProjectSlug(ctx.home)}/memory`;
    const files = ctx.list(dir).filter((f) => f.endsWith('.md') && !/^(MEMORY|REFERENCE)\.md$/.test(f));
    const rules = [], projects = [], references = [], profile = [];
    const byType = {};
    for (const f of files) {
      const path = `${dir}/${f}`;
      let text;
      try { text = ctx.readText(path); } catch (e) { ctx.warn(e.message); continue; }
      const { fm, body } = parse(text);
      const type = fm.metadata?.type || fm.type || 'unknown';
      byType[type] = (byType[type] || 0) + 1;
      const id = String(fm.name || f.replace(/\.md$/, ''));
      const desc = String(fm.description || '');
      const src = ctx.tilde(ctx.p(path));
      const mdate = String(fm.metadata?.modified || '').slice(0, 10);
      const fileDate = ctx.stat(path)?.mtime.toISOString().slice(0, 10) || null;
      if (type === 'feedback') {
        const why = body.match(/\*\*Why:?\*\*:?\s*([^\n]+)/i)?.[1];
        rules.push({
          id: `mem-${id}`,
          scope: classifyScope(desc + ' ' + id.replace(/[_-]/g, ' '), opts.scope_keywords),
          severity: severityOf(desc),
          date: firstDate(desc, body.slice(0, 400)) || (/^\d{4}-\d\d-\d\d$/.test(mdate) ? mdate : fileDate),
          reason: oneLine(why || desc, 240),
          statement: oneLine(desc, 280),
          source: src,
          status: /SUPERSEDED/i.test(desc) ? 'superseded' : 'active',
        });
      } else if (type === 'project') {
        const p = desc.match(/~\/[\w.@-]+(?:\/[\w.@-]+)*/)?.[0];
        projects.push({ id, description: oneLine(desc, 240), status: statusOf(desc), ...(p ? { path: p } : {}), source: src });
      } else if (type === 'reference') {
        references.push({ id, description: oneLine(desc, 200), source: src });
      } else if (type === 'user') {
        profile.push(src);
      }
    }
    return {
      instructions: { sources: [{ id: 'memory', path: ctx.tilde(ctx.p(dir)), kind: 'memory' }], rules },
      projects,
      data: {
        memory: { dir: ctx.tilde(ctx.p(dir)), index: ctx.tilde(ctx.p(`${dir}/MEMORY.md`)), files: files.length, by_type: byType },
        references,
      },
      identity: profile.length ? { person: { profile_refs: profile } } : {},
      _sources: [dir],
    };
  },
};
