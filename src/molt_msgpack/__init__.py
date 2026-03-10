"""Molt MsgPack/CBOR parsing via runtime intrinsics."""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

try:
    _MOLT_MSGPACK_PARSE_SCALAR_OBJ = _require_intrinsic(
        "molt_msgpack_parse_scalar_obj", globals()
    )
except RuntimeError:
    _MOLT_MSGPACK_PARSE_SCALAR_OBJ = None

try:
    _MOLT_CBOR_PARSE_SCALAR_OBJ = _require_intrinsic(
        "molt_cbor_parse_scalar_obj", globals()
    )
except RuntimeError:
    _MOLT_CBOR_PARSE_SCALAR_OBJ = None


def _require_msgpack_module() -> Any:
    try:
        import msgpack  # type: ignore[import-not-found]
    except Exception as exc:  # pragma: no cover - environment dependent
        raise RuntimeError("msgpack is required for parse_msgpack fallback") from exc
    return msgpack


def _require_cbor_module() -> Any:
    try:
        import cbor2  # type: ignore[import-not-found]
    except Exception as exc:  # pragma: no cover - environment dependent
        raise RuntimeError("cbor2 is required for parse_cbor fallback") from exc
    return cbor2


def parse_msgpack(data: bytes) -> Any:
    if _MOLT_MSGPACK_PARSE_SCALAR_OBJ is not None:
        return _MOLT_MSGPACK_PARSE_SCALAR_OBJ(data)
    # Tooling-only CPython baseline path; compiled Molt binaries always use intrinsics.
    msgpack = _require_msgpack_module()
    return msgpack.unpackb(data, raw=False)


def parse_cbor(data: bytes) -> Any:
    if _MOLT_CBOR_PARSE_SCALAR_OBJ is not None:
        return _MOLT_CBOR_PARSE_SCALAR_OBJ(data)
    # Tooling-only CPython baseline path; compiled Molt binaries always use intrinsics.
    cbor2 = _require_cbor_module()
    return cbor2.loads(data)


def parse(data: bytes) -> Any:
    return parse_msgpack(data)
