import { describe, it, expect } from 'vitest';
import { extractVariables, substitute, hasVariables } from '@/core/utils/variables';

describe('extractVariables', () => {
  it('returns empty for plain text', () => {
    expect(extractVariables('hello world')).toEqual([]);
  });

  it('extracts single variable', () => {
    expect(extractVariables('hi {{name}}')).toEqual(['name']);
  });

  it('trims whitespace inside braces', () => {
    expect(extractVariables('hi {{ name }}!')).toEqual(['name']);
  });

  it('dedupes repeated variables, preserves first-seen order', () => {
    expect(extractVariables('{{a}} {{b}} {{a}} {{c}} {{b}}')).toEqual(['a', 'b', 'c']);
  });

  it('supports CJK variable names', () => {
    expect(extractVariables('翻译为 {{目标语言}}：{{原文}}')).toEqual(['目标语言', '原文']);
  });

  it('ignores empty braces', () => {
    expect(extractVariables('{{}} {{ }} {{x}}')).toEqual(['x']);
  });

  it('ignores nested braces (cannot contain { or })', () => {
    expect(extractVariables('{{a{b}}')).toEqual([]);
  });
});

describe('substitute', () => {
  it('replaces matched variables', () => {
    expect(substitute('hi {{name}}', { name: 'Liki' })).toBe('hi Liki');
  });

  it('keeps original placeholder when value is missing', () => {
    expect(substitute('{{a}} {{b}}', { a: '1' })).toBe('1 {{b}}');
  });

  it('keeps original placeholder when value is empty string', () => {
    expect(substitute('{{a}} {{b}}', { a: '1', b: '' })).toBe('1 {{b}}');
  });

  it('handles trimmed names', () => {
    expect(substitute('{{ x }}', { x: 'OK' })).toBe('OK');
  });

  it('replaces all occurrences of same variable', () => {
    expect(substitute('{{x}} and {{x}}', { x: 'A' })).toBe('A and A');
  });
});

describe('hasVariables', () => {
  it('detects variables', () => {
    expect(hasVariables('plain text')).toBe(false);
    expect(hasVariables('hi {{name}}')).toBe(true);
  });
});
