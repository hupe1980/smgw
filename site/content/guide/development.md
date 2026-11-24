+++
title = "Build and troubleshoot"
description = "Diagnose fixture, handshake, and bridge errors. Build and preview the guide and both Rust API references locally."
weight = 6
+++

## Diagnose a failure

| Symptom | What to check |
| --- | --- |
| Fixture directory already exists | Run the simulator against the existing directory, or generate a fresh directory to renew all identities together. |
| Certificate verification failed | Check the validity period and the four fingerprints in the same fixture manifest. The HAN and WAN gateway files serve different TLS roles. |
| Unknown CLS proxy route | Compare SNI with the provisioned proxy ID. An unknown route is rejected before any WAN connection opens. |
| No shared cipher or group | Check the OpenSSL library and provider linked by Rust. Run `cargo test --locked -p xtask --test transport` to exercise real handshakes. |
| Wake-up freshness or replay rejection | Use the trusted time of receipt for the recording and inspect packet ordering within the batch. For persistent verification, check the recipient’s history in the selected replay database. |
| Bridge reports an unavailable peer | Check the mapped device, direction and bindings. For LPP, confirm the peer advertises one matching writable production limit in watts. |

## Build the documentation

With Zola 0.23.6 installed, preview the guide and both Rust API references from the workspace root.

```sh
just serve
```

Open `http://127.0.0.1:8000/`, or use `just serve 8080` to choose another port. Zola reloads the browser when you edit a guide, template or stylesheet. After a Rust API change, press Ctrl-C and restart the preview to rebuild rustdoc.

Run `just site` to build and check the publishable site in `target/site/`. [Gateway API](../../api/smgw/index.html) · [CLS API](../../api/smgw_cls/index.html)
