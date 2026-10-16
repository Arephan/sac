// Importer registry. Built-ins live here; an instance can add its own by dropping
// `*.mjs` files in <instance>/importers/ (same interface, see docs/importers.md).
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import claudeSettings from './claude-settings.mjs';
import claudeMcp from './claude-mcp.mjs';
import claudePlugins from './claude-plugins.mjs';
import claudeSkills from './claude-skills.mjs';
import claudeInstructions from './claude-instructions.mjs';
import claudeMemory from './claude-memory.mjs';
import launchd from './launchd.mjs';
import cron from './cron.mjs';
import infra from './infra.mjs';
import fileState from './file-state.mjs';

export const BUILTIN_IMPORTERS = [
  claudeSettings, claudeMcp, claudePlugins, claudeSkills, claudeInstructions, claudeMemory, launchd, cron, infra, fileState,
];

export function validateImporter(imp, where) {
  const problems = [];
  if (!imp || typeof imp !== 'object') problems.push('default export must be an object');
  else {
    if (typeof imp.id !== 'string' || !/^[a-z0-9-]+$/.test(imp.id)) problems.push('id must be kebab-case');
    if (!Array.isArray(imp.layers) || !imp.layers.length) problems.push('layers must be a non-empty array');
    if (typeof imp.run !== 'function') problems.push('run(ctx, options) is required');
    if (imp.detect && typeof imp.detect !== 'function') problems.push('detect must be a function');
  }
  if (problems.length) throw new Error(`importer ${where}: ${problems.join('; ')}`);
  return imp;
}

export async function loadImporters(instanceDir) {
  const list = [...BUILTIN_IMPORTERS];
  const dir = instanceDir && path.join(instanceDir, 'importers');
  if (dir && fs.existsSync(dir)) {
    for (const f of fs.readdirSync(dir).filter((x) => x.endsWith('.mjs')).sort()) {
      const mod = await import(pathToFileURL(path.join(dir, f)).href);
      const imp = validateImporter(mod.default, f);
      const i = list.findIndex((x) => x.id === imp.id);
      if (i >= 0) list[i] = imp; else list.push(imp);
    }
  }
  return list;
}
