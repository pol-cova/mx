#!/bin/sh
set -eu

if [ "$(uname -s)" != Darwin ]; then
    echo 'mx-guest requires macOS with an iOS Simulator SDK.' >&2
    exit 1
fi
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ -d /Volumes/DevData/tmp ]; then
    export TMPDIR="${TMPDIR:-/Volumes/DevData/tmp}"
else
    export TMPDIR="${TMPDIR:-$root/target}"
fi
destination="${MX_GUEST_PATH:-$root/target/mx-guest}"
mkdir -p "$(dirname "$destination")"
make -C "$root/guest" OUT="$destination"
printf 'mx-guest installed at %s\n' "$destination"
printf 'XCT frameworks at %s/Frameworks\n' "$(dirname "$destination")"
printf 'Optional 0B app: make -C "%s/guest" app OUT="%s"\n' "$root" "$destination"
