#!/bin/bash
set -e

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

echo "=== Personal Workbench Setup ==="
echo ""

# 1. Check & install prerequisites
echo "[1/8] Checking prerequisites..."

if ! command -v node >/dev/null 2>&1; then
  if command -v brew >/dev/null 2>&1; then
    echo "  Node.js not found, installing via Homebrew..."
    brew install node
  else
    echo "  Node.js not found, installing via nvm..."
    curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.3/install.sh | bash
    export NVM_DIR="$HOME/.nvm"
    # shellcheck source=/dev/null
    [ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"
    nvm install --lts
  fi
fi

NODE_VER=$(node -v | cut -d'.' -f1 | tr -d 'v')
if [ "$NODE_VER" -lt 18 ]; then
  echo "ERROR: Node.js >= 18 required (current: $(node -v))"
  echo "  Upgrade: brew upgrade node  OR  nvm install --lts"
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "  Rust/Cargo not found, installing via rustup..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  # shellcheck source=/dev/null
  source "$HOME/.cargo/env"
elif ! rustc --print sysroot >/dev/null 2>&1; then
  echo "  Rust toolchain broken, reinstalling stable..."
  rustup toolchain install stable --force
fi

echo "  Node $(node -v), Rust $(rustc --version | awk '{print $2}')"

# 2. Install frontend dependencies
echo "[2/8] Installing npm dependencies..."
npm install --silent

# 3. Migrate the complete legacy .env, if present
echo "[3/8] Migrating legacy .env..."
node scripts/migrate-env-to-config.mjs

# 4. Validate the unified config location
echo "[4/8] Checking ~/.pwcli/config.json..."
mkdir -p "$HOME/.pwcli"
echo "  Configuration source: $HOME/.pwcli/config.json"

# 5. Onboarding wizard (new users only)
echo "[5/8] First-time setup wizard..."
NEED_WIZARD=1
if [ -f "$HOME/.pwcli/config.json" ]; then
  if node -e 'const c=require(process.env.HOME+"/.pwcli/config.json"); process.exit(Array.isArray(c.providers)&&c.providers.some(p=>p.api_key)?0:1)' 2>/dev/null; then
    NEED_WIZARD=0
  fi
fi
if [ "$NEED_WIZARD" = "1" ]; then
  if [ -t 0 ] && [ -t 1 ]; then
    node scripts/onboarding.mjs || echo "  ⚠ 向导跳过(可稍后在前端 SettingsModal 配置)"
  else
    echo "  非交互终端,跳过向导(可稍后手动 node scripts/onboarding.mjs)"
  fi
else
  echo "  配置已存在,跳过向导"
fi

# 6. Build frontend first: pwcli embeds dist/ at Rust compile time.
echo "[6/8] Building frontend and pwcli (release)..."
npm run build --silent
echo "  Built: dist/"
(cd pwcli && cargo build --release --quiet)
echo "  Built: pwcli/target/release/pwcli (with embedded Web UI)"

# 7. Start the native daemon
echo "[7/8] Starting daemon..."
"$ROOT/pwcli/target/release/pwcli" daemon restart

# 8. Verify (poll up to 30s)
echo "[8/8] Verifying..."

DAEMON_OK="000"
for i in $(seq 1 30); do
  DAEMON_OK=$(curl -s -m 2 -o /dev/null -w "%{http_code}" http://127.0.0.1:3456/health || true)
  [ -z "$DAEMON_OK" ] && DAEMON_OK="000"
  if [ "$DAEMON_OK" = "200" ]; then
    break
  fi
  sleep 1
done

if [ "$DAEMON_OK" = "200" ]; then
  echo ""
  echo "=== All services running ==="
  echo ""
  echo "  Web + API: http://127.0.0.1:3456"
  echo ""
  echo "Commands:"
  echo "  pwcli daemon status     # Check status"
  echo "  pwcli daemon logs -f    # Follow logs"
  echo "  pwcli daemon restart    # Restart"
  echo "  pwcli daemon stop       # Stop"
  echo ""
  echo "Uninstall:"
  echo "  pwcli daemon stop"
  echo ""
else
  echo ""
  echo "WARNING: The daemon may not be ready yet (HTTP $DAEMON_OK)."
  echo "  Run 'pwcli daemon logs' to check for errors."
fi
