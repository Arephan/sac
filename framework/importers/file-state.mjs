// Checks state files declared in the instance (data.state[].path) and channel files
// (todo/journal paths) for existence. Never reads their contents.
export default {
  id: 'file-state',
  layers: ['data'],
  description: 'Existence check for declared state files (contents never read).',
  detect: () => true,
  run(ctx, opts = {}) {
    const declared = opts.declared || [];
    const state = declared.map((s) => ({ ...s, exists: ctx.exists(s.path) }));
    return { data: { state } };
  },
};
