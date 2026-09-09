#!/bin/sh
set -eu

if [ "$(uname -s)" != Darwin ]; then
    echo 'Mx requires macOS with Xcode and an iOS Simulator runtime.' >&2
    exit 1
fi
command -v cargo >/dev/null 2>&1 || {
    echo 'Install Rust 1.89 or later, then rerun this script.' >&2
    exit 1
}
xcrun --find simctl >/dev/null
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
sh "$root/scripts/setup-axe.sh"
cargo install --path "$root" --locked --root "${CARGO_HOME:-$HOME/.cargo}"
mx_binary="${CARGO_HOME:-$HOME/.cargo}/bin/mx"
printf '\nInstalled Mx. List simulators with:\n  "%s" devices\n' "$mx_binary"
printf '\nGenerate your MCP client configuration with:\n  "%s" mcp-config\n' "$mx_binary"
