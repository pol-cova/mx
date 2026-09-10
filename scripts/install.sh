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
home_available_kib=$(df -Pk "$HOME" | awk 'END {print $4}')
if [ "$home_available_kib" -lt 102400 ]; then
    echo 'Mx installation needs at least 100 MiB free in your home directory.' >&2
    exit 1
fi
build_path=${CARGO_TARGET_DIR:-$root/target}
build_parent=$build_path
while [ ! -e "$build_parent" ]; do
    build_parent=$(dirname "$build_parent")
done
build_available_kib=$(df -Pk "$build_parent" | awk 'END {print $4}')
if [ "$build_available_kib" -lt 2097152 ]; then
    echo 'Mx installation needs at least 2 GiB free on the build volume.' >&2
    echo 'Set CARGO_TARGET_DIR to a path on a larger volume and retry.' >&2
    exit 1
fi
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
sh "$root/scripts/setup-axe.sh"
printf 'Building and installing Mx...\n'
cargo install --path "$root" --locked --root "${CARGO_HOME:-$HOME/.cargo}"
mx_binary="${CARGO_HOME:-$HOME/.cargo}/bin/mx"
printf '\nInstalled Mx. List simulators with:\n  "%s" devices\n' "$mx_binary"
printf '\nGenerate your MCP client configuration with:\n  "%s" mcp-config\n' "$mx_binary"
