"""Minimal intrinsic-gated `zlib` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_ZLIB_RUNTIME_READY = _require_intrinsic("molt_zlib_runtime_ready", globals())
_MOLT_DEFLATE_RAW = _require_intrinsic("molt_deflate_raw", globals())
_MOLT_INFLATE_RAW = _require_intrinsic("molt_inflate_raw", globals())


class error(Exception):
    pass


def compress(data: bytes, level: int = -1) -> bytes:
    try:
        return bytes(_MOLT_DEFLATE_RAW(data, level))
    except Exception as exc:  # pragma: no cover - error mapping
        raise error(str(exc)) from exc


def decompress(data: bytes) -> bytes:
    try:
        return bytes(_MOLT_INFLATE_RAW(data))
    except Exception as exc:  # pragma: no cover - error mapping
        raise error(str(exc)) from exc


__all__ = ["compress", "decompress", "error"]
