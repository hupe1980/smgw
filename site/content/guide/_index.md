+++
title = "Developer guide"
description = "Read power from a simulated meter over mutual TLS, integrate the session engine, map a load limit and verify an archived wake-up."
template = "guide.html"
+++

## Run the first read

From a repository checkout, use Rust 1.90 or newer for the full workspace. The simulator needs OpenSSL development headers and a provider supporting Brainpool P-256. On Linux, install the OpenSSL development package and `pkg-config`. On macOS, Homebrew's OpenSSL works.

```sh
cargo run --locked -p xtask -- gen-pki
cargo run --locked --features simulator --example mms_read
```

The example uses Manufacturing Message Specification (MMS) to read a Controllable Local System (CLS) through a simulated smart meter gateway. The first command creates credentials under `target/test-pki`. Run the second command as often as needed during their validity period. A successful run reads a named value from the bundled IEC 61850 model and completes the MMS release handshake.

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
cargo run --locked --features simulator --example mms_read -- target/renewed-pki
```

## Drive a session

Use `engine::Session` when your application owns the sockets and clock. The core builds with `default-features = false` and tracks one connection without allocating. [Session API](../api/smgw_cls/engine/struct.Session.html)

1. Call `start(now)`. Execute the returned `Connect` action and keep its generation with that transport.
2. After peer verification and route authorization succeed, call `connected(generation, now)`. A timely `Ready` action permits application traffic. Report failure with `failed` and make authentication or configuration failures permanent.
3. After every change, re-arm the timer from `deadline()`. Supply monotonic time from one origin, including when the socket is idle. At the deadline call `tick(now)`, execute its action and recheck the deadline. A connect callback arriving exactly at its deadline is late.
4. On `Close`, release resources for that generation. On shutdown call `cancel()`. Older callbacks cannot activate a new connection.

The default policy gives each connect attempt five seconds, bounds an active session at sixty seconds, and allows three additional attempts with delays of one, two and four seconds. The one-shot MMS example uses its own fifteen-second overall budget.

## Map a consumption limit

Enable `fnn-bridge` to bind two commissioned MMS control references to a single EEBUS device offering Limitation of Power Consumption (LPC) and a nonzero reference power in watts. The first reference accepts a floating-point percentage from 0 through 100. The second accepts an explicit boolean `true` to release the limit. A zero-percent limit keeps the limit active at zero watts. [LpcBridge API](../api/smgw_cls/bridge/struct.LpcBridge.html)

Pass an authorized, immediate `Operate` event, a monotonic admission deadline and the current time to `schedule`. The bridge checks the mapped object and ready consumption guard. Its receipt records the control number and locally scheduled watts. Observe the guard's later acknowledgement separately. In the test below, a simulated device acknowledges 40% of 5,000 W and the subsequent release while two SPINE actors exchange datagrams in memory.

```sh
cargo test --locked --features fnn-bridge --test lpc_bridge
```

## Verify a management wake-up

For structural inspection, `wakeup::parse` returns borrowed, unverified fields. Enable `wakeup-verify` to check the gateway recipient, signature, algorithm, freshness and replay state. The bundled offline fixture supplies a trusted synthetic public key and fixed UTC time. [Decoder API](../api/smgw_cls/wakeup/index.html) · [Verifier API](../api/smgw_cls/wakeup/verify/struct.WakeUpVerifier.html)

```sh
cargo run --locked --features wakeup-verify --example wakeup_verify -- --fixture
```

For a recording, supply a PEM public key obtained from an independently validated gateway administrator (GWA) signing certificate, the expected nine-byte gateway ID in hex, the UTC Unix time of receipt and the packet files in arrival order. Certificate authorization, validity, trust path and revocation belong to the provisioning application.

```sh
cargo run --locked --features wakeup-verify --example wakeup_verify -- \
  examples/fixtures/wakeup-gwa-public.pem 0e5453540000000001 1735689600 \
  examples/fixtures/wakeup-v1.bin
```

The verification profile accepts ECDSA/SHA-256 on Brainpool P-256 with absolute clock skew below 30 seconds. One verifier accepts strictly increasing signed timestamps for its signer and recipient. A duplicate, an older packet or a second packet with the same timestamp fails. By default, replay history lasts for one verifier instance. The example accepts up to 16 files and performs no network I/O. [BSI wake-up format, §2.1](https://www.bsi.bund.de/SharedDocs/Downloads/DE/BSI/Publikationen/TechnischeRichtlinien/TR03109/TR-03109-1_Detailspezifikation_v2_0.pdf?__blob=publicationFile&v=2)

### Share replay history across processes

Enable `wakeup-store` and pass `--replay-db PATH` before the fixture or packet arguments. The first run below accepts the fixture. Running the verification command again with the same database rejects the duplicate.

```sh
mkdir -m 700 -p target/wakeup-state
cargo run --locked --features wakeup-store --example wakeup_verify -- \
  --replay-db target/wakeup-state/replay.redb --fixture
```

All verifiers for a recipient must share the same trusted file, including after key rotation. Protect it against replacement or rollback. Concurrent processes accept a timestamp once. Each verifier has its own signature budget.

The redb store requires local byte-range locks on Linux, Apple platforms or Windows. Its 500 ms contention limit applies when acquiring a write transaction. Opening the file and completing disk I/O can take longer. [redb concurrency modes](https://docs.rs/redb/5.0.0/redb/enum.ConcurrencyMode.html)

## Diagnose a failure

| Symptom | What to check |
| --- | --- |
| Fixture directory already exists | Run the simulator against the existing directory, or generate a fresh directory to renew all identities together. |
| Certificate verification failed | Check the validity period and the four fingerprints in the same fixture manifest. The HAN and WAN gateway files serve different TLS roles. |
| Unknown CLS proxy route | Compare SNI with the provisioned proxy ID. An unknown route is rejected before any WAN connection opens. |
| No shared cipher or group | Check the OpenSSL library and provider linked by Rust. Run `cargo test --locked -p xtask --test transport` to exercise real handshakes. |
| Wake-up freshness or replay rejection | Use the trusted time of receipt for the recording and inspect packet ordering within the batch. For persistent verification, check the recipient’s history in the selected replay database. |
| Bridge reports an unavailable peer | Commission the mapped LPC device and wait for the consumption guard to be ready before submitting a fresh command. |

## Build the documentation

Install Zola 0.23.6 for the site build. From the checkout, generate the guide and Rust API together, then serve them locally.

```sh
scripts/build-site.sh
python3 -m http.server --directory target/site --bind 127.0.0.1 8000
```

Open `http://127.0.0.1:8000/` in a browser. To generate only the Rust API, use `cargo doc --locked -p smgw-cls --all-features --no-deps --open`. [Browse the Rust API](../api/smgw_cls/index.html)
