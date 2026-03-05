from __future__ import annotations

import base64
import hashlib
import sys


def _etag_for_payload(payload: bytes) -> str:
    digest = hashlib.blake2s(payload, digest_size=8).hexdigest()
    return f'W/"{digest}"'


def _decode_payload(payload_b64: str) -> bytes | None:
    text = payload_b64.strip()
    if not text:
        return None
    try:
        return base64.b64decode(text.encode("ascii"), validate=True)
    except (ValueError, UnicodeEncodeError):
        return None


def _run_stdio() -> int:
    for line in sys.stdin:
        payload = _decode_payload(line)
        if payload is None:
            sys.stdout.write("ERR\n")
            sys.stdout.flush()
            continue
        sys.stdout.write(_etag_for_payload(payload) + "\n")
        sys.stdout.flush()
    return 0


def _parse_args(argv: list[str]) -> tuple[bool, str]:
    stdio = False
    payload_b64 = ""
    idx = 0
    while idx < len(argv):
        arg = str(argv[idx])
        if arg == "--stdio":
            stdio = True
            idx += 1
            continue
        if arg == "--payload-b64":
            if idx + 1 >= len(argv):
                return False, ""
            payload_b64 = str(argv[idx + 1])
            idx += 2
            continue
        if arg.startswith("--payload-b64="):
            payload_b64 = arg.split("=", 1)[1]
            idx += 1
            continue
        # Unknown flags are treated as invalid input.
        return False, ""
    return stdio, payload_b64


def main(argv: list[str] | None = None) -> int:
    args = list(argv or [])
    parsed = _parse_args(args)
    if parsed == (False, "") and args:
        return 2
    stdio, payload_b64 = parsed
    if stdio:
        return _run_stdio()
    payload = _decode_payload(payload_b64)
    if payload is None:
        return 2
    sys.stdout.write(_etag_for_payload(payload))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
