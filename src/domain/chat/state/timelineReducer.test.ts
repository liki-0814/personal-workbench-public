import { describe, expect, it } from 'vitest';
import type { ChatMessage, TimelineToolGroupItem, TimelineToolItem } from '../types';
import {
  reduceTimeline,
  getTimelineToolItems,
  timelineHasThinkingOrText,
  TIMELINE_ARG_LIMIT,
  TIMELINE_RESULT_LIMIT,
  TIMELINE_PROGRESS_LIMIT,
} from './timelineReducer';

function base(): ChatMessage {
  return { role: 'assistant', content: '' };
}

function groups(msg: ChatMessage): TimelineToolGroupItem[] {
  return (msg.timeline ?? []).filter((i): i is TimelineToolGroupItem => i.kind === 'tool_group');
}

function tools(msg: ChatMessage): TimelineToolItem[] {
  return getTimelineToolItems(msg.timeline);
}

describe('ordering & nesting', () => {
  it('counts thinking from model-call start before the first summary delta', () => {
    let msg = base();
    msg = reduceTimeline(msg, { type: 'thinking_start' });
    const started = (msg.timeline ?? [])[0];
    expect(started.kind === 'thinking' && started.text).toBe('');
    expect(started.kind === 'thinking' && started.status).toBe('running');
    const startedAt = started.startedAt;

    msg = reduceTimeline(msg, { type: 'thinking_delta', delta: 'summary' });
    const updated = (msg.timeline ?? [])[0];
    expect(updated.kind === 'thinking' && updated.text).toBe('summary');
    expect(updated.startedAt).toBe(startedAt);
  });

  it('interleaves thinking / text / tool groups in arrival order and nests thinking inside open groups', () => {
    let msg = base();
    msg = reduceTimeline(msg, { type: 'segment_start', round: 1 });
    msg = reduceTimeline(msg, { type: 'thinking_delta', delta: 'plan' });
    msg = reduceTimeline(msg, { type: 'text_delta', delta: '我先读源码。' });
    msg = reduceTimeline(msg, { type: 'tool_call_start', id: 'c1', name: 'read_file' });
    msg = reduceTimeline(msg, { type: 'tool_call_start', id: 'c2', name: 'grep_code' });
    msg = reduceTimeline(msg, { type: 'tool_result', id: 'c1', output: 'ok', isError: false });
    msg = reduceTimeline(msg, { type: 'tool_result', id: 'c2', output: '2 hits', isError: false });
    msg = reduceTimeline(msg, { type: 'segment_start', round: 2 });
    // 下一轮思考到达时组仍开放 → 嵌套进上一组
    msg = reduceTimeline(msg, { type: 'thinking_delta', delta: 'deep' });
    msg = reduceTimeline(msg, { type: 'text_delta', delta: '结论如下。' });

    const kinds = (msg.timeline ?? []).map(i => i.kind);
    expect(kinds).toEqual(['thinking', 'text', 'tool_group', 'text']);

    const [thinking, text1, group, text2] = msg.timeline ?? [];
    expect(thinking.kind === 'thinking' && thinking.status).toBe('done');
    expect(text1.kind === 'text' && text1.text).toBe('我先读源码。');
    expect(group.kind === 'tool_group' && group.endedAt).toBeDefined();
    expect(text2.kind === 'text' && text2.text).toBe('结论如下。');

    const children = (group as TimelineToolGroupItem).children.map(c => c.kind);
    expect(children).toEqual(['tool', 'tool', 'thinking']);
    const nested = (group as TimelineToolGroupItem).children[2];
    expect(nested.kind === 'thinking' && nested.text).toBe('deep');
    expect(nested.kind === 'thinking' && nested.status).toBe('done');
  });

  it('keeps top-level thinking when no group is open', () => {
    let msg = base();
    msg = reduceTimeline(msg, { type: 'thinking_delta', delta: 'a' });
    msg = reduceTimeline(msg, { type: 'thinking_delta', delta: 'b' });
    expect((msg.timeline ?? []).map(i => i.kind)).toEqual(['thinking']);
    const item = (msg.timeline ?? [])[0];
    expect(item.kind === 'thinking' && item.text).toBe('ab');
    expect(item.kind === 'thinking' && item.status).toBe('running');
  });
});

describe('boundary rules', () => {
  it('text_delta closes open thinking and open group', () => {
    let msg = base();
    msg = reduceTimeline(msg, { type: 'thinking_delta', delta: 't' });
    msg = reduceTimeline(msg, { type: 'tool_call_start', id: 'c1', name: 'read_file' });
    msg = reduceTimeline(msg, { type: 'text_delta', delta: 'answer' });

    const thinking = (msg.timeline ?? [])[0];
    expect(thinking.kind === 'thinking' && thinking.status).toBe('done');
    expect(thinking.kind === 'thinking' && thinking.endedAt).toBeDefined();
    const group = groups(msg)[0];
    expect(group.endedAt).toBeDefined();
  });

  it('tool_result does NOT close the group; only text/done/error do', () => {
    let msg = base();
    msg = reduceTimeline(msg, { type: 'tool_call_start', id: 'c1', name: 'read_file' });
    msg = reduceTimeline(msg, { type: 'tool_result', id: 'c1', output: 'ok', isError: false });
    expect(groups(msg)[0].endedAt).toBeUndefined();

    msg = reduceTimeline(msg, { type: 'done' });
    expect(groups(msg)[0].endedAt).toBeDefined();
  });

  it('segment_start closes open text so consecutive rounds become separate paragraphs', () => {
    let msg = base();
    msg = reduceTimeline(msg, { type: 'text_delta', delta: 'round1' });
    msg = reduceTimeline(msg, { type: 'segment_start', round: 2 });
    msg = reduceTimeline(msg, { type: 'text_delta', delta: 'round2' });
    const texts = (msg.timeline ?? []).filter(i => i.kind === 'text');
    expect(texts).toHaveLength(2);
    expect(texts.map(t => (t.kind === 'text' ? t.text : ''))).toEqual(['round1', 'round2']);
  });

  it('done closes everything: thinking done, group ended, running tool done', () => {
    let msg = base();
    msg = reduceTimeline(msg, { type: 'thinking_delta', delta: 't' });
    msg = reduceTimeline(msg, { type: 'tool_call_start', id: 'c1', name: 'bash' });
    msg = reduceTimeline(msg, { type: 'done' });

    const thinking = (msg.timeline ?? [])[0];
    expect(thinking.kind === 'thinking' && thinking.status).toBe('done');
    const group = groups(msg)[0];
    expect(group.endedAt).toBeDefined();
    expect(tools(msg)[0].status).toBe('done');
  });

  it('error marks still-running tools as error', () => {
    let msg = base();
    msg = reduceTimeline(msg, { type: 'tool_call_start', id: 'c1', name: 'bash' });
    msg = reduceTimeline(msg, { type: 'error' });
    expect(tools(msg)[0].status).toBe('error');
    expect(groups(msg)[0].endedAt).toBeDefined();
  });

  it('stream_reset clears timeline and content', () => {
    let msg = base();
    msg = reduceTimeline(msg, { type: 'text_delta', delta: 'x' });
    msg = reduceTimeline(msg, { type: 'stream_reset' });
    expect(msg.timeline).toEqual([]);
    expect(msg.content).toBe('');
  });

  it('dedupes tool_call_start with an existing id', () => {
    let msg = base();
    msg = reduceTimeline(msg, { type: 'tool_call_start', id: 'c1', name: 'read_file' });
    msg = reduceTimeline(msg, { type: 'tool_call_start', id: 'c1', name: 'read_file' });
    expect(tools(msg)).toHaveLength(1);
  });
});

describe('content compatibility', () => {
  it('uses explicit segment classification and disposition instead of tool-call inference', () => {
    let msg = base();
    msg = reduceTimeline(msg, { type: 'segment_start', round: 1 });
    msg = reduceTimeline(msg, { type: 'text_delta', delta: '先检查仓库', round: 1 });
    msg = reduceTimeline(msg, { type: 'segment_end', round: 1, hasToolCalls: false });
    msg = reduceTimeline(msg, { type: 'segment_classified', round: 1, kind: 'narration' });
    expect(msg.content).toBe('');

    msg = reduceTimeline(msg, { type: 'segment_start', round: 2 });
    msg = reduceTimeline(msg, { type: 'text_delta', delta: '候选结论', round: 2 });
    msg = reduceTimeline(msg, { type: 'segment_end', round: 2, hasToolCalls: false });
    msg = reduceTimeline(msg, { type: 'segment_classified', round: 2, kind: 'candidate' });
    expect(msg.content).toBe('');

    msg = reduceTimeline(msg, { type: 'candidate_disposition', round: 2, disposition: 'promoted' });
    expect(msg.content).toBe('候选结论');
  });

  it('does not promote an explicitly discarded candidate during finalization', () => {
    let msg = base();
    msg = reduceTimeline(msg, { type: 'text_delta', delta: '错误草稿', round: 1 });
    msg = reduceTimeline(msg, { type: 'segment_classified', round: 1, kind: 'candidate' });
    msg = reduceTimeline(msg, { type: 'candidate_disposition', round: 1, disposition: 'discarded' });
    msg = reduceTimeline(msg, { type: 'done' });
    expect(msg.content).toBe('');
  });

  it('only commits the final text item on done', () => {
    let msg = base();
    msg = reduceTimeline(msg, { type: 'thinking_delta', delta: 'hidden' });
    msg = reduceTimeline(msg, { type: 'text_delta', delta: 'para1 ' });
    msg = reduceTimeline(msg, { type: 'tool_call_start', id: 'c1', name: 'grep_code' });
    msg = reduceTimeline(msg, { type: 'text_delta', delta: 'para2' });
    expect(msg.content).toBe('');
    msg = reduceTimeline(msg, { type: 'done' });
    expect(msg.content).toBe('para2');
    // 思考绝不进入正文
    expect(msg.content).not.toContain('hidden');
  });
});

describe('truncation limits', () => {
  it('truncates args, result and progress log', () => {
    let msg = base();
    msg = reduceTimeline(msg, { type: 'tool_call_start', id: 'c1', name: 'bash' });
    msg = reduceTimeline(msg, { type: 'tool_call_args_delta', id: 'c1', delta: 'a'.repeat(TIMELINE_ARG_LIMIT + 50) });
    msg = reduceTimeline(msg, { type: 'tool_result', id: 'c1', output: 'r'.repeat(TIMELINE_RESULT_LIMIT + 50), isError: false });
    for (let i = 0; i < TIMELINE_PROGRESS_LIMIT + 10; i++) {
      msg = reduceTimeline(msg, { type: 'tool_progress', id: 'c1', line: `line ${i}` });
    }
    const tool = tools(msg)[0];
    expect(tool.args.length).toBeLessThanOrEqual(TIMELINE_ARG_LIMIT + 20);
    expect(tool.args).toContain('共');
    expect((tool.result ?? '').length).toBeLessThanOrEqual(TIMELINE_RESULT_LIMIT + 20);
    expect(tool.progressLog).toHaveLength(TIMELINE_PROGRESS_LIMIT);
    expect(tool.progressLog![0]).toBe('line 10');
  });
});

describe('query helpers', () => {
  it('timelineHasThinkingOrText detects nested thinking', () => {
    let msg = base();
    msg = reduceTimeline(msg, { type: 'tool_call_start', id: 'c1', name: 'read_file' });
    expect(timelineHasThinkingOrText(msg.timeline)).toBe(false);
    msg = reduceTimeline(msg, { type: 'thinking_delta', delta: 'x' });
    expect(timelineHasThinkingOrText(msg.timeline)).toBe(true);
  });
});
