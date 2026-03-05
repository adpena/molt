from __future__ import annotations

import argparse
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


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Hash Symphony state payload bytes into weak ETags. "
            "Supports one-shot and stdio streaming modes."
        )
    )
    parser.add_argument(
        "--payload-b64",
        default="",
        help="One-shot input payload bytes as base64.",
    )
    parser.add_argument(
        "--stdio",
        action="store_true",
        help="Run in streaming mode: one base64 payload per stdin line.",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    if bool(args.stdio):
        return _run_stdio()
    payload = _decode_payload(str(args.payload_b64 or ""))
    if payload is None:
        return 2
    sys.stdout.write(_etag_for_payload(payload))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
