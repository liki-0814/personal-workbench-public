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

export type ToolClass = 'read' | 'search' | 'write' | 'exec' | 'other';

const CLASS_KEYWORDS: Array<[ToolClass, string[]]> = [
  ['read', ['read', 'view', 'cat', 'open']],
  ['search', ['grep', 'search', 'glob', 'list', 'find']],
  ['write', ['write', 'edit', 'patch', 'create', 'delete']],
  ['exec', ['bash', 'exec', 'run', 'shell']],
];

export function classifyTool(name: string): ToolClass {
  const n = name.toLowerCase();
  for (const [cls, keys] of CLASS_KEYWORDS) {
    if (keys.some(k => n.includes(k))) return cls;
  }
  return 'other';
}

const CLASS_UNITS: Array<[ToolClass, string]> = [
  ['read', '次阅读'],
  ['search', '次检索'],
  ['write', '次修改'],
  ['exec', '次执行'],
  ['other', '次其他操作'],
];

/** 「2次阅读 3次检索」；计数为 0 的类别不显示 */
export function getToolPhaseSummary(toolNames: string[]): string {
  const counts = new Map<ToolClass, number>();
  for (const name of toolNames) {
    const cls = classifyTool(name);
    counts.set(cls, (counts.get(cls) ?? 0) + 1);
  }
  return CLASS_UNITS
    .filter(([cls]) => (counts.get(cls) ?? 0) > 0)
    .map(([cls, unit]) => `${counts.get(cls)}${unit}`)
    .join(' ');
}
