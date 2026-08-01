import { describe, expect, it } from 'vitest';
import { getToolActivitySummary, type ToolActivityItem } from './toolActivity';

describe('getToolActivitySummary', () => {
  it('shows the latest running action and completed count', () => {
    const items: ToolActivityItem[] = [
      { id: '1', label: 'Read a.ts', kind: 'read', status: 'completed' },
      { id: '2', label: 'Search routes', kind: 'search', status: 'running' },
    ];
    expect(getToolActivitySummary(items)).toMatchObject({
      label: 'Search routes',
      meta: '执行中 · 1/2 步',
      status: 'running',
    });
  });

  it('collapses completed activity into one final summary', () => {
    const items: ToolActivityItem[] = [
      { id: '1', label: 'Read a.ts', status: 'completed' },
      { id: '2', label: 'Run tests', status: 'completed' },
    ];
    expect(getToolActivitySummary(items)).toMatchObject({
      label: '工具执行记录',
      meta: '已完成 · 2 步',
      status: 'completed',
    });
  });
});
