// Machine facts, which CLIs exist, and git repos directly under $HOME.
// options.clis: extra CLI names to probe. options.repo_roots: dirs to scan (default ["~"]).
const DEFAULT_CLIS = ['claude', 'node', 'npm', 'python3', 'git', 'gh', 'brew', 'docker', 'codex', 'gemini', 'wrangler', 'gcloud', 'firebase', 'eas', 'xcodebuild', 'ffmpeg', 'rtk', 'uv', 'bun', 'deno'];

export default {
  id: 'infra',
  layers: ['infra'],
  description: 'OS, installed CLIs, git repos under the home dir.',
  detect: () => true,
  run(ctx, opts = {}) {
    const machine = { platform: ctx.platform, arch: process.arch, node: process.version };
    if (ctx.platform === 'darwin') machine.os = (ctx.tryExec('sw_vers', ['-productVersion']) || '').trim();
    else machine.os = (ctx.tryExec('uname', ['-sr']) || '').trim();

    const clis = [];
    for (const name of [...new Set([...DEFAULT_CLIS, ...(opts.clis || [])])]) {
      const p = (ctx.tryExec('which', [name]) || '').trim();
      clis.push({ id: name, present: !!p, ...(p ? { path: ctx.tilde(p) } : {}) });
    }

    const repos = [];
    for (const root of opts.repo_roots || ['~']) {
      for (const d of ctx.list(root)) {
        if (d.startsWith('.') && !opts.include_hidden) continue;
        const rel = `${root}/${d}`;
        if (!ctx.isDir(`${rel}/.git`)) continue;
        const abs = ctx.p(rel);
        const remote = (ctx.tryExec('git', ['-C', abs, 'remote', 'get-url', 'origin']) || '').trim();
        const last = (ctx.tryExec('git', ['-C', abs, 'log', '-1', '--format=%cs']) || '').trim();
        repos.push({ id: d, path: ctx.tilde(abs), ...(remote ? { remote: ctx.scrub(remote.replace(/\/\/[^@/]+@/, '//')) } : {}), ...(last ? { last_commit: last } : {}) });
      }
    }
    return { infra: { machine, clis, repos } };
  },
};
