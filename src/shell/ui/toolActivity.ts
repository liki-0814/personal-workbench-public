export type ToolActivityStatus = 'running' | 'completed' | 'backgrounded';

export interface ToolActivityItem {
  id: string;
  label: string;
  kind?: string;
  status: ToolActivityStatus;
}

export function getToolActivitySummary(items: ToolActivityItem[]) {
  const completed = items.filter(item => item.status === 'completed').length;
  const active = [...items].reverse().find(item => item.status !== 'completed');
  return {
    label: active?.label ?? '工具执行记录',
    kind: active?.kind,
    meta: active
      ? `${active.status === 'backgrounded' ? '后台运行' : '执行中'} · ${completed}/${items.length} 步`
      : `已完成 · ${completed} 步`,
    status: active?.status ?? 'completed',
  };
}
