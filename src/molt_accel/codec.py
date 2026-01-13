from __future__ import annotations

import base64
import json
from typing import Any

msgpack: Any | None
try:
    import msgpack as _msgpack  # type: ignore
except Exception:  # pragma: no cover - optional dependency
    msgpack = None
else:
    msgpack = _msgpack


class CodecUnavailableError(RuntimeError):
    pass


def choose_wire(preferred: str | None = None) -> str:
    if preferred:
        if preferred == "msgpack" and msgpack is None:
            raise CodecUnavailableError("msgpack is not installed")
        return preferred
    return "msgpack" if msgpack is not None else "json"


def encode_payload(obj: Any, codec: str) -> bytes:
    if codec in ("raw", "arrow_ipc"):
        if isinstance(obj, (bytes, bytearray)):
            return bytes(obj)
        raise TypeError("raw payloads must be bytes")
    if codec == "json":
        return json.dumps(obj, separators=(",", ":"), sort_keys=True).encode("utf-8")
    if codec == "msgpack":
        if msgpack is None:
            raise CodecUnavailableError("msgpack is not installed")
        return msgpack.packb(obj, use_bin_type=True)
    raise ValueError(f"Unknown codec '{codec}'")


def decode_payload(data: bytes, codec: str) -> Any:
    if codec in ("raw", "arrow_ipc"):
        return data
    if codec == "json":
        return json.loads(data.decode("utf-8"))
    if codec == "msgpack":
        if msgpack is None:
            raise CodecUnavailableError("msgpack is not installed")
        return msgpack.unpackb(data, raw=False)
    raise ValueError(f"Unknown codec '{codec}'")


def encode_message(message: dict[str, Any], wire: str) -> bytes:
    if wire == "msgpack":
        if msgpack is None:
            raise CodecUnavailableError("msgpack is not installed")
        return msgpack.packb(message, use_bin_type=True)
    if wire == "json":
        payload = message.get("payload")
        if isinstance(payload, (bytes, bytearray)):
            message = dict(message)
            message["payload_b64"] = base64.b64encode(payload).decode("ascii")
            message.pop("payload", None)
        return json.dumps(message, separators=(",", ":"), sort_keys=True).encode(
            "utf-8"
        )
    raise ValueError(f"Unknown wire codec '{wire}'")


def decode_message(data: bytes, wire: str) -> dict[str, Any]:
    if wire == "msgpack":
        if msgpack is None:
            raise CodecUnavailableError("msgpack is not installed")
        return msgpack.unpackb(data, raw=False)
    if wire == "json":
        message = json.loads(data.decode("utf-8"))
        if "payload_b64" in message:
            payload = base64.b64decode(message["payload_b64"])
            message["payload"] = payload
            message.pop("payload_b64", None)
        return message
    raise ValueError(f"Unknown wire codec '{wire}'")
