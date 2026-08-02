# Personal Workbench

Source-only distribution of the Personal Workbench web application and `pwcli` daemon.

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
