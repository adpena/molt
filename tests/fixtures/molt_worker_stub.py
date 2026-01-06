from __future__ import annotations

import os
import sys

from molt_accel.codec import (
    decode_message,
    decode_payload,
    encode_message,
    encode_payload,
)
from molt_accel.framing import read_frame, write_frame


WIRE = os.environ.get("MOLT_WIRE") or None


def main() -> None:
    stdin = sys.stdin.buffer
    stdout = sys.stdout.buffer
    while True:
        try:
            frame = read_frame(stdin)
        except EOFError:
            break
        message = decode_message(frame, WIRE or "json")
        request_id = message.get("request_id", 0)
        entry = message.get("entry")
        codec = message.get("codec", "raw")
        payload = message.get("payload", b"")
        status = "Ok"
        error = ""
        response_payload = b""
        response_codec = codec

        if entry == "__ping__":
            response_payload = b""
            response_codec = "raw"
        elif entry == "echo":
            response_payload = payload
        elif entry == "list_items":
            try:
                req = decode_payload(payload, codec)
                response = {
                    "items": [],
                    "next_cursor": None,
                    "counts": {"open": 0, "closed": 0},
                    "request": req,
                }
                response_payload = encode_payload(response, codec)
            except Exception as exc:  # pragma: no cover - defensive
                status = "InvalidInput"
                error = str(exc)
        elif entry == "__error__":
            status = "InternalError"
            error = "boom"
        else:
            status = "InvalidInput"
            error = f"Unknown entry '{entry}'"

        response = {
            "request_id": request_id,
            "status": status,
            "codec": response_codec,
            "payload": response_payload,
        }
        if error:
            response["error"] = error
        wire = WIRE or "json"
        write_frame(stdout, encode_message(response, wire))


if __name__ == "__main__":
    main()
