import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { apiFetch } from '@/core/utils';
import type { SessionRuntime } from '../types';
import { isSessionTurnRunning, useSessionRuntime } from './sessionRuntimeStore';

vi.mock('@/core/config/backendUrl', () => ({ getBackendUrl: () => 'http://daemon.test' }));
vi.mock('@/core/utils', () => ({ apiFetch: vi.fn() }));

class FakeEventSource {
  static instances: FakeEventSource[] = [];

  readonly url: string;
  onerror: (() => void) | null = null;
  closed = false;
  private readonly listeners = new Map<string, Array<(event: MessageEvent) => void>>();

  constructor(url: string) {
    this.url = url;
    FakeEventSource.instances.push(this);
  }

  addEventListener(type: string, listener: EventListenerOrEventListenerObject) {
    const callback = typeof listener === 'function'
      ? listener as (event: MessageEvent) => void
      : (event: MessageEvent) => listener.handleEvent(event);
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), callback]);
  }

  emit(type: string, data: unknown) {
    const event = new MessageEvent(type, { data: JSON.stringify(data) });
    this.listeners.get(type)?.forEach(listener => listener(event));
  }

  close() {
    this.closed = true;
  }
}

const snapshot = (
  sessionId: string,
  lastEventSequence: number,
  phase: SessionRuntime['phase'] = 'idle',
): SessionRuntime => ({
  sessionId,
  phase,
  activeTurnId: phase === 'running' ? `turn-${sessionId}` : undefined,
  paused: phase === 'paused',
  queue: [],
  lastEventSequence,
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}

describe('useSessionRuntime', () => {
  let container: HTMLDivElement;
  let root: Root;
  let current!: ReturnType<typeof useSessionRuntime>;

  function Harness({ sessionId }: { sessionId?: string }) {
    current = useSessionRuntime(sessionId);
    return null;
  }

  beforeEach(() => {
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
    FakeEventSource.instances = [];
    vi.stubGlobal('EventSource', FakeEventSource);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  it('isolates snapshots by session and never lets an older fetch overwrite SSE state', async () => {
    const firstFetch = deferred<SessionRuntime>();
    const secondFetch = deferred<SessionRuntime>();
    vi.mocked(apiFetch).mockImplementation((path) => (
      String(path).includes('session-a') ? firstFetch.promise : secondFetch.promise
    ));

    await act(async () => root.render(<Harness sessionId="session-a" />));
    expect(FakeEventSource.instances[0].url).toContain('/session-a/events?after=0');

    await act(async () => firstFetch.resolve(snapshot('session-a', 42)));
    expect(current.runtime?.sessionId).toBe('session-a');

    await act(async () => root.render(<Harness sessionId="session-b" />));
    expect(current.runtime).toBeNull();
    expect(FakeEventSource.instances[0].closed).toBe(true);
    expect(FakeEventSource.instances[1].url).toContain('/session-b/events?after=0');

    await act(async () => {
      FakeEventSource.instances[0].emit('session_runtime', snapshot('session-a', 43));
      FakeEventSource.instances[1].emit('session_runtime', snapshot('session-b', 3, 'running'));
    });
    expect(current.runtime).toEqual(snapshot('session-b', 3, 'running'));

    await act(async () => secondFetch.resolve(snapshot('session-b', 2)));
    expect(current.runtime).toEqual(snapshot('session-b', 3, 'running'));
  });
});

describe('isSessionTurnRunning', () => {
  it('uses the durable runtime when the browser stream is detached', () => {
    expect(isSessionTurnRunning(snapshot('session-a', 2, 'running'), false)).toBe(true);
    expect(isSessionTurnRunning(snapshot('session-a', 3), false)).toBe(false);
    expect(isSessionTurnRunning(null, true)).toBe(true);
  });
});
