# Changelog

## 0.3.0

- Extended the FNN bridge with production limits (LPP) and signed active-power readback from a grid connection point (MGCP) or individual device (MPC).

## 0.2.0

- Added an allocation-free CLS session engine with deadlines, cancellation and bounded retries.
- Added a mutually authenticated loopback gateway, a test PKI generator and an IEC 61850 MMS meter-read example.
- Added an adapter for mapping authorized IEC controls to EEBUS consumption limits.
- Added offline wake-up parsing and signature verification, with optional redb storage for replay protection across restarts and signer rotation.

## 0.1.0

- Published the initial reconnect-delay helper.
