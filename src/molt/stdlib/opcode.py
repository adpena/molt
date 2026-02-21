"""
opcode module - potentially shared between dis and other modules which
operate on bytecodes (e.g. peephole optimizers).
"""

from __future__ import annotations

import json as _json

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = [
    "cmp_op",
    "hasarg",
    "hasconst",
    "hasname",
    "hasjrel",
    "hasjabs",
    "haslocal",
    "hascompare",
    "hasfree",
    "hasexc",
    "opname",
    "opmap",
    "HAVE_ARGUMENT",
    "EXTENDED_ARG",
]

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)
_MOLT_OPCODE_PAYLOAD_312_JSON = _require_intrinsic(
    "molt_opcode_payload_312_json", globals()
)

_MOLT_IMPORT_SMOKE_RUNTIME_READY()


__all__.append("stack_effect")


def _expect_dict(value: object, label: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise RuntimeError(f"invalid opcode payload field: {label}")
    return value


def _expect_list(value: object, label: str) -> list[object]:
    if not isinstance(value, list):
        raise RuntimeError(f"invalid opcode payload field: {label}")
    return value


def _expect_int(value: object, label: str) -> int:
    if not isinstance(value, int):
        raise RuntimeError(f"invalid opcode payload field: {label}")
    return value


def _load_payload() -> dict[str, object]:
    payload_json = _MOLT_OPCODE_PAYLOAD_312_JSON()
    if not isinstance(payload_json, str):
        raise RuntimeError("invalid opcode payload: expected JSON string")
    payload = _json.loads(payload_json)
    if not isinstance(payload, dict):
        raise RuntimeError("invalid opcode payload: expected JSON object")
    return payload


_PAYLOAD = _load_payload()

cmp_op = tuple(_expect_list(_PAYLOAD.get("cmp_op"), "cmp_op"))
hasarg = [int(v) for v in _expect_list(_PAYLOAD.get("hasarg"), "hasarg")]
hasconst = [int(v) for v in _expect_list(_PAYLOAD.get("hasconst"), "hasconst")]
hasname = [int(v) for v in _expect_list(_PAYLOAD.get("hasname"), "hasname")]
hasjrel = [int(v) for v in _expect_list(_PAYLOAD.get("hasjrel"), "hasjrel")]
hasjabs = [int(v) for v in _expect_list(_PAYLOAD.get("hasjabs"), "hasjabs")]
haslocal = [int(v) for v in _expect_list(_PAYLOAD.get("haslocal"), "haslocal")]
hascompare = [int(v) for v in _expect_list(_PAYLOAD.get("hascompare"), "hascompare")]
hasfree = [int(v) for v in _expect_list(_PAYLOAD.get("hasfree"), "hasfree")]
hasexc = [int(v) for v in _expect_list(_PAYLOAD.get("hasexc"), "hasexc")]

opname = [str(v) for v in _expect_list(_PAYLOAD.get("opname"), "opname")]
opmap = {
    str(k): int(v) for k, v in _expect_dict(_PAYLOAD.get("opmap"), "opmap").items()
}

HAVE_ARGUMENT = _expect_int(_PAYLOAD.get("HAVE_ARGUMENT"), "HAVE_ARGUMENT")
EXTENDED_ARG = _expect_int(_PAYLOAD.get("EXTENDED_ARG"), "EXTENDED_ARG")
MIN_INSTRUMENTED_OPCODE = _expect_int(
    _PAYLOAD.get("MIN_INSTRUMENTED_OPCODE"), "MIN_INSTRUMENTED_OPCODE"
)
MIN_PSEUDO_OPCODE = _expect_int(_PAYLOAD.get("MIN_PSEUDO_OPCODE"), "MIN_PSEUDO_OPCODE")
MAX_PSEUDO_OPCODE = _expect_int(_PAYLOAD.get("MAX_PSEUDO_OPCODE"), "MAX_PSEUDO_OPCODE")
ENABLE_SPECIALIZATION = bool(_PAYLOAD.get("ENABLE_SPECIALIZATION"))

_nb_ops = [tuple(v) for v in _expect_list(_PAYLOAD.get("_nb_ops"), "_nb_ops")]
_intrinsic_1_descs = [
    str(v)
    for v in _expect_list(_PAYLOAD.get("_intrinsic_1_descs"), "_intrinsic_1_descs")
]
_intrinsic_2_descs = [
    str(v)
    for v in _expect_list(_PAYLOAD.get("_intrinsic_2_descs"), "_intrinsic_2_descs")
]
_specializations = {
    str(k): [str(x) for x in _expect_list(v, f"_specializations.{k}")]
    for k, v in _expect_dict(
        _PAYLOAD.get("_specializations"), "_specializations"
    ).items()
}
_specialized_instructions = [
    str(v)
    for v in _expect_list(
        _PAYLOAD.get("_specialized_instructions"), "_specialized_instructions"
    )
]
_cache_format = {
    str(k): {
        str(inner_k): int(inner_v) for inner_k, inner_v in _expect_dict(v, k).items()
    }
    for k, v in _expect_dict(_PAYLOAD.get("_cache_format"), "_cache_format").items()
}
_inline_cache_entries = [
    int(v)
    for v in _expect_list(
        _PAYLOAD.get("_inline_cache_entries"), "_inline_cache_entries"
    )
]

del _PAYLOAD


def is_pseudo(op):
    return MIN_PSEUDO_OPCODE <= op <= MAX_PSEUDO_OPCODE
