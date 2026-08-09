import { describe, expect, it } from 'vitest';
import type { ChatMessage, TimelineTextItem } from '../types';
import {
  getMessageContent,
  getMessageDecisionTrace,
  getMessageTimeline,
  prepareMessageForRegeneration,
} from './store';

function textItem(id: string, text: string): TimelineTextItem {
  return { kind: 'text', id, text, startedAt: 1, endedAt: 2 };
}

describe('message timeline versions', () => {
  it('archives the current timeline and clears turn-local state before regeneration', () => {
    const timeline = [textItem('old', '旧回答')];
    const message: ChatMessage = {
      role: 'assistant',
      content: '旧回答',
      timeline,
      thinking: '旧思考',
      progressText: ['旧进度'],
      toolTrace: [{ id: 'tool-1', name: 'read', args: '', status: 'done' }],
      decisionTrace: [{ id: 'review-old', trigger: 'agentrequest', risk: 'low', status: 'resolved', advisors: [] }],
      error: 'old error',
    };

    const reset = prepareMessageForRegeneration(message, 42);
    expect(reset.content).toBe('');
    expect(reset.timeline).toBeUndefined();
    expect(reset.thinking).toBeUndefined();
    expect(reset.progressText).toBeUndefined();
    expect(reset.toolTrace).toBeUndefined();
    expect(reset.error).toBeUndefined();
    expect(reset.versions?.[0]).toMatchObject({
      content: '旧回答',
      timeline,
      decisionTrace: message.decisionTrace,
      timestamp: 42,
    });
  });

  it('selects content and timeline from the same active version', () => {
    const oldTimeline = [textItem('old', '旧回答')];
    const latestTimeline = [textItem('latest', '新回答')];
    const message: ChatMessage = {
      role: 'assistant',
      content: '新回答',
      timeline: latestTimeline,
      decisionTrace: [{ id: 'latest-review', trigger: 'agentrequest', risk: 'low', status: 'resolved', advisors: [] }],
      activeVersion: 0,
      versions: [{
        content: '旧回答',
        timeline: oldTimeline,
        decisionTrace: [{ id: 'old-review', trigger: 'agentrequest', risk: 'low', status: 'resolved', advisors: [] }],
        timestamp: 1,
      }],
    };

    expect(getMessageContent(message)).toBe('旧回答');
    expect(getMessageTimeline(message)).toBe(oldTimeline);
    expect(getMessageDecisionTrace(message)?.[0].id).toBe('old-review');
    expect(getMessageTimeline({ ...message, activeVersion: -1 })).toBe(latestTimeline);
    expect(getMessageDecisionTrace({ ...message, activeVersion: -1 })?.[0].id).toBe('latest-review');
  });
});
