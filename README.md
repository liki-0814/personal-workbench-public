# Personal Workbench

Source-only distribution of the Personal Workbench web application and `pwcli` daemon.

## Development

```bash
npm install
npm run dev
```

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

Local configuration and runtime data are stored outside the source tree under
`~/.pwcli/`.
