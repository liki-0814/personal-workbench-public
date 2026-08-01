import { describe, expect, it } from 'vitest';
import { prepareHtmlForFrame, serializeFrameDocument } from './htmlDocument';

describe('HTML frame isolation', () => {
  it('removes executable content and resolves local assets for preview', () => {
    const prepared = prepareHtmlForFrame(
      '<!doctype html><html><head><style>.hero{background:url("assets/bg.png")}</style></head><body><script>alert(1)</script><img src="assets/chart.png" onerror="alert(1)"></body></html>',
      'doc-1',
    );
    expect(prepared).not.toContain('<script');
    expect(prepared).not.toContain('onerror');
    expect(prepared).toContain('/api/document-assets/doc-1/chart.png');
    expect(prepared).toContain('/api/document-assets/doc-1/bg.png');
  });

  it('serializes visual edits back to portable asset paths', () => {
    const prepared = prepareHtmlForFrame(
      '<!doctype html><html><body><main><img src="assets/chart.png"></main></body></html>',
      'doc-1',
    );
    const parsed = new DOMParser().parseFromString(prepared, 'text/html');
    parsed.querySelector('main')?.append(parsed.createElement('p'));
    const serialized = serializeFrameDocument(parsed, 'doc-1');
    expect(serialized).toContain('src="assets/chart.png"');
    expect(serialized).not.toContain('data-pwb-editor');
    expect(serialized).not.toContain('/api/document-assets/doc-1/');
  });
});
