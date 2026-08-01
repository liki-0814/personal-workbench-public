import { describe, expect, it, vi } from 'vitest';
import { AgentSessionCoordinator } from './agentSessionCoordinator';

describe('AgentSessionCoordinator', () => {
  it('coalesces concurrent creation for the same chat and cwd', async () => {
    let resolve!: (id: string) => void;
    const create = vi.fn(() => new Promise<string>(done => { resolve = done; }));
    const coordinator = new AgentSessionCoordinator(create);

    const first = coordinator.ensure('chat-1', '/workspace');
    const second = coordinator.ensure('chat-1', '/workspace');

    expect(create).toHaveBeenCalledTimes(1);
    resolve('daemon-1');
    await expect(Promise.all([first, second])).resolves.toEqual(['daemon-1', 'daemon-1']);
  });

  it('does not share creation across chats or workspaces', async () => {
    const create = vi.fn(async (name: string, cwd: string) => `${name}:${cwd}`);
    const coordinator = new AgentSessionCoordinator(create);

    await Promise.all([
      coordinator.ensure('chat-1', '/one'),
      coordinator.ensure('chat-2', '/one'),
      coordinator.ensure('chat-1', '/two'),
    ]);

    expect(create).toHaveBeenCalledTimes(3);
  });

  it('allows retry after a failed creation', async () => {
    const create = vi.fn()
      .mockRejectedValueOnce(new Error('offline'))
      .mockResolvedValueOnce('daemon-2');
    const coordinator = new AgentSessionCoordinator(create);

    await expect(coordinator.ensure('chat-1', '/workspace')).rejects.toThrow('offline');
    await expect(coordinator.ensure('chat-1', '/workspace')).resolves.toBe('daemon-2');
    expect(create).toHaveBeenCalledTimes(2);
  });
});
