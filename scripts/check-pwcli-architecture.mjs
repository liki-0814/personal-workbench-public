import fs from 'node:fs';
import path from 'node:path';

const root = process.cwd();
const errors = [];
const config = JSON.parse(fs.readFileSync(path.join(root, 'config/pwcli-architecture-allowlist.json'), 'utf8'));
const capabilitiesPath = path.join(root, 'config/pwcli-runtime-capabilities.json');
const capabilitiesSource = fs.readFileSync(capabilitiesPath, 'utf8');
const capabilities = JSON.parse(capabilitiesSource);
if (capabilities.schemaVersion !== 2) errors.push('runtime capability baseline schemaVersion must be 2');
for (const profile of ['web_main', 'cli_oneshot', 'internal_worker', 'acp_external_worker']) {
  if (!capabilities.profiles?.[profile]) errors.push(`missing runtime capability baseline: ${profile}`);
}
for (const profile of ['web_main', 'cli_oneshot', 'internal_worker']) {
  const baseline = capabilities.profiles?.[profile];
  for (const field of [
    'compositionOwner',
    'transport',
    'harnessProfile',
    'permissionModeSource',
    'memoryModeSource',
    'backgroundTools',
  ]) {
    if (baseline?.[field] === undefined) {
      errors.push(`runtime capability baseline ${profile} is missing ${field}`);
    }
  }
}
if (/(api.?key|token|prompt|message|workspace|\/Users\/|\/home\/)/i.test(capabilitiesSource)) {
  errors.push('runtime capability baseline contains a secret or user-data field');
}

function walk(entry) {
  if (!fs.existsSync(entry)) return [];
  const stat = fs.statSync(entry);
  if (stat.isFile()) return entry.endsWith('.rs') ? [entry] : [];
  return fs.readdirSync(entry, { withFileTypes: true }).flatMap(item =>
    walk(path.join(entry, item.name))
  );
}

function relative(file) {
  return path.relative(root, file).split(path.sep).join('/');
}

const contractFiles = walk(path.join(root, 'pwcli/src/contracts'));
const forbiddenContractPatterns = [
  ['Axum', /\baxum\b/],
  ['SQLite', /\brusqlite\b/],
  ['Git implementation', /crate::git(?:::|\b)/],
  ['service implementation', /crate::service(?:::|\b)/],
  ['task implementation', /crate::task(?:::|\b)/],
  ['product configuration', /crate::config(?:::|\b)/],
  ['provider adapter', /crate::llm::adapter(?:::|\b)/],
  ['filesystem I/O', /(?:std|tokio)::fs(?:::|\b)/],
];
for (const file of contractFiles) {
  const source = fs.readFileSync(file, 'utf8');
  for (const [label, pattern] of forbiddenContractPatterns) {
    if (pattern.test(source)) errors.push(`${relative(file)}: contracts must not depend on ${label}`);
  }
}

const coreFiles = [
  path.join(root, 'pwcli/src/agent_runner.rs'),
  ...walk(path.join(root, 'pwcli/src/graph')),
  ...walk(path.join(root, 'pwcli/src/harness')),
  ...walk(path.join(root, 'pwcli/src/hooks')),
  ...walk(path.join(root, 'pwcli/src/middleware')),
];
const allowedEdges = new Set(config.coreForbiddenEdges);
for (const file of coreFiles) {
  const source = fs.readFileSync(file, 'utf8');
  for (const target of ['service', 'task']) {
    if (new RegExp(`crate::${target}(?:::|\\b)`).test(source)) {
      const edge = `${relative(file)} -> ${target}`;
      if (!allowedEdges.has(edge)) errors.push(`${edge}: forbidden core dependency`);
    }
  }
}

const allowedTaskLocals = new Set(config.taskLocalFiles);
const expectedTaskLocalAllowlist = new Set([
  'pwcli/src/tools/fs_local.rs',
  'pwcli/src/tools/progress.rs',
]);
for (const file of allowedTaskLocals) {
  if (!expectedTaskLocalAllowlist.has(file)) errors.push(`${file}: implicit context allowlist may not expand`);
}
for (const file of expectedTaskLocalAllowlist) {
  if (!allowedTaskLocals.has(file)) errors.push(`${file}: required context/safety exception is missing`);
}
for (const file of walk(path.join(root, 'pwcli/src'))) {
  if (/\btask_local!\s*\{/.test(fs.readFileSync(file, 'utf8')) && !allowedTaskLocals.has(relative(file))) {
    errors.push(`${relative(file)}: new implicit task-local context is forbidden`);
  }
}

const retiredWebContext = path.join(root, 'pwcli/src/tools/web_context.rs');
if (fs.existsSync(retiredWebContext)) errors.push('pwcli/src/tools/web_context.rs: retired implicit context must not return');
for (const file of walk(path.join(root, 'pwcli/src'))) {
  const source = fs.readFileSync(file, 'utf8');
  for (const [label, pattern] of [
    ['web/session task-local context', /(?:crate::tools::)?web_context(?:::|\b)/],
    ['active-model task-local context', /\b(?:with_active_model|try_current_supports_vision|try_current_model_id|try_current_provider_id)\b/],
  ]) {
    if (pattern.test(source)) errors.push(`${relative(file)}: retired ${label} must not return`);
  }
}

const sourceRoot = path.join(root, 'pwcli/src');
const compositionFile = path.join(sourceRoot, 'composition/mod.rs');
const agentRunnerFile = path.join(sourceRoot, 'agent_runner.rs');

// RuntimeFactory is the sole production composition root. AgentRunner remains
// directly constructible only inside composition and its own unit tests.
for (const file of walk(sourceRoot)) {
  if (file === compositionFile || file === agentRunnerFile) continue;
  const source = fs.readFileSync(file, 'utf8');
  if (/\bAgentRunner\s*\{/.test(source)) {
    errors.push(`${relative(file)}: only composition may construct AgentRunner`);
  }
}

const entryFiles = [
  path.join(sourceRoot, 'app.rs'),
  path.join(sourceRoot, 'service/routes.rs'),
  path.join(sourceRoot, 'task/mod.rs'),
];
for (const file of entryFiles) {
  const source = fs.readFileSync(file, 'utf8');
  if (/\bAgentRunner\s*\{/.test(source)) {
    errors.push(`${relative(file)}: entry points must request AgentRuntime`);
  }
}

const webRoutes = fs.readFileSync(path.join(sourceRoot, 'service/routes.rs'), 'utf8');
for (const [label, pattern] of [
  ['LLM provider', /LlmClient::(?:from_config|with_provider)/],
  ['hook runner', /HookRunner::/],
  ['harness run options', /HarnessRunOptions::/],
  ['MoA reviewer', /fusion::moa::active_runtime/],
  ['tool schemas', /tool_registry\.to_schemas\(\)/],
]) {
  if (pattern.test(webRoutes)) {
    errors.push(`pwcli/src/service/routes.rs: Web routes must not assemble ${label}`);
  }
}

for (const file of walk(path.join(sourceRoot, 'tools'))) {
  const source = fs.readFileSync(file, 'utf8');
  if (/crate::service(?:::|\b)/.test(source)) {
    errors.push(`${relative(file)}: tools must not depend on service`);
  }
}

for (const file of [compositionFile, agentRunnerFile, ...contractFiles]) {
  const source = fs.readFileSync(file, 'utf8');
  for (const [label, pattern] of [
    ['HTTP/SSE', /\baxum\b|response::sse|\bSse\s*</],
    ['service implementation', /crate::service(?:::|\b)/],
  ]) {
    if (pattern.test(source)) {
      errors.push(`${relative(file)}: runtime/core/contracts must not depend on ${label}`);
    }
  }
}

const compositionSource = fs.readFileSync(compositionFile, 'utf8');
for (const [label, pattern] of [
  ['SessionManager', /\bSessionManager\b/],
  ['TaskBroker', /\bTaskBroker\b/],
  ['SQLite', /\brusqlite\b/],
  ['transport events', /\bStreamEvent\b/],
]) {
  if (pattern.test(compositionSource)) {
    errors.push(`pwcli/src/composition/mod.rs: RuntimeFactory/AgentRuntime must not own ${label}`);
  }
}

if (errors.length) {
  console.error(errors.join('\n'));
  process.exit(1);
}
console.log(`PWCLI architecture checks passed (${contractFiles.length} contract files, ${coreFiles.length} core files).`);
