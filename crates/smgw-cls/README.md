# smgw-cls

A Rust library and loopback simulator for a Controllable Local System (CLS) communicating through a smart meter gateway. Drive a connection with a caller-supplied clock or read an IEC 61850 measurement over mutual TLS.

[Project site](https://hupe1980.github.io/smgw/) · [Developer guide](https://hupe1980.github.io/smgw/guide/) · [Published crate](https://crates.io/crates/smgw-cls) · [Changelog](https://github.com/hupe1980/smgw/blob/main/CHANGELOG.md)

Add the library with `cargo add smgw-cls`.

## Read a measurement on loopback

Run these commands from a workspace checkout with Rust 1.90 or later and OpenSSL development libraries with Brainpool support.

```sh
cargo run -p xtask -- gen-pki
cargo run -p smgw-cls --features simulator --example mms_read
```

The first command generates seven-day test identities in `target/test-pki`. The second runs the CLS, gateway and market participant on loopback. The market participant reads the meter over two mutually authenticated TLS connections.

```text
CLSLD0/MMXU1$MX$TotW$mag$f = 1250 W (MMS read over HAN and WAN mutual TLS)
```

With the same test identities, run `cargo run -p smgw-cls --features simulator,fsch-tariff --example mms_tariff` to import three eight-hour windows from the read-only tariff provider. The example checks the provider’s clock and maps 12, 6 and 18 ct/kWh to Standard, Low and High.

## Choose a feature

| Configuration | Capability |
| --- | --- |
| `default-features = false` | Allocation-free `no_std` session engine, logical FNN relay controls and Module 3 tariff lookup |
| `runtime` | Tokio/OpenSSL endpoints and a bounded, single-session loopback proxy |
| `simulator` | Runtime and the read-only IEC 61850 example |
| `fnn-bridge` | Map IEC controls to EEBUS limits or logical relays, and serve power measurements or contact feedback over MMS |
| `fsch-tariff` | Import fixed-start FSCH schedules over blocking or async MMS, check the provider’s clock quality and map prices to tariff levels |

Run `just bridge` to exercise the controls and independent feedback. The guide covers TLS setup, commissioning and tariff schedules. [Loopback simulator](https://hupe1980.github.io/smgw/guide/cls/) · [EEBUS and relay controls](https://hupe1980.github.io/smgw/guide/controls/)

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

Licensed under MIT OR Apache-2.0.
