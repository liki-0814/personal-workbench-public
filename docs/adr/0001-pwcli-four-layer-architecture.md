# ADR 0001: PWCLI runtime ownership and dependency direction

Status: Accepted

## Decision

PWCLI keeps its existing crate and long source files. The framework is defined by four runtime
owners, not by directory count:

```text
                         ownership

CLI / Web JSON / Web SSE / internal worker
                    │
                    ▼
             RuntimeFactory             concrete dependency assembly
                    │ creates
                    ▼
              AgentRuntime              exactly one Agent turn
                │       │
                │       └──────────────► TaskBroker       cross-process work
                └──────────────────────► SessionManager   cross-turn session state

HTTP and SSE validation/mapping stay at the Web boundary. ACP external workers remain adapters
managed by TaskBroker.
```

The allowed code dependency direction is:

```text
entry adapters ──► composition ──► agent core ──► AI/provider adapters
      │                  │              │
      ├──► SessionManager│              └──► contracts / tool ports
      └──► TaskBroker    └──────────────────► contracts / concrete adapters

Forbidden: agent core ──► service / SessionManager / TaskBroker
Forbidden: tools ────────► service
Forbidden: HTTP/SSE ─────► composition / AgentRuntime / contracts
```

`RuntimeFactory` is the only production module allowed to construct `AgentRunner`, select an LLM,
collect tool schemas, select permissions, resolve Harness policy/reviewer, inject background/cache
dependencies, and produce a capability snapshot. It never persists a Session or Task.

`AgentRuntime::run_turn` is the only entry into one model/tool Turn. It creates the borrowing
`AgentRunner`, which creates `AgentGraph`; both are implementation details. AgentRuntime neither
writes a database nor emits SSE.

`SessionManager` owns queued input, steer/follow-up settlement, pause state and durable state across
Turns. `TaskBroker` owns delegated/background work across processes. Web routes retain the existing
session loop and finalization flow but receive an AgentRuntime from RuntimeFactory.

Runtime identity is explicit. Session/work-item IDs, cwd, acting model, image references, Web cache
and cancellation flow from `RuntimeFactory` through `ToolExecutionContext` to every tool invocation,
including parallel and spawned work. Tools must not recover these values from task-local or global
state. The only task-local exception is the legacy progress-emitter bridge; the independent worker
filesystem policy remains a process-wide security boundary rather than request identity.

## Migration rules

1. Establish logical boundaries inside the existing crate before creating a Cargo workspace.
2. Move stable data contracts without changing their serialized representation; retain temporary
   compatibility re-exports at their previous module paths.
3. CLI oneshot, internal worker and Web main request an `AgentRuntime` from `RuntimeFactory`; an
   entry point's old composition path is deleted in the same migration.
4. Do not dual-write durable state and do not change SQLite schemas during structural refactors.
5. Architecture checks are a ratchet: existing exceptions may be removed, never expanded without
   updating this ADR or adding a new one.
6. A tool that needs new invocation state extends `ToolExecutionContext`; it does not create another
   ambient context or read service state.

## Terminology

- **Turn**: one model/tool loop owned by agent-core.
- **AgentRuntime**: the fully assembled owner of one Turn; it is not durable state.
- **Runtime profile**: an assembly policy: CLI oneshot, internal worker or Web main.
- **Adapter**: infrastructure implementing a stable port.
- **Projection**: a transport-facing read model derived from domain state or events.
- **Capability snapshot**: a secret-free description of one entry point's effective tool, model,
  middleware, permission and harness configuration.
