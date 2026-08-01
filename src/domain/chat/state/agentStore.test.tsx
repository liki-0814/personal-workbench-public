import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ChatMessage } from '../types';
import { useAgentChat } from './agentStore';

describe('useAgentChat', () => {
  let container: HTMLDivElement;
  let root: Root;
  let current: ReturnType<typeof useAgentChat>;

  function Harness() {
    current = useAgentChat();
    return null;
  }

  beforeEach(async () => {
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
    await act(async () => root.render(<Harness />));
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('finishes immediately when done arrives even if the SSE connection stays open', async () => {
    let streamCancelled = false;
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(new TextEncoder().encode(
          'event: done\ndata: {"prompt_tokens":12,"completion_tokens":3,"total_tokens":15}\n\n',
        ));
      },
      cancel() {
        streamCancelled = true;
      },
    });
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(body, { status: 200 })));
    const onDone = vi.fn();

    await act(async () => {
      await current.streamMessage([] as ChatMessage[], {
        sessionId: 'session-1',
        onDone,
      });
    });

    expect(onDone).toHaveBeenCalledWith({
      promptTokens: 12,
      completionTokens: 3,
      totalTokens: 15,
    });
    expect(streamCancelled).toBe(true);
    expect(current.isLoading).toBe(false);
  });
});
