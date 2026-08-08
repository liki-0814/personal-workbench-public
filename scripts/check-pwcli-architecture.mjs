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

const sourceRoot = path.join(root, 'pwcli/src');

// ---------------------------------------------------------------------------
// Source preprocessing: architecture rules apply to production code, so strip
// comments (including doc comments) and #[cfg(test)] items before matching.
// ---------------------------------------------------------------------------

function stripComments(source) {
  let out = '';
  let i = 0;
  const n = source.length;
  while (i < n) {
    const ch = source[i];
    // Raw strings: r"..."/r#"..."# — keep content verbatim.
    if (ch === 'r') {
      const raw = /^r(#*)"/.exec(source.slice(i, i + 8));
      const prev = out.length ? out[out.length - 1] : '';
      if (raw && !/[\w]/.test(prev)) {
        const closer = `"${raw[1]}`;
        const end = source.indexOf(closer, i + raw[0].length);
        const stop = end === -1 ? n : end + closer.length;
        out += source.slice(i, stop);
        i = stop;
        continue;
      }
    }
    // Regular string literals: keep verbatim so // inside them survives.
    if (ch === '"') {
      out += ch;
      i += 1;
      while (i < n) {
        if (source[i] === '\\') {
          out += source.slice(i, i + 2);
          i += 2;
          continue;
        }
        out += source[i];
        if (source[i] === '"') {
          i += 1;
          break;
        }
        i += 1;
      }
      continue;
    }
    if (ch === '/' && source[i + 1] === '/') {
      while (i < n && source[i] !== '\n') i += 1;
      continue;
    }
    if (ch === '/' && source[i + 1] === '*') {
      let depth = 1;
      i += 2;
      while (i < n && depth > 0) {
        if (source[i] === '/' && source[i + 1] === '*') { depth += 1; i += 2; continue; }
        if (source[i] === '*' && source[i + 1] === '/') { depth -= 1; i += 2; continue; }
        i += 1;
      }
      out += ' ';
      continue;
    }
    out += ch;
    i += 1;
  }
  return out;
}

function stripTestItems(source) {
  let src = source;
  const marker = '#[cfg(test)]';
  let idx;
  while ((idx = src.indexOf(marker)) !== -1) {
    let j = idx + marker.length;
    // Skip whitespace and any stacked attributes (e.g. #[tokio::test]).
    for (;;) {
      while (j < src.length && /\s/.test(src[j])) j += 1;
      if (src.startsWith('#[', j)) {
        let depth = 0;
        while (j < src.length) {
          if (src[j] === '[') depth += 1;
          else if (src[j] === ']') {
            depth -= 1;
            if (depth === 0) { j += 1; break; }
          }
          j += 1;
        }
        continue;
      }
      break;
    }
    // Consume the following item: brace-delimited or up to ';'.
    let k = j;
    let brace = -1;
    let semi = -1;
    while (k < src.length) {
      if (src[k] === '{') { brace = k; break; }
      if (src[k] === ';') { semi = k; break; }
      k += 1;
    }
    let end;
    if (semi !== -1 && (brace === -1 || semi < brace)) {
      end = semi + 1;
    } else if (brace !== -1) {
      let depth = 0;
      k = brace;
      for (; k < src.length; k += 1) {
        if (src[k] === '{') depth += 1;
        else if (src[k] === '}') {
          depth -= 1;
          if (depth === 0) break;
        }
      }
      end = k + 1;
    } else {
      end = src.length;
    }
    src = src.slice(0, idx) + src.slice(end);
  }
  return src;
}

const productionCache = new Map();
function productionSource(file) {
  if (!productionCache.has(file)) {
    productionCache.set(file, stripTestItems(stripComments(fs.readFileSync(file, 'utf8'))));
  }
  return productionCache.get(file);
}

// ---------------------------------------------------------------------------
// Four-layer dependency direction (ADR-0001): app -> runtime -> agent_core -> ai.
// Forbidden: any reference to a layer above the current one.
// ---------------------------------------------------------------------------

const forbiddenUpstream = {
  ai: ['agent_core', 'runtime', 'app'],
  agent_core: ['runtime', 'app'],
  runtime: ['app'],
  app: [],
};

const layerExceptions = config.layerExceptions ?? {};
const exceptionByEdge = new Map();
for (const [edge, files] of Object.entries(layerExceptions)) {
  const [from, to] = edge.split('->');
  if (!forbiddenUpstream[from]?.includes(to)) {
    errors.push(`unknown layer exception edge: ${edge}`);
    continue;
  }
  exceptionByEdge.set(edge, new Set(files));
  for (const file of files) {
    if (!fs.existsSync(path.join(root, file))) {
      errors.push(`layer exception no longer applies, remove it from the allowlist: ${file}`);
    }
  }
}

const layerFileCounts = {};
for (const [layer, forbidden] of Object.entries(forbiddenUpstream)) {
  const files = walk(path.join(sourceRoot, layer));
  layerFileCounts[layer] = files.length;
  if (!files.length) errors.push(`layer directory missing or empty: pwcli/src/${layer}`);
  for (const file of files) {
    const source = productionSource(file);
    for (const target of forbidden) {
      const edge = `${layer}->${target}`;
      if (exceptionByEdge.get(edge)?.has(relative(file))) continue;
      if (new RegExp(`crate::${target}(?:::|\\b)`).test(source)) {
        errors.push(`${relative(file)}: forbidden dependency ${edge} (ADR-0001; add nothing here without an ADR)`);
      }
    }
  }
}

// ---------------------------------------------------------------------------
// Contracts: stable ports must stay free of infrastructure and adapters.
// ---------------------------------------------------------------------------

const contractFiles = walk(path.join(sourceRoot, 'agent_core/contracts'));
if (!contractFiles.length) errors.push('agent_core/contracts missing: stable port layer not found');
const forbiddenContractPatterns = [
  ['Axum', /\baxum\b/],
  ['SQLite', /\brusqlite\b/],
  ['provider adapter', /crate::ai::llm::adapter(?:::|\b)/],
  ['filesystem I/O', /(?:std|tokio)::fs(?:::|\b)/],
];
for (const file of contractFiles) {
  const source = productionSource(file);
  for (const [label, pattern] of forbiddenContractPatterns) {
    if (pattern.test(source)) errors.push(`${relative(file)}: contracts must not depend on ${label}`);
  }
}

// ---------------------------------------------------------------------------
// Implicit task-local context is forbidden outside the legacy allowlist.
// ---------------------------------------------------------------------------

const allowedTaskLocals = new Set(config.taskLocalFiles);
const expectedTaskLocalAllowlist = new Set([
  'pwcli/src/runtime/tools/fs_local.rs',
  'pwcli/src/runtime/tools/progress.rs',
]);
for (const file of allowedTaskLocals) {
  if (!expectedTaskLocalAllowlist.has(file)) errors.push(`${file}: implicit context allowlist may not expand`);
}
for (const file of expectedTaskLocalAllowlist) {
  if (!allowedTaskLocals.has(file)) errors.push(`${file}: required context/safety exception is missing`);
}
for (const file of walk(sourceRoot)) {
  if (/\btask_local!\s*\{/.test(fs.readFileSync(file, 'utf8')) && !allowedTaskLocals.has(relative(file))) {
    errors.push(`${relative(file)}: new implicit task-local context is forbidden`);
  }
}

// ---------------------------------------------------------------------------
// Retired ambient contexts must not return.
// ---------------------------------------------------------------------------

const retiredWebContext = path.join(sourceRoot, 'runtime/tools/web_context.rs');
if (fs.existsSync(retiredWebContext)) errors.push('pwcli/src/runtime/tools/web_context.rs: retired implicit context must not return');
for (const file of walk(sourceRoot)) {
  const source = productionSource(file);
  for (const [label, pattern] of [
    ['web/session task-local context', /\bweb_context(?:::|\b)/],
    ['active-model task-local context', /\b(?:with_active_model|try_current_supports_vision|try_current_model_id|try_current_provider_id)\b/],
  ]) {
    if (pattern.test(source)) errors.push(`${relative(file)}: retired ${label} must not return`);
  }
}

// ---------------------------------------------------------------------------
// RuntimeFactory is the sole production composition root. AgentRunner remains
// directly constructible only inside composition and its own unit tests.
// ---------------------------------------------------------------------------

const compositionFile = path.join(sourceRoot, 'app/composition/mod.rs');
const agentRunnerFile = path.join(sourceRoot, 'agent_core/runner.rs');
if (!fs.existsSync(compositionFile)) errors.push('app/composition/mod.rs missing: composition root not found');
if (!fs.existsSync(agentRunnerFile)) errors.push('agent_core/runner.rs missing: agent core runner not found');

for (const file of [...walk(sourceRoot), path.join(sourceRoot, 'main.rs')]) {
  if (file === compositionFile || file === agentRunnerFile) continue;
  if (!fs.existsSync(file)) continue;
  const source = fs.readFileSync(file, 'utf8');
  if (/\bAgentRunner\s*\{/.test(source)) {
    errors.push(`${relative(file)}: only composition may construct AgentRunner`);
  }
}

// ---------------------------------------------------------------------------
// Web route handlers must request a pre-assembled AgentRuntime; they must not
// assemble LLM clients, hooks, harness options, reviewers or tool schemas.
// ---------------------------------------------------------------------------

const routeFiles = [
  path.join(sourceRoot, 'app/service/routes.rs'),
  path.join(sourceRoot, 'app/service/acp_routes.rs'),
  path.join(sourceRoot, 'app/service/memory_routes.rs'),
  ...walk(path.join(sourceRoot, 'app/service/routes')),
];
for (const file of routeFiles) {
  if (!fs.existsSync(file)) {
    errors.push(`${relative(file)}: expected Web route file is missing`);
    continue;
  }
  const source = fs.readFileSync(file, 'utf8');
  for (const [label, pattern] of [
    ['LLM provider', /LlmClient::(?:from_config|with_provider)/],
    ['hook runner', /HookRunner::/],
    ['harness run options', /HarnessRunOptions::/],
    ['MoA reviewer', /fusion::moa::active_runtime/],
    ['tool schemas', /tool_registry\.to_schemas\(\)/],
  ]) {
    if (pattern.test(source)) {
      errors.push(`${relative(file)}: Web routes must not assemble ${label}`);
    }
  }
}

// Tools must not depend on the Web service layer (ADR-0001).
for (const file of walk(path.join(sourceRoot, 'runtime/tools'))) {
  if (/crate::app::service(?:::|\b)/.test(productionSource(file))) {
    errors.push(`${relative(file)}: tools must not depend on service`);
  }
}

// Composition, the agent core runner and contracts must stay transport-free.
for (const file of [compositionFile, agentRunnerFile, ...contractFiles]) {
  const source = productionSource(file);
  for (const [label, pattern] of [
    ['HTTP/SSE', /\baxum\b|response::sse|\bSse\s*</],
    ['service implementation', /crate::app::service(?:::|\b)/],
  ]) {
    if (pattern.test(source)) {
      errors.push(`${relative(file)}: runtime/core/contracts must not depend on ${label}`);
    }
  }
}

// ---------------------------------------------------------------------------
// RuntimeFactory/AgentRuntime never owns durable state or transport plumbing.
// ---------------------------------------------------------------------------

if (fs.existsSync(compositionFile)) {
  const compositionSource = productionSource(compositionFile);
  for (const [label, pattern] of [
    ['SessionManager', /\bSessionManager\b/],
    ['TaskBroker', /\bTaskBroker\b/],
    ['SQLite', /\brusqlite\b/],
    ['transport events', /\bStreamEvent\b/],
  ]) {
    if (pattern.test(compositionSource)) {
      errors.push(`pwcli/src/app/composition/mod.rs: RuntimeFactory/AgentRuntime must not own ${label}`);
    }
  }
}

if (errors.length) {
  console.error(errors.join('\n'));
  process.exit(1);
}
const exceptionCount = Object.values(layerExceptions).reduce((sum, files) => sum + files.length, 0);
console.log(
  `PWCLI architecture checks passed (` +
  `ai ${layerFileCounts.ai}, agent_core ${layerFileCounts.agent_core}, ` +
  `runtime ${layerFileCounts.runtime}, app ${layerFileCounts.app} files; ` +
  `${contractFiles.length} contract files; ${exceptionCount} tracked layer exceptions).`
);
