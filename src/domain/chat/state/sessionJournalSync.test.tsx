import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { apiFetch } from '@/core/utils';
import type { ChatMessage, SessionRuntime } from '../types';
import {
  mergeDurableSessionMessages,
  projectDurableSessionMessages,
  shouldSyncSessionJournal,
  useSessionJournalSync,
} from './sessionJournalSync';

vi.mock('@/core/utils', () => ({ apiFetch: vi.fn() }));

const message = (role: ChatMessage['role'], content: string, id?: string): ChatMessage => ({
  id,
  role,
  content,
});

const runtime = (
  sessionId: string,
  phase: SessionRuntime['phase'],
  lastEventSequence: number,
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

describe('mergeDurableSessionMessages', () => {
  it('collapses daemon tool cycles to the assistant bubble shape used by Chat', () => {
    const projected = projectDurableSessionMessages([
      message('user', '帮我检查文件'),
      { ...message('assistant', '我先检查。'), tool_calls: [{ id: 'call-1', type: 'function', function: { name: 'read_file', arguments: '{}' } }] },
      { ...message('tool', '文件内容'), tool_call_id: 'call-1' },
      message('assistant', '检查完成。'),
    ]);
    expect(projected.map(item => [item.role, item.content])).toEqual([
      ['user', '帮我检查文件'],
      ['assistant', '我先检查。检查完成。'],
    ]);
  });

  it('inserts the durable suffix before messages authored after the request', () => {
    const base = message('user', '第一问', 'local-user');
    const newer = message('user', '用户刚刚发送的新消息', 'local-newer');
    const durableUser = message('user', '排队消息');
    const durableAnswer = message('assistant', '排队消息的回答');

    const merged = mergeDurableSessionMessages(
      [base],
      [base, newer],
      [message('user', '第一问'), durableUser, durableAnswer],
    );

    expect(merged.map(item => item.content)).toEqual([
      '第一问',
      '排队消息',
      '排队消息的回答',
      '用户刚刚发送的新消息',
    ]);
    expect(merged[0]).toBe(base);
    expect(merged[3]).toBe(newer);
    expect(merged[1].id).toBeTruthy();
    expect(merged[2].id).toBeTruthy();
  });

  it('keeps the current projection unchanged when its baseline diverged', () => {
    const baseline = [message('user', '旧问题')];
    const current = [message('user', '已经重新生成的问题', 'local')];
    const durable = [message('user', '旧问题'), message('assistant', '后台回答')];
    expect(mergeDurableSessionMessages(baseline, current, durable)).toBe(current);
  });
});

describe('shouldSyncSessionJournal', () => {
  it('selects initial settled snapshots and running-to-idle transitions only', () => {
    const idle = runtime('session-a', 'idle', 3);
    expect(shouldSyncSessionJournal(null, idle)).toBe(true);
    expect(shouldSyncSessionJournal(runtime('session-a', 'running', 2), idle)).toBe(true);
    expect(shouldSyncSessionJournal(idle, idle)).toBe(false);
    expect(shouldSyncSessionJournal(idle, runtime('session-a', 'running', 4))).toBe(false);
  });
});

describe('useSessionJournalSync', () => {
  let container: HTMLDivElement;
  let root: Root;

  function Harness({
    sessionId,
    sessionRuntime,
    localLoading = false,
    messages,
    onMerge,
  }: {
    sessionId?: string;
    sessionRuntime: SessionRuntime | null;
    localLoading?: boolean;
    messages: ChatMessage[];
    onMerge: (next: ChatMessage[]) => void;
  }) {
    useSessionJournalSync({
      sessionId,
      runtime: sessionRuntime,
      localLoading,
      messages,
      onMerge,
    });
    return null;
  }

  beforeEach(() => {
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  it('syncs the durable journal when opening a session that already settled', async () => {
    vi.mocked(apiFetch).mockResolvedValue({
      id: 'session-a',
      messages: [
        message('user', '原问题'),
        message('assistant', '原回答'),
        message('user', '离开页面期间执行的排队消息'),
        message('assistant', '后台完成结果'),
      ],
    });
    const onMerge = vi.fn();
    const visible = [message('user', '原问题'), message('assistant', '原回答')];

    await act(async () => root.render(
      <Harness sessionId="session-a" sessionRuntime={null} messages={visible} onMerge={onMerge} />,
    ));
    await act(async () => Promise.resolve());

    expect(apiFetch).toHaveBeenCalledWith('/api/agent/sessions/session-a/snapshot');
    expect(onMerge).toHaveBeenCalledTimes(1);
    expect(onMerge.mock.calls[0][0].map((item: ChatMessage) => item.content)).toEqual([
      '原问题',
      '原回答',
      '离开页面期间执行的排队消息',
      '后台完成结果',
    ]);
  });

  it('fences a late snapshot after switching sessions', async () => {
    const first = deferred<{ id: string; messages: ChatMessage[] }>();
    const second = deferred<{ id: string; messages: ChatMessage[] }>();
    vi.mocked(apiFetch).mockImplementation(path => (
      String(path).includes('session-a') ? first.promise : second.promise
    ));
    const onMerge = vi.fn();
    const baseA = [message('user', 'A')];
    const baseB = [message('user', 'B')];

    await act(async () => root.render(
      <Harness sessionId="session-a" sessionRuntime={runtime('session-a', 'idle', 1)} messages={baseA} onMerge={onMerge} />,
    ));
    await act(async () => root.render(
      <Harness sessionId="session-b" sessionRuntime={runtime('session-b', 'idle', 1)} messages={baseB} onMerge={onMerge} />,
    ));

    await act(async () => second.resolve({
      id: 'session-b',
      messages: [message('user', 'B'), message('assistant', 'B 的后台回答')],
    }));
    expect(onMerge).toHaveBeenCalledTimes(1);
    expect(onMerge.mock.calls[0][0].map((item: ChatMessage) => item.content)).toEqual(['B', 'B 的后台回答']);

    await act(async () => first.resolve({
      id: 'session-a',
      messages: [message('user', 'A'), message('assistant', 'A 的迟到回答')],
    }));
    expect(onMerge).toHaveBeenCalledTimes(1);
  });

  it('waits for the local stream to finish, then merges a daemon-owned turn', async () => {
    const response = deferred<{ id: string; messages: ChatMessage[] }>();
    vi.mocked(apiFetch).mockReturnValue(response.promise);
    const onMerge = vi.fn();
    const base = [message('user', '当前问题', 'local-base')];
    const running = runtime('session-a', 'running', 2);
    const settled = runtime('session-a', 'idle', 3);

    await act(async () => root.render(
      <Harness sessionId="session-a" sessionRuntime={running} localLoading messages={base} onMerge={onMerge} />,
    ));
    await act(async () => root.render(
      <Harness sessionId="session-a" sessionRuntime={settled} localLoading messages={base} onMerge={onMerge} />,
    ));
    expect(apiFetch).not.toHaveBeenCalled();

    await act(async () => root.render(
      <Harness sessionId="session-a" sessionRuntime={settled} messages={base} onMerge={onMerge} />,
    ));
    expect(apiFetch).toHaveBeenCalledTimes(1);

    const newer = message('user', '本地更新', 'local-newer');
    await act(async () => root.render(
      <Harness sessionId="session-a" sessionRuntime={settled} messages={[...base, newer]} onMerge={onMerge} />,
    ));
    await act(async () => response.resolve({
      id: 'session-a',
      messages: [
        message('user', '当前问题'),
        message('user', '排队问题'),
        message('assistant', '后台回答'),
      ],
    }));

    expect(onMerge).toHaveBeenCalledTimes(1);
    expect(onMerge.mock.calls[0][0].map((item: ChatMessage) => item.content)).toEqual([
      '当前问题',
      '排队问题',
      '后台回答',
      '本地更新',
    ]);
  });
});
