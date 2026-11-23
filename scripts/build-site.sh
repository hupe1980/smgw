#!/bin/sh
# Assemble the Zola pages and their matching Rust API in one portable directory.
set -eu
cd "$(dirname "$0")/.."
root=$(pwd)
cargo doc --locked -p smgw-cls --all-features --no-deps --target-dir target/site-build
zola --root site check --skip-external-links
zola --root site build --output-dir "$root/target/site" --force
mkdir -p target/site/api
cp -R target/site-build/doc/. target/site/api/
printf '%s\n' 'Site built in target/site. Preview with: python3 -m http.server --directory target/site'
