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


def _read_exact(stream: object, count: int) -> bytes | None:
    if count <= 0:
        return b""
    read = getattr(stream, "read", None)
    if not callable(read):
        return None
    out = bytearray()
    while len(out) < count:
        chunk = read(count - len(out))
        if not isinstance(chunk, (bytes, bytearray)):
            return None
        if not chunk:
            return None
        out.extend(chunk)
    return bytes(out)


def _run_stdio_frame() -> int:
    stdin = sys.stdin.buffer
    stdout = sys.stdout.buffer
    while True:
        header = stdin.read(4)
        if not header:
            return 0
        if len(header) != 4:
            return 2
        size = int.from_bytes(header, "big", signed=False)
        payload = _read_exact(stdin, size)
        if payload is None:
            return 2
        digest = hashlib.blake2s(payload, digest_size=8).digest()
        stdout.write(digest)
        stdout.flush()


def _parse_args(argv: list[str]) -> tuple[str, str]:
    stdio = False
    stdio_frame = False
    payload_b64 = ""
    idx = 0
    while idx < len(argv):
        arg = str(argv[idx])
        if arg == "--stdio":
            stdio = True
            idx += 1
            continue
        if arg == "--stdio-frame":
            stdio_frame = True
            idx += 1
            continue
        if arg == "--payload-b64":
            if idx + 1 >= len(argv):
                return "invalid", ""
            payload_b64 = str(argv[idx + 1])
            idx += 2
            continue
        if arg.startswith("--payload-b64="):
            payload_b64 = arg.split("=", 1)[1]
            idx += 1
            continue
        # Unknown flags are treated as invalid input.
        return "invalid", ""
    if stdio and stdio_frame:
        return "invalid", ""
    if stdio:
        return "stdio", payload_b64
    if stdio_frame:
        return "stdio-frame", payload_b64
    return "oneshot", payload_b64


def main(argv: list[str] | None = None) -> int:
    args = list(argv or [])
    mode, payload_b64 = _parse_args(args)
    if mode == "invalid":
        return 2
    if mode == "stdio":
        return _run_stdio()
    if mode == "stdio-frame":
        return _run_stdio_frame()
    payload = _decode_payload(payload_b64)
    if payload is None:
        return 2
    sys.stdout.write(_etag_for_payload(payload))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
