import { describe, expect, it } from 'vitest';
import { parseUnifiedPatch } from '@/core/utils/unifiedDiff';

describe('parseUnifiedPatch', () => {
  it('tracks old and new line numbers across a hunk', () => {
    const lines = parseUnifiedPatch('@@ -4,2 +4,2 @@\n-old\n+new\n same');
    expect(lines.map(line => [line.kind, line.oldLine, line.newLine])).toEqual([
      ['hunk', undefined, undefined],
      ['delete', 4, undefined],
      ['add', undefined, 4],
      ['context', 5, 5],
    ]);
  });

  it('keeps diff headers out of addition and deletion counts', () => {
    const lines = parseUnifiedPatch('--- a/file\n+++ b/file');
    expect(lines.every(line => line.kind === 'meta')).toBe(true);
  });

  it('resets line numbers for each hunk', () => {
    const lines = parseUnifiedPatch('@@ -2,1 +4,1 @@\n-old\n+new\n@@ -10,1 +20,1 @@\n-before\n+after');
    expect(lines.map(line => [line.kind, line.oldLine, line.newLine])).toEqual([
      ['hunk', undefined, undefined],
      ['delete', 2, undefined],
      ['add', undefined, 4],
      ['hunk', undefined, undefined],
      ['delete', 10, undefined],
      ['add', undefined, 20],
    ]);
  });
});
