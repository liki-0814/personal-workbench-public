export type AgentSessionFactory = (name: string, cwd: string) => Promise<string>;

export function formatAgentSessionCreationError(error: unknown): string {
  const detail = error instanceof Error ? error.message.trim() : '';
  const daemonUnavailable = error instanceof TypeError
    || /failed to fetch|networkerror|load failed|connection refused|无法连接|网络错误/i.test(detail);

  if (daemonUnavailable) return '无法连接 daemon，请运行 pwcli daemon start';
  if (detail) return `创建会话失败：${detail}`;
  return '创建会话失败，请查看 daemon 日志';
}

/**
 * Coalesces concurrent daemon-session creation for the same local chat.
 * The coordinator is intentionally keyed by both chat id and canonical cwd so
 * a workspace relocation can never reuse an in-flight request for the old path.
 */
export class AgentSessionCoordinator {
  private readonly pending = new Map<string, Promise<string>>();

  constructor(private readonly create: AgentSessionFactory) {}

  ensure(sessionKey: string, cwd: string): Promise<string> {
    const key = `${sessionKey}\u0000${cwd}`;
    const existing = this.pending.get(key);
    if (existing) return existing;

    const request = this.create(sessionKey, cwd).finally(() => {
      if (this.pending.get(key) === request) this.pending.delete(key);
    });
    this.pending.set(key, request);
    return request;
  }
}
