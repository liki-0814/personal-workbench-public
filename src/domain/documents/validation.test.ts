import { describe, expect, it } from 'vitest';
import { assertHtmlDocumentContent, assertMarkdownDocumentContent, validateEditableDocument } from './validation';

const html = '<!doctype html><html><body><h1>Title</h1></body></html>';

describe('HTML document validation', () => {
  it('accepts the single HTML document protocol', () => {
    expect(() => validateEditableDocument({
      manifest: {
        schemaVersion: 1,
        id: 'doc-1',
        kind: 'html',
        title: 'Document',
        status: 'draft',
        revision: 1,
        updatedAt: new Date(0).toISOString(),
        qaStatus: 'pending',
      },
      content: { html },
    })).not.toThrow();
  });

  it('reports incomplete HTML at the source field', () => {
    expect(() => assertHtmlDocumentContent({ html: '<p>fragment</p>' })).toThrow('/html');
  });

  it('accepts a Markdown document and rejects an empty body', () => {
    expect(() => validateEditableDocument({
      manifest: {
        schemaVersion: 1,
        id: 'doc-2',
        kind: 'markdown',
        title: 'Delegated result',
        status: 'ready',
        revision: 1,
        updatedAt: new Date(0).toISOString(),
        qaStatus: 'passed',
        runtime: 'delegated',
        origin: {
          type: 'runtime_task',
          taskId: 'task-1',
          attemptId: 'attempt-1',
          batchId: 'batch-1',
          executorId: 'codex',
          agentName: 'Alex',
        },
      },
      content: { markdown: '# Result' },
    })).not.toThrow();
    expect(() => assertMarkdownDocumentContent({ markdown: '  ' })).toThrow('/markdown');
  });

  it('validates content against its declared kind', () => {
    expect(() => validateEditableDocument({
      manifest: {
        schemaVersion: 1,
        id: 'doc-3',
        kind: 'markdown',
        title: 'Wrong content',
        status: 'ready',
        revision: 1,
        updatedAt: new Date(0).toISOString(),
        qaStatus: 'passed',
      },
      content: { html },
    })).toThrow('/markdown');
  });
});
