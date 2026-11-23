# Synthetic management wake-up verification vector

The expected recipient is `0e5453540000000001` and the signed Unix timestamp is `1735689600` (2025-01-01 00:00:00 UTC). The fixture uses the little-endian timestamp specified in the text of table 2.1. The published hexadecimal timestamp example in that table has the opposite byte order. [BSI TR-03109-1 v2.0 detail specification, §2.1](https://www.bsi.bund.de/SharedDocs/Downloads/DE/BSI/Publikationen/TechnischeRichtlinien/TR03109/TR-03109-1_Detailspezifikation_v2_0.pdf?__blob=publicationFile&v=2)

The fixture was assembled independently of the Rust parser and verifier. OpenSSL 4.1.0 created an ephemeral Brainpool P-256 signing key and signed the 32-byte prefix using SHA-256. OpenSSL's `dgst -verify` verified the DER signature against the exported public key before its positive integer components were serialized at 32 bytes each in the binary vector. The algorithm OID is `0.4.0.127.0.7.1.1.4.1.3`. Only the SubjectPublicKeyInfo PEM and signed datagram were retained. Use this synthetic vector for offline regression testing. The verifier example selects the fixture's signed time explicitly so the result is reproducible.

| File | SHA-256 |
| --- | --- |
| `wakeup-v1.bin` | `41f2cb7eedb1cb709da746dd462344fa8772a234f85d82ecddaa53ffeabd1b0f` |
| `wakeup-gwa-public.pem` | `d223cf6fc8ed5879726283f4b9c74027277c3d22d240d147ce824a3e35a8a22c` |

From a repository checkout, run `python3 scripts/check-wakeup-fixture.py` for an OpenSSL CLI cross-check of the retained packet and public key.
