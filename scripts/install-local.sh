#!/usr/bin/env bash
# Install the local loom binary into ~/.cargo/bin/ and re-sign it on macOS.
# This prevents macOS Gatekeeper SIGKILL after a manual cargo build.
set -euo pipefail

cargo build --release

INSTALL_DIR="${CARGO_INSTALL_ROOT:-$HOME/.cargo}/bin"
mkdir -p "$INSTALL_DIR"

cp target/release/loom "$INSTALL_DIR/loom"
chmod +x "$INSTALL_DIR/loom"

if [[ "$OSTYPE" == "darwin"* ]]; then
    codesign -s - -f "$INSTALL_DIR/loom"
    echo "Signed $INSTALL_DIR/loom"
fi

echo "Installed $INSTALL_DIR/loom"
echo "  version: $($INSTALL_DIR/loom --version)"
