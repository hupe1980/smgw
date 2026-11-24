# List the available recipes.
default:
    @just --list

# Apply Rust formatting across the workspace.
fmt:
    cargo fmt --all

# Check formatting, the no_std parser, and all Clippy targets.
lint:
    cargo fmt --all -- --check
    cargo check --locked -p smgw --no-default-features --features wakeup
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

[private]
_ensure-pki directory:
    @if [ ! -d {{quote(directory)}} ]; then just pki {{quote(directory)}}; fi

# Read the simulated 1250 W measurement through the authenticated gateway.
demo directory="target/test-pki": (_ensure-pki directory)
    cargo run --locked -p smgw-cls --features simulator --example mms_read -- {{quote(directory)}}

# Import the three daily FSCH tariff windows through the authenticated gateway.
tariff directory="target/test-pki": (_ensure-pki directory)
    cargo run --locked -p smgw-cls --features simulator,fsch-tariff --example mms_tariff -- {{quote(directory)}}

# Exercise the IEC 61850 and EEBUS bridge with in-memory peers.
bridge:
    cargo test --locked -p smgw-cls --features fnn-bridge --test lpc_bridge --test relay_bridge

# Verify the bundled wake-up fixture with OpenSSL and Rust.
wakeup:
    python3 scripts/check-wakeup-fixture.py
    cargo run --locked -p smgw --features wakeup-verify --example wakeup_verify -- --fixture

# Build and check the Zola site and Rust API documentation.
site:
    RUSTDOCFLAGS="-D warnings" scripts/build-site.sh
    python3 scripts/check-site.py

# Preview the guide with live reload and both local Rust API references.
serve port="8000":
    RUSTDOCFLAGS="-D warnings" scripts/build-site.sh --serve {{quote(port)}}
