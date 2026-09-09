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
available_kib=$(df -Pk "$HOME" | awk 'END {print $4}')
if [ "$available_kib" -lt 2097152 ]; then
    echo 'Mx installation needs at least 2 GiB free for downloads and build output. Free disk space and retry.' >&2
    exit 1
fi
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
sh "$root/scripts/setup-axe.sh"
printf 'Building and installing Mx...\n'
cargo install --path "$root" --locked --root "${CARGO_HOME:-$HOME/.cargo}"
mx_binary="${CARGO_HOME:-$HOME/.cargo}/bin/mx"
printf '\nInstalled Mx. List simulators with:\n  "%s" devices\n' "$mx_binary"
printf '\nGenerate your MCP client configuration with:\n  "%s" mcp-config\n' "$mx_binary"
