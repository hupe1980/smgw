+++
title = "Gateway wake-up verification"
description = "Inspect a gateway administrator’s datagram, verify its signature and freshness, and retain replay history across restarts."
weight = 5
+++

## Verify a management wake-up

For structural inspection, `smgw::wakeup::parse` returns borrowed, unverified fields. Enable `wakeup-verify` to check the gateway recipient, signature, algorithm, freshness and replay state. The bundled offline fixture supplies a trusted synthetic public key and fixed UTC time. [Decoder API](../../api/smgw/wakeup/index.html) · [Verifier API](../../api/smgw/wakeup/verify/struct.WakeUpVerifier.html)

```sh
cargo run --locked -p smgw --features wakeup-verify --example wakeup_verify -- --fixture
```

For a recording, supply a PEM public key obtained from an independently validated gateway administrator (GWA) signing certificate, the expected nine-byte gateway ID in hex, the UTC Unix time of receipt and the packet files in arrival order. Certificate authorization, validity, trust path and revocation belong to the provisioning application.

```sh
cargo run --locked -p smgw --features wakeup-verify --example wakeup_verify -- \
  crates/smgw/examples/fixtures/wakeup-gwa-public.pem 0e5453540000000001 1735689600 \
  crates/smgw/examples/fixtures/wakeup-v1.bin
```

The verification profile accepts ECDSA/SHA-256 on Brainpool P-256 with absolute clock skew below 30 seconds. One verifier accepts strictly increasing signed timestamps for its signer and recipient. A duplicate, an older packet or a second packet with the same timestamp fails. By default, replay history lasts for one verifier instance. The example accepts up to 16 files and performs no network I/O. [BSI wake-up format, §2.1](https://www.bsi.bund.de/SharedDocs/Downloads/DE/BSI/Publikationen/TechnischeRichtlinien/TR03109/TR-03109-1_Detailspezifikation_v2_0.pdf?__blob=publicationFile&v=2)

### Share replay history across processes

Enable `wakeup-store` and pass `--replay-db PATH` before the fixture or packet arguments. The first run below accepts the fixture. Running the verification command again with the same database rejects the duplicate.

```sh
mkdir -m 700 -p target/wakeup-state
cargo run --locked -p smgw --features wakeup-store --example wakeup_verify -- \
  --replay-db target/wakeup-state/replay.redb --fixture
```

All verifiers for a recipient must share the same trusted file, including after key rotation. Protect it against replacement or rollback. Concurrent processes accept a timestamp once. Each verifier has its own signature budget.

The redb store requires local byte-range locks on Linux, Apple platforms or Windows. Its 500 ms contention limit applies when acquiring a write transaction. Opening the file and completing disk I/O can take longer. [redb concurrency modes](https://docs.rs/redb/5.0.0/redb/enum.ConcurrencyMode.html)
