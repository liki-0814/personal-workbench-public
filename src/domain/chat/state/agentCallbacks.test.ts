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

describe('assistant segment projection', () => {
  it('moves tool-round narration into progress and keeps only the final segment as content', () => {
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
    expect(messages[0].content).toBe('');
    expect(messages[0].progressText).toEqual(['我先检查仓库。']);

    callbacks.onAssistantSegmentStart?.(2);
    callbacks.onDelta?.('问题来自事件边界缺失。');
    callbacks.onAssistantSegmentEnd?.(2, false);
    expect(messages[0].content).toBe('问题来自事件边界缺失。');
    expect(messages[0].progressText).toEqual(['我先检查仓库。']);
  });
});
