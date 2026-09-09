#!/bin/sh
set -eu

# Pinned upstream release; no Homebrew or system-wide installation is needed.
version=1.8.0
checksum=7b76340b72e90d0f211bc7c4636f15009076eff07acef2f2b632b175debd8834
destination="$HOME/Library/Application Support/Mx/tools/axe-$version"
if [ -x "$destination/axe" ]; then
    printf 'Using installed AXe at %s/axe\n' "$destination"
    "$destination/axe" --version
    exit 0
fi
archive=$(mktemp -t mx-axe)
trap 'rm -f "$archive"' EXIT HUP INT TERM
printf 'Downloading AXe %s...\n' "$version"
curl --fail --location --progress-bar --show-error --connect-timeout 30 --max-time 600 \
    "https://github.com/cameroncooke/AXe/releases/download/v$version/AXe-macOS-v$version-universal.tar.gz" \
    --output "$archive"
printf 'Verifying AXe download...\n'
actual=$(shasum -a 256 "$archive" | cut -d ' ' -f 1)
if [ "$actual" != "$checksum" ]; then
    echo "AXe checksum mismatch; installation stopped" >&2
    exit 1
fi
mkdir -p "$destination"
tar -xzf "$archive" -C "$destination"
"$destination/axe" --version
printf 'AXe installed at %s/axe. Mx discovers it automatically.\n' "$destination"
