"""Token constants."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["tok_name", "ISTERMINAL", "ISNONTERMINAL", "ISEOF", "EXACT_TOKEN_TYPES"]

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)
_MOLT_TOKEN_PAYLOAD_312 = _require_intrinsic("molt_token_payload_312", globals())

_MOLT_IMPORT_SMOKE_RUNTIME_READY()

_TOKEN_PAYLOAD_SCHEMA = "molt.token_payload.312.v1"
_TOKEN_PAYLOAD_MINOR = "3.12"


def _expect_dict(value: object, label: str) -> dict:
    if not isinstance(value, dict):
        raise RuntimeError(f"invalid token payload field: {label}")
    return value


def _expect_list(value: object, label: str) -> list:
    if not isinstance(value, list):
        raise RuntimeError(f"invalid token payload field: {label}")
    return value


def _expect_int(value: object, label: str) -> int:
    if type(value) is not int:
        raise RuntimeError(f"invalid token payload field: {label}")
    return value


def _expect_str(value: object, label: str) -> str:
    if not isinstance(value, str):
        raise RuntimeError(f"invalid token payload field: {label}")
    return value


def _to_int_key(value: object, label: str) -> int:
    if type(value) is int:
        return value
    if isinstance(value, str):
        try:
            parsed = int(value)
        except ValueError as exc:
            raise RuntimeError(f"invalid token payload field: {label}") from exc
        if str(parsed) != value:
            raise RuntimeError(f"invalid token payload field: {label}")
        return parsed
    raise RuntimeError(f"invalid token payload field: {label}")


def _load_payload() -> dict:
    payload = _MOLT_TOKEN_PAYLOAD_312()
    if not isinstance(payload, dict):
        raise RuntimeError("invalid token payload: expected dict payload")
    if _expect_str(payload.get("_payload_schema"), "_payload_schema") != (
        _TOKEN_PAYLOAD_SCHEMA
    ):
        raise RuntimeError("invalid token payload field: _payload_schema")
    if (
        _expect_str(payload.get("_python_minor"), "_python_minor")
        != _TOKEN_PAYLOAD_MINOR
    ):
        raise RuntimeError("invalid token payload field: _python_minor")
    return payload


def _load_constants(payload: dict) -> tuple[dict, list]:
    constants_obj = _expect_dict(payload.get("constants"), "constants")
    order = _expect_list(payload.get("constant_order"), "constant_order")
    if not order:
        raise RuntimeError("invalid token payload field: constant_order")
    seen: set[str] = set()
    for raw_name in order:
        name = _expect_str(raw_name, "constant_order[]")
        if name in seen:
            raise RuntimeError(f"invalid token payload field: constant_order.{name}")
        seen.add(name)
        _expect_int(constants_obj.get(name), f"constants.{name}")
    return constants_obj, order


def _load_tok_name(payload: dict, constants: dict) -> dict[int, str]:
    tok_name_obj = _expect_dict(payload.get("tok_name"), "tok_name")
    tok_name_map: dict[int, str] = {}
    for raw_token_id, raw_name in tok_name_obj.items():
        token_id = _to_int_key(raw_token_id, "tok_name.key")
        name = _expect_str(raw_name, f"tok_name.{token_id}")
        expected_name = constants.get(name)
        if type(expected_name) is not int or expected_name != token_id:
            raise RuntimeError(f"invalid token payload field: tok_name.{token_id}")
        tok_name_map[token_id] = name
    return tok_name_map


def _load_exact_token_types(
    payload: dict, token_names: dict[int, str]
) -> dict[str, int]:
    exact_obj = _expect_dict(payload.get("EXACT_TOKEN_TYPES"), "EXACT_TOKEN_TYPES")
    exact_token_types: dict[str, int] = {}
    for symbol, value in exact_obj.items():
        symbol_name = _expect_str(symbol, "EXACT_TOKEN_TYPES.key")
        token_id = _expect_int(value, f"EXACT_TOKEN_TYPES.{symbol_name}")
        if token_id not in token_names:
            raise RuntimeError(
                f"invalid token payload field: EXACT_TOKEN_TYPES.{symbol_name}"
            )
        exact_token_types[symbol_name] = token_id
    if not exact_token_types:
        raise RuntimeError("invalid token payload field: EXACT_TOKEN_TYPES")
    return exact_token_types


_PAYLOAD = _load_payload()
_CONSTANTS, _CONSTANT_ORDER = _load_constants(_PAYLOAD)

import sys as _sys
_mod_dict = getattr(_sys.modules.get(__name__), "__dict__", None) or globals()
del _sys

for _constant_name in _CONSTANT_ORDER:
    _mod_dict[_constant_name] = _expect_int(
        _CONSTANTS.get(_constant_name), f"constants.{_constant_name}"
    )

NT_OFFSET = _expect_int(_CONSTANTS.get("NT_OFFSET"), "constants.NT_OFFSET")
ENDMARKER = _expect_int(_CONSTANTS.get("ENDMARKER"), "constants.ENDMARKER")

tok_name = _load_tok_name(_PAYLOAD, _CONSTANTS)
__all__.extend(_CONSTANT_ORDER)

EXACT_TOKEN_TYPES = _load_exact_token_types(_PAYLOAD, tok_name)


def ISTERMINAL(x: int) -> bool:
    return x < NT_OFFSET


def ISNONTERMINAL(x: int) -> bool:
    return x >= NT_OFFSET


def ISEOF(x: int) -> bool:
    return x == ENDMARKER
