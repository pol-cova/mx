#!/bin/sh
set -eu

# Pinned upstream release; no Homebrew or system-wide installation is needed.
version=1.8.0
checksum=7b76340b72e90d0f211bc7c4636f15009076eff07acef2f2b632b175debd8834
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
destination="$root/.mx/tools/axe-$version"
if [ -x "$destination/axe" ]; then
    "$destination/axe" --version
    exit 0
fi
archive=$(mktemp -t mx-axe)
trap 'rm -f "$archive"' EXIT HUP INT TERM
curl --fail --location --silent --show-error \
    "https://github.com/cameroncooke/AXe/releases/download/v$version/AXe-macOS-v$version-universal.tar.gz" \
    --output "$archive"
actual=$(shasum -a 256 "$archive" | cut -d ' ' -f 1)
if [ "$actual" != "$checksum" ]; then
    echo "AXe checksum mismatch; installation stopped" >&2
    exit 1
fi
mkdir -p "$destination"
tar -xzf "$archive" -C "$destination"
"$destination/axe" --version
printf 'Set MX_AXE_PATH to %s/axe\n' "$destination"
