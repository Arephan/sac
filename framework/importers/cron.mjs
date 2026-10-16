// The user crontab (read-only `crontab -l`). Each non-comment line becomes a loop.
import { slug } from '../lib/util.mjs';

export default {
  id: 'cron',
  layers: ['loops'],
  description: 'User crontab entries.',
  detect: (ctx) => ctx.isRealHome && ctx.platform !== 'win32',
  run(ctx) {
    const text = ctx.tryExec('crontab', ['-l']) || '';
    const loops = [];
    for (const line of text.split('\n')) {
      const l = line.trim();
      if (!l || l.startsWith('#') || /^[A-Z_]+=/.test(l)) continue;
      const parts = l.split(/\s+/);
      const spec = parts[0].startsWith('@') ? parts.slice(0, 1) : parts.slice(0, 5);
      const cmd = parts.slice(spec.length).join(' ');
      loops.push({ id: `cron-${slug(cmd, 40)}`, runner: 'cron', enabled: true, status: 'installed', schedule: { cron: spec.join(' ') }, program: [ctx.scrub(cmd)] });
    }
    return { loops };
  },
};
