from __future__ import annotations

import base64
import hashlib
import subprocess
import sys

import tools.symphony_state_hasher as hasher


def test_main_one_shot_payload_b64() -> None:
    payload = b"abc"
    payload_b64 = base64.b64encode(payload).decode("ascii")
    rc = hasher.main(["--payload-b64", payload_b64])
    assert rc == 0


def test_parse_args_rejects_unknown_flags() -> None:
    rc = hasher.main(["--unknown-flag"])
    assert rc == 2


def test_parse_args_accepts_equals_form() -> None:
    payload = b"xyz"
    payload_b64 = base64.b64encode(payload).decode("ascii")
    rc = hasher.main([f"--payload-b64={payload_b64}"])
    assert rc == 0


def test_stdio_frame_roundtrip_digest_bytes() -> None:
    payload = b"frame-mode-payload"
    proc = subprocess.Popen(
        [sys.executable, "tools/symphony_state_hasher.py", "--stdio-frame"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        assert proc.stdin is not None
        assert proc.stdout is not None
        proc.stdin.write(len(payload).to_bytes(4, "big"))
        proc.stdin.write(payload)
        proc.stdin.flush()
        digest = proc.stdout.read(8)
        expected = hashlib.blake2s(payload, digest_size=8).digest()
        assert digest == expected
    finally:
        if proc.stdin is not None:
            proc.stdin.close()
        proc.terminate()
        proc.wait(timeout=2.0)
