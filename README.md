# smgw

Rust libraries for smart meter gateway simulation. Model electricity metering or connect a Controllable Local System (CLS) through an authenticated loopback gateway.

| Crate | Use it for |
| --- | --- |
| [smgw](crates/smgw/) | Simulate the seven TR-03109-1 v1.1 metering use cases, v2.0 demand calculation (TAF 8) and meter-registered histories (TAF 15), or verify gateway-management wake-ups. |
| [smgw-cls](crates/smgw-cls/) | Drive CLS sessions, read IEC 61850 devices over TLS, and bridge EEBUS controls and measurements. |

Install with `cargo add smgw`, or select a CLS feature directly, for example `cargo add smgw-cls --features fnn-bridge`.

From the workspace root, run `cargo run -p smgw --example taf1` for a monthly closing reading. The [metering guide](https://hupe1980.github.io/smgw/guide/metering/) also shows how the same missing reading affects a tariff register and a meter series.

## Try the CLS simulator

With Rust 1.90 or later, OpenSSL development libraries with Brainpool support, and `just` installed, run `just demo` from the workspace root. This generates test identities and reads a simulated 1250 W meter value over two mutually authenticated TLS connections. Run `just tariff` to read a daily FSCH schedule through the same gateway and print its three UTC tariff windows.

Run `just check` for formatting, Clippy and workspace tests. With Zola installed, run `just serve` and open `http://127.0.0.1:8000/`. The preview reloads site edits and includes both crates’ API documentation.

[Developer guide](https://hupe1980.github.io/smgw/guide/) · [Changelog](CHANGELOG.md)

Licensed under MIT OR Apache-2.0.
