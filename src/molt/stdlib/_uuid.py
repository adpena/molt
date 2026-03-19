"""Intrinsic-backed compatibility surface for CPython's `_uuid`."""

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_UUID_UUID1_BYTES = _require_intrinsic("molt_uuid_uuid1_bytes")


def generate_time_safe():
    payload = _MOLT_UUID_UUID1_BYTES(None, None)
    if not isinstance(payload, (bytes, bytearray, memoryview)):
        raise RuntimeError("invalid uuid1 intrinsic payload")
    data = bytes(payload)
    if len(data) != 16:
        raise RuntimeError("invalid uuid1 intrinsic payload")
    return data, None


has_stable_extractable_node = 0
has_uuid_generate_time_safe = 0

__all__ = [
    "generate_time_safe",
    "has_stable_extractable_node",
    "has_uuid_generate_time_safe",
]

globals().pop("_require_intrinsic", None)
