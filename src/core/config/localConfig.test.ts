import { describe, expect, it } from 'vitest';
import { DEFAULT_FS_BASE, normalizeFsBase } from './localConfig';

describe('normalizeFsBase', () => {
  it('将旧的空值规范化为可见且可持久化的默认目录', () => {
    expect(DEFAULT_FS_BASE).toBe('~/');
    expect(normalizeFsBase()).toBe('~/');
    expect(normalizeFsBase('   ')).toBe('~/');
  });

  it('保留并清理用户配置的目录', () => {
    expect(normalizeFsBase('  /workspace/projects  ')).toBe('/workspace/projects');
  });
});
