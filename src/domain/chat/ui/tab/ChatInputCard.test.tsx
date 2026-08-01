import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import ChatInputCard from './ChatInputCard';

describe('ChatInputCard actions', () => {
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
    vi.restoreAllMocks();
  });

  it('sends an attachment-only message and suppresses rapid duplicate clicks', async () => {
    let finish!: () => void;
    const onSubmit = vi.fn(() => new Promise<void>(resolve => { finish = resolve; }));
    await act(async () => root.render(
      <ChatInputCard
        value=""
        onChange={vi.fn()}
        onSubmit={onSubmit}
        attachments={[{ type: 'file', id: 'file-1', title: 'notes.md', content: '', path: '/workspace/notes.md' }]}
      />,
    ));

    const send = container.querySelector('[aria-label="发送消息"]') as HTMLButtonElement;
    expect(send.disabled).toBe(false);
    await act(async () => {
      send.click();
      send.click();
      await Promise.resolve();
    });
    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(container.querySelector('[aria-label="正在提交消息"]')).not.toBeNull();

    await act(async () => finish());
    expect(container.querySelector('[aria-label="发送消息"]')).not.toBeNull();
  });

  it('uses Cmd+Enter for guidance while a turn is running', async () => {
    const onSubmit = vi.fn();
    const onGuide = vi.fn();
    await act(async () => root.render(
      <ChatInputCard
        value="补充这个约束"
        onChange={vi.fn()}
        onSubmit={onSubmit}
        onGuide={onGuide}
        streaming
      />,
    ));

    const textarea = container.querySelector('textarea') as HTMLTextAreaElement;
    await act(async () => {
      textarea.dispatchEvent(new KeyboardEvent('keydown', {
        key: 'Enter',
        metaKey: true,
        bubbles: true,
        cancelable: true,
      }));
    });

    expect(onGuide).toHaveBeenCalledTimes(1);
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it('does not submit Enter during IME composition', async () => {
    const onSubmit = vi.fn();
    await act(async () => root.render(
      <ChatInputCard value="正在输入" onChange={vi.fn()} onSubmit={onSubmit} />,
    ));

    const textarea = container.querySelector('textarea') as HTMLTextAreaElement;
    await act(async () => {
      textarea.dispatchEvent(new KeyboardEvent('keydown', {
        key: 'Enter',
        isComposing: true,
        bubbles: true,
        cancelable: true,
      }));
    });

    expect(onSubmit).not.toHaveBeenCalled();
  });
});
