import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ChatMessage } from '../../types';
import MessageBubble from './MessageBubble';

describe('MessageBubble process disclosure', () => {
  let container: HTMLDivElement | null = null;

  afterEach(() => {
    container?.remove();
    container = null;
  });

  it('keeps process open while running and collapses it when the final answer arrives', async () => {
    container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    const baseProps = {
      isLastAssistant: true,
      expanded: false,
      onToggleExpand: vi.fn(),
      copied: false,
      onCopy: vi.fn(),
    };
    const running: ChatMessage = {
      id: 'assistant-1',
      role: 'assistant',
      content: '',
      timeline: [
        { kind: 'text', id: 'narration', text: '正在检查代码', phase: 'narration', startedAt: 1, endedAt: 2 },
        { kind: 'text', id: 'draft', text: '候选答案', phase: 'draft', startedAt: 3 },
      ],
      decisionTrace: [{
        id: 'review-1',
        trigger: 'agentrequest',
        risk: 'low',
        status: 'reviewing',
        advisors: [{ model: 'advisor', status: 'running', round: 1 }],
      }],
    };

    await act(async () => root.render(<MessageBubble {...baseProps} msg={running} loading />));
    expect(container.querySelector('button[aria-expanded="true"]')).not.toBeNull();
    expect(container.textContent).toContain('正在检查代码');
    expect(container.textContent).toContain('决策复核');

    const judging: ChatMessage = {
      ...running,
      decisionTrace: [{
        ...running.decisionTrace![0],
        advisors: [{ model: 'advisor', status: 'done', round: 1, summary: '建议通过' }],
      }],
    };
    await act(async () => root.render(<MessageBubble {...baseProps} msg={judging} loading />));
    expect(container.textContent).toContain('裁决汇总中');
    expect(container.textContent).toContain('顾问审阅完成，正在汇总裁决');

    const completed: ChatMessage = {
      ...running,
      content: '这是最终答案',
      timeline: [
        running.timeline![0],
        { kind: 'text', id: 'final', text: '这是最终答案', phase: 'final', startedAt: 4, endedAt: 5 },
      ],
      decisionTrace: [{
        ...running.decisionTrace![0],
        status: 'resolved',
        outcome: 'proceed',
        advisors: [{ model: 'advisor', status: 'done', round: 1, summary: '建议通过' }],
      }],
    };
    await act(async () => root.render(<MessageBubble {...baseProps} msg={completed} loading={false} />));
    await act(async () => {});

    expect(container.querySelector('button[aria-expanded="false"]')).not.toBeNull();
    expect(container.textContent).toContain('这是最终答案');
    expect(container.textContent).not.toContain('正在检查代码');
    expect(container.textContent).not.toContain('建议通过');

    await act(async () => root.unmount());
  });
});
