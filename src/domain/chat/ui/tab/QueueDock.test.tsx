import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { showToast } from '@/shell';
import type { QueuedMessage, SessionRuntime } from '../../types';
import QueueDock from './QueueDock';

vi.mock('@/shell', () => ({ showToast: vi.fn() }));

const queued = (overrides: Partial<QueuedMessage> = {}): QueuedMessage => ({
  id: 'queue-user',
  clientMessageId: 'client-user',
  sequence: 1,
  delivery: 'next_turn',
  status: 'queued',
  source: 'user',
  priority: 'normal',
  message: { role: 'user', content: '普通排队消息' },
  createdAt: '2026-07-30T00:00:00Z',
  ...overrides,
});

const runtime = (queue: QueuedMessage[]): SessionRuntime => ({
  sessionId: 'session-1',
  phase: 'paused',
  activeTurnId: 'turn-1',
  paused: true,
  queue,
  lastEventSequence: 2,
});

describe('QueueDock', () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  it('renders a runtime callback as an immutable system row', async () => {
    const onUpdate = vi.fn();
    const onDelete = vi.fn();
    await act(async () => root.render(
      <QueueDock
        runtime={runtime([queued({
          id: 'queue-callback',
          clientMessageId: 'runtime-batch:1:ready',
          source: 'runtime_callback',
          priority: 'high',
          status: 'paused',
          message: { role: 'user', content: '内部 callback 原文' },
        })])}
        onUpdate={onUpdate}
        onDelete={onDelete}
        onControl={vi.fn()}
      />,
    ));

    const row = container.querySelector('[data-source="runtime_callback"]') as HTMLElement;
    expect(row.textContent).toContain('协作者结果待处理');
    expect(row.textContent).not.toContain('内部 callback 原文');
    expect(row.querySelector('button')).toBeNull();
    expect(onUpdate).not.toHaveBeenCalled();
    expect(onDelete).not.toHaveBeenCalled();
  });

  it('keeps legacy user queue items editable when source is absent', async () => {
    const onUpdate = vi.fn().mockResolvedValue(undefined);
    await act(async () => root.render(
      <QueueDock
        runtime={runtime([queued({ source: undefined, priority: undefined })])}
        onUpdate={onUpdate}
        onDelete={vi.fn()}
        onControl={vi.fn()}
      />,
    ));

    await act(async () => {
      (container.querySelector('.chat-queue-copy') as HTMLButtonElement).click();
    });
    const input = container.querySelector('.chat-queue-editor input') as HTMLInputElement;
    expect(input.value).toBe('普通排队消息');
  });

  it('wires guidance, reorder, delete and pause controls to one guarded action at a time', async () => {
    let release!: () => void;
    const onUpdate = vi.fn(() => new Promise<void>(resolve => { release = resolve; }));
    const onDelete = vi.fn().mockResolvedValue(undefined);
    const onControl = vi.fn().mockResolvedValue(undefined);
    await act(async () => root.render(
      <QueueDock
        runtime={runtime([queued(), queued({ id: 'queue-2', sequence: 2, message: { role: 'user', content: '第二条' } })])}
        onUpdate={onUpdate}
        onDelete={onDelete}
        onControl={onControl}
      />,
    ));

    const guide = container.querySelector('.chat-queue-guide') as HTMLButtonElement;
    await act(async () => {
      guide.click();
      guide.click();
      await Promise.resolve();
    });
    expect(onUpdate).toHaveBeenCalledTimes(1);
    expect(onUpdate).toHaveBeenCalledWith('queue-user', { delivery: 'guidance' });

    await act(async () => release());
    const more = container.querySelector('.chat-queue-more > button') as HTMLButtonElement;
    await act(async () => more.click());
    const down = Array.from(container.querySelectorAll<HTMLButtonElement>('.precision-popover button'))
      .find(button => button.textContent?.includes('下移'))!;
    await act(async () => down.click());
    expect(onUpdate).toHaveBeenLastCalledWith('queue-user', { position: 1 });
    await act(async () => release());

    const remove = container.querySelector('.chat-queue-delete') as HTMLButtonElement;
    await act(async () => remove.click());
    expect(onDelete).toHaveBeenCalledWith('queue-user');

    const sendNext = Array.from(container.querySelectorAll<HTMLButtonElement>('header button'))
      .find(button => button.textContent?.includes('发送下一条'))!;
    await act(async () => sendNext.click());
    expect(onControl).toHaveBeenCalledWith('send_next');
  });

  it('surfaces queue action failures instead of leaving an unhandled rejection', async () => {
    const onControl = vi.fn().mockRejectedValue(new Error('daemon unavailable'));
    await act(async () => root.render(
      <QueueDock
        runtime={runtime([queued()])}
        onUpdate={vi.fn()}
        onDelete={vi.fn()}
        onControl={onControl}
      />,
    ));

    const resume = Array.from(container.querySelectorAll<HTMLButtonElement>('header button'))
      .find(button => button.textContent?.includes('继续队列'))!;
    await act(async () => resume.click());
    expect(showToast).toHaveBeenCalledWith({ message: 'daemon unavailable', type: 'error' });
  });
});
