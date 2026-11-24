#!/bin/sh
# Build the Pages artifact or start a live preview with the same API references.
set -eu
cd "$(dirname "$0")/.."
root=$(pwd)
if [ "$#" -gt 0 ]; then
    if [ "$#" -ne 2 ] || [ "$1" != "--serve" ]; then
        echo 'Usage: build-site.sh [--serve PORT]' >&2
        exit 2
    fi
    port=$2
    # Check the requested port before rebuilding rustdoc.
    python3 - "$port" <<'PY'
import socket
import sys

value = sys.argv[1]
if not (value.isascii() and value.isdecimal() and len(value) <= 5 and 1 <= int(value) <= 65535):
    sys.exit("Choose a port between 1 and 65535.")
with socket.socket() as probe:
    probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        probe.bind(("127.0.0.1", int(value)))
        probe.listen()
    except OSError as error:
        sys.exit(f"Cannot use 127.0.0.1:{value}: {error.strerror}")
PY
fi

# rustdoc can leave pages for removed modules in its output directory.
cargo clean --doc --target-dir target/site-build
cargo doc --locked -p smgw -p smgw-cls --all-features --no-deps --target-dir target/site-build
if [ "$#" -gt 0 ]; then
    preview=$(mktemp -d "$root/target/site-preview.XXXXXXXX")
    trap 'rm -rf "$preview"' EXIT
    printf 'Starting preview at http://127.0.0.1:%s/ (Ctrl-C to stop)\n' "$port"
    zola --root site serve --interface 127.0.0.1 --port "$port" --output-dir "$preview" --force
    exit
fi

zola --root site check --skip-external-links
zola --root site build --output-dir "$root/target/site" --force
printf '%s\n' 'Site built in target/site.'
