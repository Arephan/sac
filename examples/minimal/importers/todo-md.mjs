// Example custom importer: report a markdown todo file as a channel, with open/done counts.
// Drop a file like this into <instance>/importers/ and list its id under import.importers.
export default {
  id: 'todo-md',
  layers: ['channels'],
  description: 'Markdown todo file (- [ ] / - [x]) as channels.todo with counts.',
  // Only run when the file exists. ctx paths accept ~ and are resolved against --home.
  detect: (ctx) => ctx.exists('~/assistant/todo.md'),
  run(ctx) {
    const text = ctx.readText('~/assistant/todo.md');
    const open = (text.match(/^\s*- \[ \]/gm) || []).length;
    const done = (text.match(/^\s*- \[x\]/gim) || []).length;
    return { channels: { todo: [{ id: 'todo', path: '~/assistant/todo.md', open, done }] } };
  },
};
