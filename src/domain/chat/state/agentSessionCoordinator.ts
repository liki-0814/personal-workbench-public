export type AgentSessionFactory = (name: string, cwd: string) => Promise<string>;

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
