import type {
  ChatMessage,
  FailureEnvelope,
  TimelineItem,
  TimelineThinkingItem,
  TimelineToolGroupItem,
  TimelineToolItem,
} from '../types';

/** 时间线工具子项截断上限（与旧 toolTrace 规格一致，控制持久化体积）。 */
export const TIMELINE_ARG_LIMIT = 500;
export const TIMELINE_RESULT_LIMIT = 1000;
export const TIMELINE_PROGRESS_LIMIT = 200;

export type TimelineEvent =
  | { type: 'segment_start'; round: number }
  | { type: 'segment_end'; round: number; hasToolCalls: boolean }
  | { type: 'segment_classified'; round: number; kind: 'narration' | 'candidate' | 'final' }
  | { type: 'candidate_disposition'; round: number; disposition: 'promoted' | 'discarded' | 'superseded' }
  | { type: 'thinking_delta'; delta: string }
  | { type: 'text_delta'; delta: string; round?: number }
  | { type: 'tool_call_start'; id: string; name: string }
  | { type: 'tool_call_args_delta'; id: string; delta: string }
  | { type: 'tool_result'; id: string; output: string; isError: boolean; failure?: FailureEnvelope }
  | { type: 'tool_recovery'; id: string; phase: string; failure: FailureEnvelope }
  | { type: 'tool_progress'; id: string; line: string }
  | { type: 'stream_reset' }
  | { type: 'decision_started'; id: string; trigger: string }
  | { type: 'decision_resolved'; id: string; outcome: string }
  | { type: 'error' }
  | { type: 'done' };

let idSeq = 0;
function nextId(prefix: string): string {
  idSeq += 1;
  return `${prefix}-${idSeq.toString(36)}-${Date.now().toString(36)}`;
}

function truncField(s: string, max: number): string {
  return s.length > max ? `${s.slice(0, max)}…[共 ${s.length} 字符]` : s;
}

// ---------- 开放状态判定 ----------

function isTextOpen(item: Extract<TimelineItem, { kind: 'text' }>): boolean {
  return item.endedAt === undefined;
}
function isThinkingOpen(item: Extract<TimelineItem, { kind: 'thinking' }>): boolean {
  return item.endedAt === undefined;
}
function isGroupOpen(item: TimelineToolGroupItem): boolean {
  return item.endedAt === undefined;
}

// ---------- 结构性更新辅助 ----------

function replaceAt<T>(list: T[], index: number, value: T): T[] {
  const next = [...list];
  next[index] = value;
  return next;
}

function findOpenGroup(items: TimelineItem[]): { index: number; group: TimelineToolGroupItem } | null {
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i];
    if (it.kind === 'tool_group' && isGroupOpen(it)) return { index: i, group: it };
  }
  return null;
}

function closeOpenText(items: TimelineItem[], now: number): TimelineItem[] {
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i];
    if (it.kind === 'text' && isTextOpen(it)) return replaceAt(items, i, { ...it, endedAt: now });
  }
  return items;
}

function doneThinking(t: TimelineThinkingItem, now: number): TimelineThinkingItem {
  return isThinkingOpen(t) ? { ...t, status: 'done', endedAt: now } : t;
}

/** 关闭顶层与开放组内各自的开放思考段（同一时刻至多一个开放）。 */
function closeOpenThinking(items: TimelineItem[], now: number): TimelineItem[] {
  return items.map(it => {
    if (it.kind === 'thinking') return doneThinking(it, now);
    if (it.kind === 'tool_group' && isGroupOpen(it)) {
      return { ...it, children: it.children.map(c => c.kind === 'thinking' ? doneThinking(c, now) : c) };
    }
    return it;
  });
}

function closeOpenGroup(items: TimelineItem[], now: number): TimelineItem[] {
  const found = findOpenGroup(items);
  if (found) return replaceAt(items, found.index, { ...found.group, endedAt: now });
  return items;
}

function appendThinking(
  list: TimelineItem[] | TimelineToolGroupItem['children'],
  delta: string,
  now: number,
): TimelineItem[] {
  const arr = list as TimelineItem[];
  const last = arr[arr.length - 1];
  if (last && last.kind === 'thinking' && isThinkingOpen(last)) {
    return replaceAt(arr, arr.length - 1, { ...last, text: last.text + delta });
  }
  return [
    ...arr,
    { kind: 'thinking' as const, id: nextId('th'), text: delta, status: 'running' as const, startedAt: now },
  ];
}

function appendText(items: TimelineItem[], delta: string, now: number, round?: number): TimelineItem[] {
  const last = items[items.length - 1];
  if (last && last.kind === 'text' && isTextOpen(last)) {
    return replaceAt(items, items.length - 1, { ...last, text: last.text + delta });
  }
  return [...items, { kind: 'text' as const, id: nextId('tx'), text: delta, phase: 'draft', round, startedAt: now }];
}

function updateLatestDraft(items: TimelineItem[], fn: (item: Extract<TimelineItem, { kind: 'text' }>) => Extract<TimelineItem, { kind: 'text' }>, round?: number): TimelineItem[] {
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i];
    if (item.kind === 'text' && (item.phase === 'draft' || item.phase === undefined) && (round === undefined || item.round === round)) {
      return replaceAt(items, i, fn(item));
    }
  }
  return items;
}

function committedContent(items: TimelineItem[]): string {
  return items
    .filter((item): item is Extract<TimelineItem, { kind: 'text' }> => item.kind === 'text')
    .filter(item => item.phase === 'final' || item.phase === undefined)
    .map(item => item.text)
    .join('');
}

function hasTool(items: TimelineItem[], id: string): boolean {
  return items.some(
    it => it.kind === 'tool_group' && it.children.some(c => c.kind === 'tool' && c.id === id),
  );
}

function updateTool(
  items: TimelineItem[],
  id: string,
  fn: (tool: TimelineToolItem) => TimelineToolItem,
): TimelineItem[] {
  for (let i = 0; i < items.length; i++) {
    const it = items[i];
    if (it.kind !== 'tool_group') continue;
    const ci = it.children.findIndex(c => c.kind === 'tool' && c.id === id);
    if (ci === -1) continue;
    const children = replaceAt(it.children, ci, fn(it.children[ci] as TimelineToolItem));
    return replaceAt(items, i, { ...it, children });
  }
  return items;
}

/** done/error 时关闭全部开放项；仍 running 的工具按 toolStatus 收尾。 */
function closeAll(items: TimelineItem[], now: number, toolStatus: 'done' | 'error'): TimelineItem[] {
  return items.map(it => {
    if (it.kind === 'thinking' && isThinkingOpen(it)) return { ...it, status: 'done' as const, endedAt: now };
    if (it.kind === 'text' && isTextOpen(it)) return { ...it, endedAt: now };
    if (it.kind === 'tool_group') {
      return {
        ...it,
        endedAt: now,
        children: it.children.map(c =>
          c.kind === 'tool' && (c.status === 'running' || c.status === 'recovering')
            ? { ...c, status: toolStatus, endedAt: c.endedAt ?? now }
            : c,
        ),
      };
    }
    return it;
  });
}

// ---------- 主 reducer ----------

/**
 * 时间线单一事实源：把流事件归约为有序 TimelineItem 列表。
 * 关键边界：组只被 text_delta / done / error 关闭（下一轮思考自然嵌套进上一组）；
 * text 关闭其前一切（正文一出，组折叠、思考收尾）。
 * 兼容字段 content 只同步已提交的最终正文，过程旁白和被否决草稿不再混入下一轮上下文。
 */
export function reduceTimeline(msg: ChatMessage, ev: TimelineEvent): ChatMessage {
  const now = Date.now();
  let items = msg.timeline ?? [];

  switch (ev.type) {
    case 'segment_start':
      items = closeOpenText(items, now);
      break;
    case 'segment_end':
      items = closeOpenText(items, now);
      items = updateLatestDraft(items, item => ({ ...item, round: ev.round }), ev.round);
      break;
    case 'segment_classified':
      items = updateLatestDraft(items, item => ({
        ...item,
        phase: ev.kind === 'narration' ? 'narration' : ev.kind === 'final' ? 'final' : 'draft',
        round: ev.round,
      }), ev.round);
      break;
    case 'candidate_disposition':
      items = updateLatestDraft(items, item => ({
        ...item,
        phase: ev.disposition === 'promoted' ? 'final' : 'discarded',
        round: ev.round,
      }), ev.round);
      break;
    case 'thinking_delta': {
      items = closeOpenText(items, now);
      const found = findOpenGroup(items);
      if (found) {
        items = replaceAt(items, found.index, {
          ...found.group,
          children: appendThinking(found.group.children, ev.delta, now) as TimelineToolGroupItem['children'],
        });
      } else {
        items = appendThinking(items, ev.delta, now);
      }
      break;
    }
    case 'text_delta':
      items = closeOpenThinking(items, now);
      items = closeOpenGroup(items, now);
      items = appendText(items, ev.delta, now, ev.round);
      break;
    case 'tool_call_start': {
      if (hasTool(items, ev.id)) break;
      items = closeOpenThinking(items, now);
      items = closeOpenText(items, now);
      let found = findOpenGroup(items);
      if (!found) {
        const group: TimelineToolGroupItem = { kind: 'tool_group', id: nextId('tg'), children: [], startedAt: now };
        items = [...items, group];
        found = { index: items.length - 1, group };
      }
      const tool: TimelineToolItem = {
        kind: 'tool',
        id: ev.id,
        name: ev.name,
        args: '',
        status: 'running',
        startedAt: now,
      };
      items = replaceAt(items, found.index, { ...found.group, children: [...found.group.children, tool] });
      break;
    }
    case 'tool_call_args_delta':
      items = updateTool(items, ev.id, t => ({ ...t, args: truncField(t.args + ev.delta, TIMELINE_ARG_LIMIT) }));
      break;
    case 'tool_result':
      items = updateTool(items, ev.id, t => ({
        ...t,
        result: truncField(ev.output, TIMELINE_RESULT_LIMIT),
        isError: ev.isError,
        failure: ev.failure,
        recoveryPhase: undefined,
        status: ev.isError ? 'error' : 'done',
        endedAt: now,
      }));
      break;
    case 'tool_recovery':
      items = updateTool(items, ev.id, t => ({
        ...t,
        failure: ev.failure,
        recoveryPhase: ev.phase,
        status: ev.phase === 'auto_retrying' ? 'recovering' : t.status,
      }));
      break;
    case 'tool_progress':
      items = updateTool(items, ev.id, t => {
        const log = [...(t.progressLog ?? []), ev.line];
        return {
          ...t,
          progressLog: log.length > TIMELINE_PROGRESS_LIMIT
            ? log.slice(log.length - TIMELINE_PROGRESS_LIMIT)
            : log,
        };
      });
      break;
    case 'stream_reset':
      return { ...msg, timeline: [], content: '' };
    case 'decision_started':
      if (ev.trigger === 'finalreview' || ev.trigger === 'final_review') {
        items = closeOpenText(items, now);
      }
      break;
    case 'decision_resolved':
      if (ev.outcome === 'proceed') {
        items = updateLatestDraft(items, item => ({ ...item, phase: 'final' }));
      } else if (ev.outcome === 'revise' || ev.outcome === 'gather_evidence' || ev.outcome === 'escalate') {
        items = updateLatestDraft(items, item => ({ ...item, phase: 'discarded' }));
      }
      break;
    case 'error':
      items = closeAll(items, now, 'error');
      break;
    case 'done':
      items = closeAll(items, now, 'done');
      items = updateLatestDraft(items, item => ({ ...item, phase: 'final' }));
      break;
  }

  return { ...msg, timeline: items, content: committedContent(items) };
}

// ---------- 渲染层查询辅助 ----------

/** 提取时间线内全部工具子项（跨组），供空态/后台态派生检查使用。 */
export function getTimelineToolItems(items?: TimelineItem[]): TimelineToolItem[] {
  if (!items) return [];
  const out: TimelineToolItem[] = [];
  for (const it of items) {
    if (it.kind === 'tool_group') {
      for (const c of it.children) if (c.kind === 'tool') out.push(c);
    }
  }
  return out;
}

/** 时间线是否已出现思考段或正文（用于等待指示器抑制）。 */
export function timelineHasThinkingOrText(items?: TimelineItem[]): boolean {
  if (!items) return false;
  return items.some(
    it =>
      it.kind === 'text' ||
      it.kind === 'thinking' ||
      (it.kind === 'tool_group' && it.children.some(c => c.kind === 'thinking')),
  );
}

/** 回合结束时仍 running/recovering 的工具标记为 backgrounded（组同时关闭）。 */
export function backgroundOpenTools(msg: ChatMessage): ChatMessage {
  if (!msg.timeline?.some(
    it => it.kind === 'tool_group' &&
      it.children.some(c => c.kind === 'tool' && (c.status === 'running' || c.status === 'recovering')),
  )) return msg;
  const now = Date.now();
  return {
    ...msg,
    timeline: msg.timeline.map(it => it.kind === 'tool_group' ? {
      ...it,
      endedAt: it.endedAt ?? now,
      children: it.children.map(c =>
        c.kind === 'tool' && (c.status === 'running' || c.status === 'recovering')
          ? { ...c, status: 'backgrounded' as const, endedAt: c.endedAt ?? now }
          : c,
      ),
    } : it),
  };
}
