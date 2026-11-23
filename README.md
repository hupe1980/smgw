# smgw-cls

A Rust library and loopback simulator for a Controllable Local System (CLS) communicating through a smart meter gateway. Drive a connection with a caller-supplied clock, read an IEC 61850 measurement over mutual TLS, or verify an archived gateway-management wake-up.

[Project site](https://hupe1980.github.io/smgw-cls/) · [Developer guide](https://hupe1980.github.io/smgw-cls/guide/) · [Published crate](https://crates.io/crates/smgw-cls)

Add the library with `cargo add smgw-cls`. To run the simulator, clone the repository and follow the example below.

## Read a measurement on loopback

The full workspace requires Rust 1.90 or later and OpenSSL development libraries with Brainpool support. From the repository root, run:

```sh
cargo run -p xtask -- gen-pki
cargo run --features simulator --example mms_read
```

The first command creates seven-day test identities in `target/test-pki`. Reuse them for further reads. To regenerate, pass a new directory to `gen-pki` and to `mms_read` after `--`.

The CLS connects to the gateway on the home area network (HAN). The gateway authenticates the CLS and checks its requested route before connecting to the active external market participant on the wide area network (WAN). That participant reads a synthetic meter value through the gateway using Manufacturing Message Specification (MMS).

```text
CLSLD0/MMXU1$MX$TotW$mag$f = 1250 W (MMS read over HAN and WAN mutual TLS)
```

## Choose a feature

| Configuration | Capability |
| --- | --- |
| `default-features = false` | Allocation-free `no_std` session engine, logical FNN relay controls and Module 3 tariff lookup |
| `runtime` | Tokio/OpenSSL endpoints and a bounded, single-session loopback proxy |
| `simulator` | Runtime and the read-only IEC 61850 example |
| `fnn-bridge` | Map IEC controls to EEBUS limits or logical relays, and serve power measurements or contact feedback over MMS |
| `wakeup` | Inspect a management wake-up datagram without allocation |
| `wakeup-verify` | Verify the recipient, signature, freshness and replay status against a provisioned gateway administrator key |
| `wakeup-store` | Persist each recipient’s latest accepted timestamp in redb across verifier restarts and signing-key rotation |

From the repository root, run `just bridge` to test the MMS mappings, or `cargo run --locked --no-default-features --example relay_tariff` to see relay commands, independent contact feedback and tariff changes.

The simulator uses a local TLS 1.2 Brainpool P-256 profile with separate trust anchors and pinned peer certificates on each leg. For an offline signature check with a known public key and reference time, run `cargo run --features wakeup-verify --example wakeup_verify -- --fixture`. Deployment requires its own certificate policy and gateway interoperability testing.

## Drive a session

```rust
use core::time::Duration;
use smgw_cls::engine::{Action, Policy, Session};

let mut session = Session::new(Policy::default());
let Some(Action::Connect { generation }) = session.start(Duration::ZERO) else {
    unreachable!("a fresh session admits one connection");
};
// Report success after the transport authenticates the peer and authorizes its route.
assert_eq!(session.connected(generation, Duration::from_secs(1)),
           Some(Action::Ready { generation }));
assert_eq!(session.cancel(), Some(Action::Close { generation }));
```

Arm a timer for `Session::deadline()`, call `tick(now)` when it expires, and execute each returned action. The generation identifies the attempt so a late callback cannot activate a newer connection.

For development, `just check` runs formatting, Clippy and tests. With Zola installed, `just site` builds and checks the guide and Rust API.

Licensed under MIT OR Apache-2.0.
