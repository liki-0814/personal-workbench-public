import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

function extensionFiles() {
  const roots = [path.join(os.homedir(), '.pwcli', 'extensions')];
  const projectRoot = path.join(process.cwd(), '.pwcli');
  const trustPath = path.join(projectRoot, 'extensions.trust.json');
  try {
    const trust = JSON.parse(fs.readFileSync(trustPath, 'utf8'));
    if (trust?.trusted === true) roots.push(path.join(projectRoot, 'extensions'));
  } catch {}
  return roots.flatMap(root => {
    try {
      return fs.readdirSync(root)
        .filter(name => name.endsWith('.mjs') || name.endsWith('.js'))
        .map(name => path.join(root, name));
    } catch { return []; }
  });
}

async function loadAll() {
  const loaded = [];
  for (const file of extensionFiles()) {
    const stat = fs.statSync(file);
    const module = await import(`${pathToFileURL(file).href}?mtime=${stat.mtimeMs}`);
    loaded.push({
      file,
      name: module.manifest?.name || module.name || path.basename(file, path.extname(file)),
      manifest: module.manifest || {},
      tools: module.tools || {},
      commands: module.commands || {},
      onEvent: module.onEvent,
    });
  }
  return loaded;
}

async function handle(request) {
  const extensions = await loadAll();
  if (request.method === 'list') {
    return extensions.map(ext => ({
      name: ext.name,
      file: ext.file,
      manifest: ext.manifest,
      tools: Object.entries(ext.tools).map(([name, value]) => ({
        name,
        description: value?.description || value?.execute?.description || `Extension tool ${name}`,
        parameters: value?.parameters || value?.schema || value?.execute?.parameters || { type: 'object', properties: {} },
      })),
      commands: Object.keys(ext.commands),
    }));
  }
  const ext = extensions.find(item => item.name === request.extension);
  if (!ext) throw new Error(`extension not found: ${request.extension}`);
  if (request.method === 'invokeTool') {
    const definition = ext.tools[request.name];
    const tool = typeof definition === 'function' ? definition : definition?.execute;
    if (typeof tool !== 'function') throw new Error(`tool not found: ${request.name}`);
    return await tool(request.args || {});
  }
  if (request.method === 'invokeCommand') {
    const command = ext.commands[request.name];
    if (typeof command !== 'function') throw new Error(`command not found: ${request.name}`);
    return await command(request.args || {});
  }
  if (request.method === 'event') {
    if (typeof ext.onEvent === 'function') await ext.onEvent(request.event, request.payload);
    return { delivered: true };
  }
  throw new Error(`unknown extension host method: ${request.method}`);
}

let input = '';
for await (const chunk of process.stdin) input += chunk;
try {
  const request = JSON.parse(input);
  process.stdout.write(`${JSON.stringify({ result: await handle(request) })}\n`);
} catch (error) {
  process.stdout.write(`${JSON.stringify({ error: String(error?.message || error) })}\n`);
  process.exitCode = 1;
}
