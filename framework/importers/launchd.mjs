// macOS launchd user agents: every job in ~/Library/LaunchAgents, including parked files
// (*.plist.disabled-*, *.plist.killed-*, a disabled/ subfolder), labels the system has marked
// disabled without a plist, and live load state. Strictly read-only: `plutil -convert json -o -`,
// `launchctl list`, `launchctl print-disabled`. Never load/unload/bootstrap anything.
//
// options.managed_prefixes: label prefixes that belong to this person (others = vendor jobs).

const DIR = '~/Library/LaunchAgents';

function schedule(j) {
  const s = {};
  if (j.StartInterval) s.every_seconds = j.StartInterval;
  if (j.StartCalendarInterval) {
    const arr = Array.isArray(j.StartCalendarInterval) ? j.StartCalendarInterval : [j.StartCalendarInterval];
    s.calendar = arr.map((c) => Object.entries(c).map(([k, v]) => `${k.toLowerCase()}=${v}`).join(','));
  }
  if (j.KeepAlive) s.keep_alive = true;
  if (j.RunAtLoad) s.run_at_load = true;
  if (j.WatchPaths) s.watch_paths = j.WatchPaths;
  return s;
}

export default {
  id: 'launchd',
  layers: ['loops'],
  description: 'launchd user agents incl. disabled/parked/removed ones (read-only).',
  detect: (ctx) => ctx.platform === 'darwin' && ctx.isDir(DIR),
  run(ctx, opts = {}) {
    const prefixes = opts.managed_prefixes || [];
    const loaded = new Map();
    const disabledMap = new Map();
    if (ctx.isRealHome) {
      const list = ctx.tryExec('launchctl', ['list']) || '';
      for (const line of list.split('\n').slice(1)) {
        const [pid, status, label] = line.trim().split(/\s+/);
        if (label) loaded.set(label, { pid: pid === '-' ? null : Number(pid), last_exit: Number(status) });
      }
      const uid = process.getuid?.();
      const pd = uid != null ? ctx.tryExec('launchctl', ['print-disabled', `gui/${uid}`]) || '' : '';
      for (const m of pd.matchAll(/"([^"]+)"\s*=>\s*(enabled|disabled|true|false)/g)) {
        disabledMap.set(m[1], m[2] === 'disabled' || m[2] === 'true');
      }
    }

    const jobs = new Map();
    const files = [
      ...ctx.list(DIR).map((f) => ({ f, rel: `${DIR}/${f}`, parked: false })),
      ...ctx.list(`${DIR}/disabled`).map((f) => ({ f, rel: `${DIR}/disabled/${f}`, parked: true })),
    ].filter(({ f }) => /\.plist/.test(f));

    // Group every file by label, then describe the job from its best file:
    // installed plist > parked copy (disabled/killed/renamed) > backup copy.
    const groups = new Map();
    for (const { f, rel, parked } of files) {
      const suffix = f.replace(/^.*?\.plist\.?/, '');
      const rank = /\.plist$/.test(f) && !parked ? 0 : /^bak/.test(suffix) ? 2 : 1;
      let j = null;
      const raw = ctx.tryExec('plutil', ['-convert', 'json', '-o', '-', ctx.p(rel)]);
      if (raw) { try { j = JSON.parse(raw); } catch { /* unreadable plist */ } }
      const label = j?.Label || f.replace(/\.plist.*$/, '');
      if (!groups.has(label)) groups.set(label, []);
      groups.get(label).push({ rel, rank, suffix, parked, j });
    }
    for (const [label, list] of groups) {
      list.sort((a, b) => a.rank - b.rank);
      const best = list[0];
      const j = best.j;
      const state = best.rank === 0 ? 'installed' : best.rank === 2 ? 'backup-only'
        : best.parked && !best.suffix ? 'parked' : `parked:${best.suffix.replace(/-?\d{6,8}$/, '') || 'renamed'}`;
      jobs.set(label, {
        id: label,
        runner: 'launchd',
        enabled: false,
        status: state,
        managed: prefixes.length ? prefixes.some((p) => label.startsWith(p)) : true,
        schedule: j ? schedule(j) : {},
        program: j ? ctx.scrub((j.ProgramArguments || (j.Program ? [j.Program] : [])).map(String)) : [],
        ...(j?.WorkingDirectory ? { working_dir: ctx.tilde(j.WorkingDirectory) } : {}),
        ...(j?.StandardOutPath ? { log: ctx.tilde(j.StandardOutPath) } : {}),
        env_names: Object.keys(j?.EnvironmentVariables || {}).sort(),
        files: list.map((x) => x.rel),
      });
    }

    // Labels launchd has an override for but no plist on disk: removed jobs.
    for (const [label, isDisabled] of disabledMap) {
      if (jobs.has(label)) continue;
      if (prefixes.length && !prefixes.some((p) => label.startsWith(p))) continue;
      jobs.set(label, { id: label, runner: 'launchd', enabled: false, status: isDisabled ? 'removed:override-disabled' : 'removed:override-enabled', managed: true, schedule: {}, program: [], env_names: [], files: [] });
    }

    for (const job of jobs.values()) {
      const l = loaded.get(job.id);
      const overrideDisabled = disabledMap.get(job.id) === true;
      if (job.status === 'installed') {
        job.enabled = !overrideDisabled && (ctx.isRealHome ? loaded.has(job.id) : true);
        job.status = overrideDisabled ? 'installed:override-disabled' : l ? (l.pid ? 'running' : 'loaded') : ctx.isRealHome ? 'installed:not-loaded' : 'installed';
      }
      if (!job.files.length) delete job.files;
    }
    return { loops: [...jobs.values()].sort((a, b) => a.id.localeCompare(b.id)), _sources: [DIR] };
  },
};
