"""Exercise persistent replay protection through independently launched verifier processes."""

from concurrent.futures import ThreadPoolExecutor
import json
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def build_verifier():
    completed = subprocess.run(
        ["cargo", "build", "--locked", "-p", "smgw", "--features", "wakeup-store", "--example",
         "wakeup_verify", "--message-format=json-render-diagnostics"],
        cwd=ROOT, stdout=subprocess.PIPE, text=True, check=True,
    )
    for line in completed.stdout.splitlines():
        item = json.loads(line)
        if item["reason"] == "compiler-artifact" and item["target"]["name"] == "wakeup_verify":
            return item["executable"]
    raise RuntimeError("Cargo did not report an executable for wakeup_verify")


def verify(command):
    return subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=20)


def expect_replay(returncode, stderr):
    if returncode != 1 or "already accepted or superseded" not in stderr:
        raise RuntimeError(f"expected replay rejection, got exit {returncode}: {stderr}")


def check(executable, directory):
    packet = ROOT / "crates/smgw/examples/fixtures/wakeup-v1.bin"
    key = ROOT / "crates/smgw/examples/fixtures/wakeup-gwa-public.pem"
    archive = [str(key), "0e5453540000000001", "1735689600"]
    damaged = bytearray(packet.read_bytes())
    damaged[-1] ^= 1
    invalid = directory / "damaged.bin"
    invalid.write_bytes(damaged)
    unavailable = verify([executable, "--replay-db", str(directory), "--fixture"])
    if unavailable.returncode != 1 or "replay database" not in unavailable.stderr:
        raise RuntimeError(f"expected a storage error for a directory: {unavailable.stderr}")
    prefix = [executable, "--replay-db", str(directory / "restart.redb")]
    bad = verify([*prefix, *archive, str(invalid)])
    if bad.returncode != 1 or "invalid management signature" not in bad.stderr:
        raise RuntimeError(f"expected signature rejection: {bad.stderr}")
    verify([*prefix, *archive, str(packet)]).check_returncode()
    replay = verify([*prefix, "--fixture"])
    expect_replay(replay.returncode, replay.stderr)

    command = [executable, "--replay-db", str(directory / "concurrent.redb"), "--fixture"]
    with ThreadPoolExecutor(max_workers=2) as pool:
        results = sorted(pool.map(verify, [command, command]), key=lambda p: p.returncode)
    results[0].check_returncode()
    expect_replay(results[1].returncode, results[1].stderr)
    replay = verify(command)
    expect_replay(replay.returncode, replay.stderr)
    # Without the option, an invoker explicitly gets an independent in-memory history.
    for _ in range(2):
        verify([executable, "--fixture"]).check_returncode()


if __name__ == "__main__":
    verifier = build_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        check(verifier, Path(tmp))
    print("Wake-up replay checks passed for restart and concurrent processes.")
