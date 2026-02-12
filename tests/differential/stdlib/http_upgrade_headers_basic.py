"""Purpose: differential coverage for WebSocket handshake accept key."""

import base64
import hashlib

_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


def _accept_key(key: str) -> str:
    raw = (key + _GUID).encode("ascii")
    digest = hashlib.sha1(raw).digest()
    return base64.b64encode(digest).decode("ascii")


example_key = "dGhlIHNhbXBsZSBub25jZQ=="
print(_accept_key(example_key))
