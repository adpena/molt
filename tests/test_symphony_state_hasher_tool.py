from __future__ import annotations

import base64

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
