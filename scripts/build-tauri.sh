#!/usr/bin/env bash
# Release-build the Gen-Audio desktop app and the gen-audio-mcp sidecar.
# Linux packages deb when the webview SDK is present. NSIS/MSI are Windows-only;
# use scripts/build-tauri-windows.ps1 on that host.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo is required" >&2
  exit 1
fi

echo "build gen-audio-mcp sidecar"
cargo build -p gen-audio-mcp --release
sidecar="target/release/gen-audio-mcp"
test -x "$sidecar"

echo "mcp initialize handshake"
addr="127.0.0.1:8765"
"$sidecar" --http "$addr" >/tmp/gen-audio-mcp-build.log 2>&1 &
pid=$!
cleanup() { kill "$pid" >/dev/null 2>&1 || true; }
trap cleanup EXIT
ready=0
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if curl -sf "http://$addr/health" >/dev/null; then
    ready=1
    break
  fi
  sleep 0.2
done
test "$ready" = 1
body='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"build","version":"0.1.0"}}}'
response="$(curl -sf -H 'Content-Type: application/json' --data "$body" "http://$addr/mcp")"
printf '%s\n' "$response" | grep -q '"protocolVersion":"2025-03-26"'
printf '%s\n' "$response" | grep -q '"name":"gen-audio"'
echo "mcp handshake ok"

echo "desktop libraries (tray + embedded mcp)"
cargo build -p gen-audio-desktop --release

if command -v npm >/dev/null 2>&1; then
  (cd apps/desktop && npm ci && npm run build)
fi

if cargo tauri --version >/dev/null 2>&1 || npx --yes @tauri-apps/cli --version >/dev/null 2>&1; then
  echo "package linux bundles"
  (cd apps/desktop/src-tauri && npx --yes @tauri-apps/cli build --bundles deb) || {
    echo "tauri bundle step did not produce a deb; the release binary is still at target/release"
  }
else
  echo "tauri cli not used; release binary is target/release/gen-audio-desktop"
fi

echo "sidecar copy next to the desktop binary"
cp -f "$sidecar" "target/release/gen-audio-mcp"
echo "build-tauri.sh finished"
