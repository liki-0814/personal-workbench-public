import { describe, expect, it } from 'vitest';
import type { ChatMessage, GeneratedImageRecord } from '../types';
import { buildAgentStreamCallbacks } from './agentCallbacks';

const record: GeneratedImageRecord = {
  id: 'generation-1',
  url: '/api/image-artifacts/session/generation-1',
  status: 'ready_with_warnings',
  intent: 'generate',
  scene: 'scientific',
  visualBrief: {
    intent: 'generate',
    scene: 'scientific',
    prompt: 'model diagram',
    exactText: [],
    references: [],
    invariants: [],
    exclusions: [],
    factualConstraints: [],
  },
  finalPrompt: 'compiled prompt',
  aspectRatio: '16:9',
  modelId: 'image-model',
  providerName: 'Image Provider',
  references: [],
  qa: {
    status: 'not-checked',
    issues: [{ category: 'artifact', description: '未配置 vision 模型' }],
  },
  createdAt: '2026-07-21T00:00:00Z',
};

describe('thinking event projection', () => {
  it('starts the timeline before a reasoning summary delta arrives', () => {
    let messages: ChatMessage[] = [{ role: 'assistant', content: '' }];
    const callbacks = buildAgentStreamCallbacks({
      getMessages: () => messages,
      setMessages: next => { messages = next; },
      assistantIndex: 0,
      target: 'session',
    });

    callbacks.onThinkingStart?.();
    callbacks.onThinkingDelta?.('摘要');

    const item = (messages[0].timeline ?? [])[0];
    expect(item.kind === 'thinking' && item.text).toBe('摘要');
    expect(item.kind === 'thinking' && item.status).toBe('running');
  });
});

describe('image tool projection', () => {
  it('keeps the legacy URL and the structured generation record together', async () => {
    let messages: ChatMessage[] = [{ role: 'assistant', content: '' }];
    const callbacks = buildAgentStreamCallbacks({
      getMessages: () => messages,
      setMessages: next => { messages = next; },
      assistantIndex: 0,
      target: 'session',
    });

    await callbacks.onToolImage?.('tool-1', record.url, '', record);

    expect(messages[0].generatedImages).toEqual([record.url]);
    expect(messages[0].generatedImageRecords).toEqual([record]);
  });
});

describe('document tool projection', () => {
  it('upserts a structured document reference without parsing tool text', () => {
    let messages: ChatMessage[] = [{ role: 'assistant', content: '' }];
    const callbacks = buildAgentStreamCallbacks({
      getMessages: () => messages,
      setMessages: next => { messages = next; },
      assistantIndex: 0,
      target: 'session',
    });
    callbacks.onToolDocument?.('tool-1', {
      id: 'doc-1', kind: 'html', title: 'Quarterly review', revision: 1, status: 'draft', qaStatus: 'pending',
    });
    callbacks.onToolDocument?.('tool-2', {
      id: 'doc-1', kind: 'html', title: 'Quarterly review', revision: 2, status: 'draft', qaStatus: 'passed',
    });
    expect(messages[0].documentRefs).toEqual([{
      id: 'doc-1', kind: 'html', title: 'Quarterly review', revision: 2, status: 'draft', qaStatus: 'passed',
    }]);
  });

  it('persists a structured composer decision on the assistant message', () => {
    let messages: ChatMessage[] = [{ role: 'assistant', content: '' }];
    const callbacks = buildAgentStreamCallbacks({
      getMessages: () => messages,
      setMessages: next => { messages = next; },
      assistantIndex: 0,
      target: 'session',
    });
    callbacks.onToolDecision?.('tool-1', {
      id: 'decision-1', title: '选择风格', options: [
        { id: 'clean', label: '简洁', description: '强调信息层级' },
        { id: 'visual', label: '视觉', description: '强调图片表现' },
      ], step: 1, total: 2, allowCustom: true, allowSkip: true,
    });
    expect(messages[0].decisionPrompt?.title).toBe('选择风格');
    expect(messages[0].decisionPrompt?.options).toHaveLength(2);
  });
});

describe('timeline projection', () => {
  it('shows advisor progress and replaces it with the completed summary', () => {
    let messages: ChatMessage[] = [{ role: 'assistant', content: '' }];
    const callbacks = buildAgentStreamCallbacks({
      getMessages: () => messages,
      setMessages: next => { messages = next; },
      assistantIndex: 0,
      target: 'session',
    });
    callbacks.onDecisionStarted?.({ id: 'review-1', trigger: 'finalreview', risk: 'elevated' });
    callbacks.onDecisionAdvisor?.({ id: 'review-1', model: 'openai:gpt', status: 'running', round: 1 });
    callbacks.onDecisionAdvisor?.({ id: 'review-1', model: 'openai:gpt', status: 'done', round: 1, summary: '建议通过' });
    callbacks.onDecisionAdvisor?.({ id: 'review-1', model: 'openai:gpt', status: 'running', round: 2 });
    callbacks.onDecisionAdvisor?.({ id: 'review-1', model: 'openai:gpt', status: 'done', round: 2, summary: '复议后仍建议通过' });
    expect(messages[0].decisionTrace?.[0].advisors).toEqual([
      { model: 'openai:gpt', status: 'done', round: 1, summary: '建议通过' },
      { model: 'openai:gpt', status: 'done', round: 2, summary: '复议后仍建议通过' },
    ]);
  });

  it('separates tool narration from the committed final answer', () => {
    let messages: ChatMessage[] = [{ role: 'assistant', content: '' }];
    const callbacks = buildAgentStreamCallbacks({
      getMessages: () => messages,
      setMessages: next => { messages = next; },
      assistantIndex: 0,
      target: 'session',
    });

    callbacks.onAssistantSegmentStart?.(1);
    callbacks.onDelta?.('我先检查仓库。');
    callbacks.onAssistantSegmentEnd?.(1, true);
    callbacks.onAssistantSegmentClassified?.(1, 'narration');
    callbacks.onToolCall?.({ id: 'c1', name: 'read_file' });
    callbacks.onToolResult?.({ toolCallId: 'c1', output: 'ok', isError: false });
    callbacks.onAssistantSegmentStart?.(2);
    callbacks.onDelta?.('问题来自事件边界缺失。');
    callbacks.onAssistantSegmentEnd?.(2, false);
    callbacks.onAssistantSegmentClassified?.(2, 'candidate');
    callbacks.onCandidateDisposition?.(2, 'promoted');
    callbacks.finalizeTimeline();

    expect(messages[0].content).toBe('问题来自事件边界缺失。');
    expect(messages[0].progressText).toBeUndefined();

    const kinds = (messages[0].timeline ?? []).map(i => i.kind);
    expect(kinds).toEqual(['text', 'tool_group', 'text']);
    const group = (messages[0].timeline ?? [])[1];
    expect(group.kind === 'tool_group' && group.endedAt).toBeDefined();
    const narration = (messages[0].timeline ?? [])[0];
    expect(narration.kind === 'text' && narration.phase).toBe('narration');
  });

  it('discards a rejected draft and commits only the revised answer', () => {
    let messages: ChatMessage[] = [{ role: 'assistant', content: '' }];
    const callbacks = buildAgentStreamCallbacks({
      getMessages: () => messages,
      setMessages: next => { messages = next; },
      assistantIndex: 0,
      target: 'session',
    });

    callbacks.onAssistantSegmentStart?.(1);
    callbacks.onDelta?.('第一版草稿');
    callbacks.onAssistantSegmentEnd?.(1, false);
    callbacks.onAssistantSegmentClassified?.(1, 'candidate');
    callbacks.onDecisionStarted?.({ id: 'review-1', trigger: 'finalreview', risk: 'elevated' });
    expect(messages[0].content).toBe('');

    callbacks.onDecisionResolved?.({
      id: 'review-1', outcome: 'revise', confidence: 0.9, consensus: 1, rationale: '需要修订',
    });
    expect(messages[0].content).toBe('');

    callbacks.onAssistantSegmentStart?.(2);
    callbacks.onDelta?.('第二版终稿');
    callbacks.onAssistantSegmentEnd?.(2, false);
    callbacks.onAssistantSegmentClassified?.(2, 'candidate');
    callbacks.onCandidateDisposition?.(2, 'promoted');
    callbacks.finalizeTimeline();
    expect(messages[0].content).toBe('第二版终稿');
    const texts = (messages[0].timeline ?? []).filter(i => i.kind === 'text');
    expect(texts).toHaveLength(2);
    expect(texts[0].kind === 'text' && texts[0].phase).toBe('discarded');
  });
});
