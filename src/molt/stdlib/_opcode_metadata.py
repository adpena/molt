"""Intrinsic-backed `_opcode_metadata` payload for Python 3.14+."""

from __future__ import annotations

import json as _json
import sys as _sys

from _intrinsics import require_intrinsic as _require_intrinsic

if _sys.version_info < (3, 14):
    raise ModuleNotFoundError("No module named '_opcode_metadata'")

_MOLT_OPCODE_METADATA_PAYLOAD_314_JSON = _require_intrinsic(
    "molt_opcode_metadata_payload_314_json", globals()
)


def _expect_dict(value: object, label: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise RuntimeError(f"invalid _opcode_metadata payload field: {label}")
    return value


def _expect_int(value: object, label: str) -> int:
    if not isinstance(value, int):
        raise RuntimeError(f"invalid _opcode_metadata payload field: {label}")
    return value


def _expect_list(value: object, label: str) -> list[object]:
    if not isinstance(value, list):
        raise RuntimeError(f"invalid _opcode_metadata payload field: {label}")
    return value


def _load_payload() -> dict[str, object]:
    payload_json = _MOLT_OPCODE_METADATA_PAYLOAD_314_JSON()
    if not isinstance(payload_json, str):
        raise RuntimeError("invalid _opcode_metadata payload: expected JSON string")
    payload = _json.loads(payload_json)
    if not isinstance(payload, dict):
        raise RuntimeError("invalid _opcode_metadata payload: expected JSON object")
    return payload


_PAYLOAD = _load_payload()

HAVE_ARGUMENT = _expect_int(_PAYLOAD.get("HAVE_ARGUMENT"), "HAVE_ARGUMENT")
MIN_INSTRUMENTED_OPCODE = _expect_int(
    _PAYLOAD.get("MIN_INSTRUMENTED_OPCODE"), "MIN_INSTRUMENTED_OPCODE"
)
opmap = {
    str(k): int(v) for k, v in _expect_dict(_PAYLOAD.get("opmap"), "opmap").items()
}
_specialized_opmap = {
    str(k): int(v)
    for k, v in _expect_dict(
        _PAYLOAD.get("_specialized_opmap"), "_specialized_opmap"
    ).items()
}
_specializations = {
    str(k): [str(x) for x in _expect_list(v, f"_specializations.{k}")]
    for k, v in _expect_dict(
        _PAYLOAD.get("_specializations"), "_specializations"
    ).items()
}

del _PAYLOAD
