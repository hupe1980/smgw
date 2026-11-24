#!/usr/bin/env python3
"""Cross-check the public wake-up vector with the OpenSSL command-line tool."""
from pathlib import Path
import subprocess
import tempfile


def der_integer(raw: bytes) -> bytes:
    unsigned = raw.lstrip(b"\0") or b"\0"
    if unsigned[0] & 0x80:
        unsigned = b"\0" + unsigned
    return bytes([2, len(unsigned)]) + unsigned


def main() -> None:
    fixtures = Path(__file__).resolve().parents[1] / "crates/smgw/examples" / "fixtures"
    packet = (fixtures / "wakeup-v1.bin").read_bytes()
    # All fields outside the raw signature are fixed for this known-answer vector.
    prefix = (
        bytes.fromhex("5755010e5453540000000001")
        + (1735689600).to_bytes(8, "little")
        + bytes([11]) * 12
    )
    algorithm = bytes.fromhex("010a04007f00070101040103")
    if len(packet) != 108 or packet[:44] != prefix + algorithm:
        raise SystemExit("Unexpected fixture layout, recipient, timestamp, or algorithm")
    scalars = der_integer(packet[44:76]) + der_integer(packet[76:])
    with tempfile.TemporaryDirectory() as directory:
        signature = Path(directory) / "signature.der"
        signature.write_bytes(bytes([0x30, len(scalars)]) + scalars)
        subprocess.run(
            ["openssl", "dgst", "-sha256", "-verify",
             str(fixtures / "wakeup-gwa-public.pem"), "-signature", str(signature)],
            input=prefix, check=True,
        )


if __name__ == "__main__":
    main()
