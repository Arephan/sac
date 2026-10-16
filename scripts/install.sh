#!/usr/bin/env bash
# Build sac from source and put it on your PATH. Needs a Rust toolchain (https://rustup.rs).
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. Install Rust first: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
  exit 1
fi
cargo build --release
dest="${1:-$HOME/.local/bin}"
mkdir -p "$dest"
cp target/release/sac "$dest/sac"
echo "installed $dest/sac ($("$dest/sac" --version))"
case ":$PATH:" in
  *":$dest:"*) ;;
  *) echo "add to PATH: export PATH=\"$dest:\$PATH\"" ;;
esac
