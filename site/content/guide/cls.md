+++
title = "CLS loopback simulator"
description = "Read a simulated IEC 61850 meter over mutual TLS, then drive a CLS connection with your own clock and transport."
weight = 3
+++

## Run the first read

Run the commands from the workspace root with Rust 1.90 or newer. The simulator needs OpenSSL development headers and a provider supporting Brainpool P-256. On Linux, install the OpenSSL development package and `pkg-config`. On macOS, Homebrew's OpenSSL works.

```sh
cargo run --locked -p xtask -- gen-pki
cargo run --locked -p smgw-cls --features simulator --example mms_read
```

The Controllable Local System (CLS) serves the reading using Manufacturing Message Specification (MMS). The first command creates credentials under `target/test-pki`. Run the second command as often as needed during their validity period. A successful run reads a named value from the bundled IEC 61850 model and completes the MMS release handshake.

```text
CLSLD0/MMXU1$MX$TotW$mag$f = 1250 W (MMS read over HAN and WAN mutual TLS)
```

## Follow the connection

The gateway accepts one connection on the home area network (HAN) and opens a second on the wide area network (WAN). Both use mutual TLS and ephemeral loopback ports in the simulator.

| Connection | What the TLS client does |
| --- | --- |
| HAN · CLS → gateway | The meter supplies the configured proxy ID, `meter.test`, through the TLS Server Name Indication (SNI) extension. |
| WAN · gateway → aEMT | After authenticating the meter and authorizing its route, the gateway dials the provisioned endpoint of the active external market participant (aEMT). |

The aEMT acts as the **MMS client** and the meter serves the read. The gateway terminates TLS on each leg and relays application bytes in both directions. Each side verifies its immediate peer's certificate chain and configured leaf fingerprint. The two gateway identities have distinct keys. The fixture selects TLS 1.2, `ECDHE-ECDSA-AES128-GCM-SHA256` and `brainpoolP256r1`. Hardware interoperability requires a separate test against the intended gateway. [BSI operating processes, §3.13.3](https://www.bsi.bund.de/SharedDocs/Downloads/DE/BSI/Publikationen/TechnischeRichtlinien/TR03109/TR-03109-1_Anlage_Betriebsprozesse_v2_0.pdf?__blob=publicationFile&v=2)

**Renew fixtures** by generating a fresh directory and passing it to the simulator. Each set has two independent roots and intermediate issuers, four endpoint key pairs and a seven-day validity period. Inspect `manifest.tsv` for the roles, leaf fingerprints, trust-anchor paths and reference time. The generator refuses to replace an existing directory.

```sh
cargo run --locked -p xtask -- gen-pki target/renewed-pki
cargo run --locked -p smgw-cls --features simulator --example mms_read -- target/renewed-pki
```

## Drive a session

Use `engine::Session` when your application owns the sockets and clock. The core builds with `default-features = false` and tracks one connection without allocating. [Session API](../../api/smgw_cls/engine/struct.Session.html)

1. Call `start(now)`. Execute the returned `Connect` action and keep its generation with that transport.
2. After peer verification and route authorization succeed, call `connected(generation, now)`. A timely `Ready` action permits application traffic. Report failure with `failed` and make authentication or configuration failures permanent.
3. After every change, re-arm the timer from `deadline()`. Supply monotonic time from one origin, including when the socket is idle. At the deadline call `tick(now)`, execute its action and recheck the deadline. A connect callback arriving exactly at its deadline is late.
4. On `Close`, release resources for that generation. On shutdown call `cancel()`. Older callbacks cannot activate a new connection.

The default policy gives each connect attempt five seconds, bounds an active session at sixty seconds, and allows three additional attempts with delays of one, two and four seconds. The one-shot MMS example uses its own fifteen-second overall budget.
