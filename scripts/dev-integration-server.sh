#!/usr/bin/env bash
set -euo pipefail

integration_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ ! -e "$integration_root/.git" || "$(uname -s)" != Linux ]] ||
  [[ "$(git -C "$integration_root" branch --show-current)" == main ]]; then
  echo "Run integration server from a non-main Git checkout on Linux." >&2
  exit 1
fi

integration_runtime=/tmp/phantom-terax-v090-runtime
mkdir -p "$integration_runtime"/{config,data,cache}
cat > "$integration_runtime/config.yaml" <<EOF
bind-addr: 127.0.0.1:19889
auth: none
pty-backend: local
web-root: $integration_root/dist
room:
  name: integration-v090
  peer: integration-v090
  auto-start: false
  peek: false
EOF

unset PHANTOM_CERT PHANTOM_CERT_KEY PHANTOM_PASSWORD PHANTOM_USERNAME
cd -- "$integration_root"
exec env \
  PHANTOM_CONFIG="$integration_runtime/config.yaml" \
  PHANTOM_HOST=127.0.0.1 PHANTOM_PORT=19889 PHANTOM_PTY_BACKEND=local \
  PHANTOM_WEB_ROOT="$integration_root/dist" \
  XDG_CONFIG_HOME="$integration_runtime/config" \
  XDG_DATA_HOME="$integration_runtime/data" \
  XDG_CACHE_HOME="$integration_runtime/cache" \
  CARGO_TARGET_DIR="$integration_root/target" CARGO_PROFILE_DEV_DEBUG=0 \
  CARGO_INCREMENTAL=0 HISTFILE=/dev/null \
  cargo run --locked -p phantom-server --bin phantom-server
