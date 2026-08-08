# Personal Workbench

Source-only distribution of the Personal Workbench web application and `pwcli` daemon.

## Repository map

Personal Workbench is one product with two build targets: a React frontend and a
Rust executable that provides the CLI, local daemon, HTTP/SSE API, and embedded
production web application.

```text
Browser
  -> index.html -> src/main.tsx -> src/App.tsx
  -> shell / features / domain
  -> core HTTP, configuration, storage, and LLM clients
  -> /api
  -> pwcli service boundary
  -> SessionManager / TaskBroker / RuntimeFactory
  -> AgentRuntime -> agent core -> tools and AI provider adapters
```

| Path | Responsibility |
| --- | --- |
| `src/core/` | Frontend infrastructure shared across product areas: configuration, storage, LLM clients, and generic utilities. |
| `src/domain/` | Product domains such as chat, todo, documents, jobs, habits, and pomodoro. A domain owns its types, state, domain logic, and domain-specific UI. |
| `src/features/` | User-facing workflows that compose one or more domains, such as workbench, settings, import/export, and search. |
| `src/shell/` | Application-wide chrome and reusable UI/state, including the header, theme, notifications, and Markdown rendering. |
| `src/App.tsx` | Frontend composition root: connects domains and features and owns top-level navigation. |
| `pwcli/src/` | Rust application code for CLI/TUI entry points, daemon/API, sessions, delegated tasks, the agent runtime, tools, and provider integrations. |
| `pwcli/resources/` | Runtime resources embedded or distributed with `pwcli`. |
| `config/` | TypeScript, Vite, Vitest, ESLint, Tailwind, and architecture-check configuration. |
| `scripts/` | Build, migration, and architecture-check scripts. |
| `tests/` | Cross-module frontend integration and regression tests plus shared fixtures. |
| `docs/adr/` | Accepted architectural decisions and dependency rules. |

### Frontend boundaries

`src/main.tsx` bootstraps configuration and storage synchronization, then mounts
`src/App.tsx`. `App.tsx` is the composition root; reusable product behavior
belongs in a domain or feature rather than in the entry point.

- Put framework-independent, broadly shared infrastructure in `core`.
- Put behavior and state owned by one product concept in its `domain`.
- Put workflows that coordinate multiple domains in `features`.
- Put global application chrome and shared presentation primitives in `shell`.
- Import a domain through its `index.ts` public surface when one exists; avoid
  reaching into another domain's internal state or UI directories.

The `@/` alias resolves to `src/`.

### PWCLI runtime boundaries

The Rust directory is intentionally a single crate. Its architecture is based
on runtime ownership and dependency direction rather than one directory per
layer:

```text
CLI / Web JSON / Web SSE / internal worker
                  -> RuntimeFactory (composition)
                  -> AgentRuntime (one model/tool turn)
                     -> agent core -> contracts / tool ports
                     -> AI and provider adapters

SessionManager owns state across turns.
TaskBroker owns delegated/background work across processes.
```

Agent core must not depend on `service`, `SessionManager`, or `TaskBroker`, and
tools must not depend on `service`. `RuntimeFactory` is the only production
composition root for an `AgentRuntime`. See
[`docs/adr/0001-pwcli-four-layer-architecture.md`](docs/adr/0001-pwcli-four-layer-architecture.md)
for the complete ownership model. Run `npm run check:architecture` to verify the
enforced dependency rules.

### Test placement

- Keep focused unit and component tests beside the source as `*.test.ts` or
  `*.test.tsx`.
- Put cross-module behavior, build contracts, storage integration, and shared
  regression fixtures under top-level `tests/`.
- Rust unit tests stay beside their modules; crate-level integration tests live
  in `pwcli/tests/`.

## Prerequisites

- Node.js 18 or newer and npm
- A working Rust toolchain with Cargo

The setup script can install missing prerequisites, migrate a legacy `.env`,
run first-time configuration, build both parts of the application, and start
the daemon:

```bash
./setup.sh
```

## Development

Install the frontend dependencies first:

```bash
npm install
```

Run the Rust API/daemon and Vite frontend in separate terminals:

```bash
cd pwcli
cargo run -- daemon run
```

```bash
npm run dev
```

Vite serves the frontend at `http://127.0.0.1:5173` and proxies `/api` to the
daemon. The Vite development port is fixed at `5173`. The daemon defaults to
`http://127.0.0.1:3456`; its port or an explicit frontend backend URL can be
set in `~/.pwcli/config.json`.

## Verification

```bash
npm run lint
npm run test
npm run build
cd pwcli
cargo fmt --check
cargo test
```

The production build embeds the generated `dist/` frontend into the Rust binary:

```bash
npm run build
cd pwcli
cargo build --release
```

Run the resulting application with:

```bash
./pwcli/target/release/pwcli web
```

Local configuration and runtime data are stored outside the source tree under
`~/.pwcli/`.

## AI Provider 协议与个性化配置

协议层负责“求同”，个性化 knobs 负责“存异”：

- 协议：`openai_chat` / `openai_responses` / `anthropic_messages` / `google_generative`
- Provider 级：`useProxy`
- Model 级：`capabilities`、`requestParams`、`thinkingParams`、`deferredToolsMode`、`maxOutput`、`contextWindow`

示例：

```json
{
  "name": "Claude",
  "protocol": "anthropic_messages",
  "baseUrl": "https://api.anthropic.com",
  "apiKey": "sk-...",
  "models": [
    {
      "id": "claude-sonnet-4-6",
      "name": "Sonnet",
      "capabilities": { "vision": true, "thinking": true },
      "thinkingParams": { "budget_tokens": 2048 }
    }
  ]
}
```

```json
{
  "name": "Compatible Gateway",
  "protocol": "openai_chat",
  "baseUrl": "https://gateway.example.com/v1",
  "useProxy": true,
  "apiKey": "sk-...",
  "models": [
    {
      "id": "demo-model",
      "name": "Demo",
      "deferredToolsMode": "enabled",
      "requestParams": { "top_p": 0.95 }
    }
  ]
}
```

同协议内不根据供应商名称/URL/模型 id 做暗规则；代理、延迟工具、额外参数都必须显式配置 knobs。
