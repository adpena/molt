from __future__ import annotations

import ast
import hashlib
import json
from typing import Any, Iterable, Iterator, Mapping

from molt.frontend import MoltValue

from molt.cli.cache_fingerprints import _cache_fingerprint, _cache_tooling_fingerprint


_CACHE_KEY_SCHEMA_VERSION = "v4"
_FUNCTION_CACHE_KEY_SCHEMA_VERSION = "func-v2"


def _json_ir_default(value: Any) -> Any:
    if isinstance(value, complex):
        return {"__complex__": [value.real, value.imag]}
    if value is Ellipsis:
        return {"__ellipsis__": True}
    if isinstance(value, bytes):
        return {"__bytes__": list(value)}
    if isinstance(value, tuple):
        return {
            "__tuple__": [
                _json_ir_default(item)
                if not isinstance(item, (str, int, float, bool, type(None), list, dict))
                else item
                for item in value
            ]
        }
    if isinstance(value, ast.AST):
        return {"__ast__": ast.dump(value, include_attributes=False)}
    if isinstance(value, (set, frozenset)):
        try:
            items = sorted(value)
        except TypeError:
            items = sorted((repr(item) for item in value))
        return {"__set__": items}
    if isinstance(value, MoltValue):
        return {
            "__molt_value__": {
                "name": value.name,
                "type_hint": value.type_hint,
            }
        }
    raise TypeError(f"Object of type {type(value).__name__} is not JSON serializable")


def _cache_ir_payload_ir(ir: Mapping[str, Any]) -> dict[str, Any]:
    funcs = ir.get("functions")
    normalized: dict[str, Any] = dict(ir)
    if isinstance(funcs, list):
        normalized["functions"] = _sorted_ir_functions(funcs)
    return normalized


def _sorted_ir_functions(functions: list[Any]) -> list[Any]:
    def _func_sort_key(entry: Any) -> str:
        if isinstance(entry, dict):
            name = entry.get("name")
            if isinstance(name, str):
                return name
        return ""

    return sorted(functions, key=_func_sort_key)


def _cache_backend_payload_ir(ir: Mapping[str, Any]) -> dict[str, Any]:
    functions = ir.get("functions")
    sorted_funcs: list[Any] = []
    if isinstance(functions, list):
        sorted_funcs = _sorted_ir_functions(functions)

    return {
        "functions": sorted_funcs,
        "profile": ir.get("profile"),
        "top_level_extras_digest": _ir_top_level_extras_digest(ir),
    }


def _iter_cache_json_payload_bytes(payload_ir: Mapping[str, Any]) -> Iterator[bytes]:
    encoder = json.JSONEncoder(
        sort_keys=True,
        separators=(",", ":"),
        default=_json_ir_default,
    )
    for chunk in encoder.iterencode(payload_ir):
        yield chunk.encode("utf-8")


def _cache_key_for_json_payload_bytes(
    payload_chunks: Iterable[bytes],
    *,
    target: str,
    target_triple: str | None,
    variant: str,
    schema_version: str,
) -> str:
    suffix = target_triple or target
    if variant:
        suffix = f"{suffix}:{variant}"
    digest = hashlib.sha256()
    for chunk in payload_chunks:
        digest.update(chunk)
    digest.update(b"|")
    digest.update(suffix.encode("utf-8"))
    digest.update(b"|")
    digest.update(_cache_fingerprint().encode("utf-8"))
    digest.update(b"|")
    digest.update(_cache_tooling_fingerprint().encode("utf-8"))
    digest.update(b"|")
    digest.update(schema_version.encode("utf-8"))
    return digest.hexdigest()


def _cache_key_for_payload_ir(
    payload_ir: Mapping[str, Any],
    *,
    target: str,
    target_triple: str | None,
    variant: str,
    schema_version: str,
) -> str:
    return _cache_key_for_json_payload_bytes(
        _iter_cache_json_payload_bytes(payload_ir),
        target=target,
        target_triple=target_triple,
        variant=variant,
        schema_version=schema_version,
    )


def _cache_key(
    ir: Mapping[str, Any],
    target: str,
    target_triple: str | None,
    variant: str = "",
    payload_ir: Mapping[str, Any] | None = None,
) -> str:
    if payload_ir is None:
        payload_ir = _cache_ir_payload_ir(ir)
    return _cache_key_for_payload_ir(
        payload_ir,
        target=target,
        target_triple=target_triple,
        variant=variant,
        schema_version=_CACHE_KEY_SCHEMA_VERSION,
    )


def _ir_top_level_extras_digest(ir: Mapping[str, Any]) -> str:
    extras = {
        key: value for key, value in ir.items() if key not in {"functions", "profile"}
    }
    encoded = json.dumps(
        extras, sort_keys=True, separators=(",", ":"), default=_json_ir_default
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _backend_ir_text(ir: Mapping[str, Any]) -> str:
    return json.dumps(ir, separators=(",", ":"), default=_json_ir_default)


def _backend_ir_bytes(ir: Mapping[str, Any]) -> bytes:
    return _backend_ir_text(ir).encode("utf-8")


def _backend_ir_format_and_bytes(ir: dict[str, Any]) -> tuple[str, bytes]:
    try:
        import msgpack  # type: ignore[import-untyped]

        return "msgpack", msgpack.packb(ir)
    except ImportError:
        return "json", _backend_ir_bytes(ir)


def _function_cache_key(
    ir: Mapping[str, Any],
    target: str,
    target_triple: str | None,
    variant: str = "",
    payload_ir: Mapping[str, Any] | None = None,
) -> str:
    if payload_ir is None:
        payload_ir = _cache_backend_payload_ir(ir)
    return _cache_key_for_payload_ir(
        payload_ir,
        target=target,
        target_triple=target_triple,
        variant=variant,
        schema_version=_FUNCTION_CACHE_KEY_SCHEMA_VERSION,
    )
