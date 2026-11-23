# List the available recipes.
default:
    @just --list

# Apply Rust formatting across the workspace.
fmt:
    cargo fmt --all

# Check formatting, the no_std parser, and all Clippy targets.
lint:
    cargo fmt --all -- --check
    cargo check --locked -p smgw-cls --no-default-features --features wakeup
    cargo clippy --locked --workspace --all-features --all-targets -- -D warnings

# Run the workspace tests, including examples and doctests.
test:
    cargo test --locked --workspace --all-features --all-targets
    cargo test --locked --workspace --all-features --doc
    python3 scripts/check-wakeup-replay.py

# Run lint and tests together.
check: lint test

# Generate a fresh test PKI. The destination must be new.
pki directory="target/test-pki":
    cargo run --locked -p xtask -- gen-pki {{quote(directory)}}

# Run the loopback MMS read with a test PKI, creating it on first use.
demo directory="target/test-pki":
    @if [ ! -d {{quote(directory)}} ]; then just pki {{quote(directory)}}; fi
    cargo run --locked --features simulator --example mms_read -- {{quote(directory)}}

# Exercise the IEC 61850 and EEBUS bridge with in-memory peers.
bridge:
    cargo test --locked -p smgw-cls --features fnn-bridge --test lpc_bridge

# Verify the bundled wake-up fixture with OpenSSL and Rust.
wakeup:
    python3 scripts/check-wakeup-fixture.py
    cargo run --locked --features wakeup-verify --example wakeup_verify -- --fixture

# Build and check the Zola site and Rust API documentation.
site:
    RUSTDOCFLAGS="-D warnings" scripts/build-site.sh
    python3 scripts/check-site.py
