import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import vm from 'node:vm';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const sourceRoot = path.join(root, 'pwcli', 'resources', 'archify');
const rendererRoot = path.join(sourceRoot, 'renderers');
const outputRoot = path.join(sourceRoot, 'generated');
const diagramTypes = ['architecture', 'workflow', 'sequence', 'dataflow', 'lifecycle'];
const comparisonExamples = {
  architecture: 'web-app.architecture.json',
  workflow: 'agent-tool-call.workflow.json',
  sequence: 'cache-miss-request.sequence.json',
  dataflow: 'product-analytics.dataflow.json',
  lifecycle: 'agent-run.lifecycle.json',
};

const cliPath = path.join(rendererRoot, 'shared', 'cli.mjs');
const diagnosticsPath = path.join(rendererRoot, 'shared', 'diagnostics.mjs');

function browserCliSource() {
  let source = fs.readFileSync(cliPath, 'utf8');
  source = source
    .replace("import fs from 'node:fs';\n", '')
    .replace("import path from 'node:path';\n", '')
    .replace("import { verifyRepositoryEvidence } from './repository-evidence.mjs';\n", '');

  const loadStart = source.indexOf('export function loadDiagram(');
  const loadEnd = source.indexOf('\n\nconst START_TYPES', loadStart);
  if (loadStart < 0 || loadEnd < 0) throw new Error('Unable to locate Archify loadDiagram');
  source = `${source.slice(0, loadStart)}export function loadDiagram({ diagramType }) {
  const diagram = globalThis.__ARCHIFY_INPUT__;
  validateSchema(diagramType, diagram);
  validateGuidedViews(diagramType, diagram);
  validateRelationshipIds(diagramType, diagram);
  validateEngineeringProfile(diagramType, diagram);
  return {
    diagram,
    template: globalThis.__ARCHIFY_TEMPLATE__,
    outPath: '',
    sourceEvidence: null,
  };
}${source.slice(loadEnd)}`;

  const writeStart = source.indexOf('export function writeDiagram(');
  const writeEnd = source.indexOf('\n\nconst SEMANTIC_COLLECTIONS', writeStart);
  if (writeStart < 0 || writeEnd < 0) throw new Error('Unable to locate Archify writeDiagram');
  source = `${source.slice(0, writeStart)}export function writeDiagram({ diagramType, meta, footerLabel, svg, cards, sourceEvidence = null }) {
  if (!START_TYPES.has(diagramType)) {
    throw new Error(\`writeDiagram: unknown diagram type \${JSON.stringify(diagramType)}\`);
  }
  const guidedHint = Array.isArray(meta.views) && meta.views.length
    ? ' &bull; <kbd>[</kbd>/<kbd>]</kbd> views &bull; <kbd>P</kbd> play story'
    : '';
  const startUrl = \`https://tt-a1i.github.io/archify/start.html?type=\${esc(diagramType)}\`;
  globalThis.__ARCHIFY_OUTPUT__ = applyTemplate(globalThis.__ARCHIFY_TEMPLATE__, {
    title: meta.title,
    subtitle: meta.subtitle,
    footer: \`\${footerLabel} &bull; Built with Archify<span class="no-print"> &bull; <a class="artifact-start-link" href="\${startUrl}" target="_blank" rel="noopener noreferrer">Create yours &nearr;</a> &bull; Hover to trace &bull; <kbd>R</kbd> route &bull; Click to focus &bull; <kbd>+</kbd>/<kbd>&minus;</kbd> zoom &bull; <kbd>M</kbd> radar\${guidedHint} &bull; <kbd>T</kbd> theme &bull; <kbd>E</kbd> export</span>\`,
    svg,
    cards: renderCards(cards),
    visualPreset: meta.visual_preset || 'classic',
    guidedViews: meta.views || [],
    sourceEvidence,
  });
}${source.slice(writeEnd)}`;
  return source;
}

const diagnosticsShim = `
export function recordDiagnostic() {}
export function throwDiagnosticError(message, diagnostics = []) {
  const detail = diagnostics.map((item) => item?.message || String(item)).join('\\n');
  throw new Error(detail ? message + '\\n' + detail : message);
}
export function throwDiagnosticProblems(prefix, problems = []) {
  if (problems.length) throw new Error(prefix + ':\\n- ' + problems.join('\\n- '));
}
export function installRendererDiagnosticBoundary() {}
`;

const browserPlugin = {
  name: 'archify-browser-runtime',
  setup(builder) {
    builder.onResolve({ filter: /.*/ }, args => {
      const absolute = path.resolve(args.resolveDir, args.path);
      if (absolute === cliPath) return { path: cliPath, namespace: 'archify-cli' };
      if (absolute === diagnosticsPath) {
        return { path: diagnosticsPath, namespace: 'archify-diagnostics' };
      }
      return null;
    });
    builder.onLoad({ filter: /render-(?:architecture|workflow|sequence|dataflow|lifecycle)\.mjs$/ }, args => ({
      contents: fs.readFileSync(args.path, 'utf8')
        .replace("import path from 'node:path';\n", '')
        .replace("import { fileURLToPath } from 'node:url';\n", '')
        .replace("const __dirname = path.dirname(fileURLToPath(import.meta.url));\n", "const __dirname = '';\n"),
      loader: 'js',
      resolveDir: path.dirname(args.path),
    }));
    builder.onLoad({ filter: /.*/, namespace: 'archify-cli' }, () => ({
      contents: browserCliSource(),
      loader: 'js',
      resolveDir: path.dirname(cliPath),
    }));
    builder.onLoad({ filter: /.*/, namespace: 'archify-diagnostics' }, () => ({
      contents: diagnosticsShim,
      loader: 'js',
    }));
  },
};

fs.mkdirSync(outputRoot, { recursive: true });
for (const diagramType of diagramTypes) {
  await build({
    entryPoints: [path.join(rendererRoot, diagramType, `render-${diagramType}.mjs`)],
    outfile: path.join(outputRoot, `${diagramType}.js`),
    bundle: true,
    format: 'iife',
    platform: 'browser',
    target: 'es2020',
    minify: true,
    legalComments: 'none',
    banner: {
      js: 'var process={argv:[],env:{},exit(){throw new Error("unexpected process.exit")}};',
    },
    plugins: [browserPlugin],
  });
}

const comparisonRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'pwcli-archify-'));
try {
  const template = fs.readFileSync(path.join(sourceRoot, 'assets', 'template.html'), 'utf8');
  for (const diagramType of diagramTypes) {
    const inputPath = path.join(sourceRoot, 'examples', comparisonExamples[diagramType]);
    const originalPath = path.join(comparisonRoot, `${diagramType}.html`);
    const result = spawnSync(
      process.execPath,
      [
        path.join(rendererRoot, diagramType, `render-${diagramType}.mjs`),
        inputPath,
        originalPath,
      ],
      { encoding: 'utf8' },
    );
    if (result.status !== 0) {
      throw new Error(`Original Archify ${diagramType} renderer failed:\n${result.stderr}`);
    }
    const context = {
      __ARCHIFY_INPUT__: JSON.parse(fs.readFileSync(inputPath, 'utf8')),
      __ARCHIFY_TEMPLATE__: template,
    };
    context.globalThis = context;
    vm.runInNewContext(
      fs.readFileSync(path.join(outputRoot, `${diagramType}.js`), 'utf8'),
      context,
    );
    const original = fs.readFileSync(originalPath, 'utf8');
    if (context.__ARCHIFY_OUTPUT__ !== original) {
      throw new Error(`Embedded ${diagramType} renderer does not match the original Node CLI`);
    }
  }
} finally {
  fs.rmSync(comparisonRoot, { recursive: true, force: true });
}

console.log(`Built ${diagramTypes.length} embedded Archify renderers in ${outputRoot}`);
