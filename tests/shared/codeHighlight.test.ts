import { describe, expect, it } from 'vitest';
import { highlightCode, normalizeCodeLanguage } from '../../src/shell/ui/codeHighlight';

describe('code highlighting', () => {
  it('normalizes common fenced-code aliases', () => {
    expect(normalizeCodeLanguage('sh')).toBe('bash');
    expect(normalizeCodeLanguage('ts')).toBe('typescript');
    expect(normalizeCodeLanguage('c++')).toBe('cpp');
  });

  it('highlights an explicitly named language', () => {
    const result = highlightCode('SELECT total FROM report', 'sql');
    expect(result.language).toBe('sql');
    expect(result.html).toContain('hljs-keyword');
  });

  it('auto-detects unlabeled code and safely escapes markup', () => {
    const result = highlightCode('<script>alert("x")</script>');
    expect(result.html).not.toContain('<script>');
    expect(result.html).toContain('&lt;');
  });
});
