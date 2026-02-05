"""WASM-friendly DB client shims for Molt."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any
import asyncio as _asyncio

from _intrinsics import require_intrinsic as _intrinsics_require



_PENDING = getattr(_asyncio, "_PENDING", 0x7FFD_0000_0000_0000)

_molt_db_query_obj = _intrinsics_require("molt_db_query_obj", globals())
_molt_db_exec_obj = _intrinsics_require("molt_db_exec_obj", globals())
_molt_stream_recv = _intrinsics_require("molt_stream_recv", globals())
_molt_stream_drop = _intrinsics_require("molt_stream_drop", globals())
_molt_msgpack_parse_scalar_obj = _intrinsics_require(
    "molt_msgpack_parse_scalar_obj", globals()
)


@dataclass(frozen=True)
class DbResponse:
    status: str
    codec: str | None
    payload: bytes | None
    error: str | None
    metrics: dict[str, Any] | None


def _require_intrinsic(fn: Any, name: str) -> Any:
    if not callable(fn):
        raise RuntimeError(f"missing intrinsic: {name}")
    return fn


async def _recv_frame(handle: Any) -> bytes | None:
    recv = _require_intrinsic(_molt_stream_recv, "molt_stream_recv")
    while True:
        res = recv(handle)
        if res == _PENDING:
            await _asyncio.sleep(0.0)
            continue
        if res is None:
            return None
        if isinstance(res, (bytes, bytearray, memoryview)):
            return bytes(res)
        await _asyncio.sleep(0.0)


def _parse_header(payload: bytes) -> dict[str, Any]:
    parser = _require_intrinsic(
        _molt_msgpack_parse_scalar_obj, "molt_msgpack_parse_scalar_obj"
    )
    header = parser(payload)
    if not isinstance(header, dict):
        raise ValueError("db header must be a msgpack map")
    return header


async def _collect_response(handle: Any) -> DbResponse:
    drop = _require_intrinsic(_molt_stream_drop, "molt_stream_drop")
    try:
        header_frame = await _recv_frame(handle)
        if header_frame is None:
            raise RuntimeError("db stream closed before header")
        header = _parse_header(header_frame)
        status = str(header.get("status", ""))
        codec = header.get("codec")
        codec = str(codec) if codec is not None else None
        payload = header.get("payload")
        error = header.get("error")
        metrics = header.get("metrics")
        if error is not None and not isinstance(error, str):
            error = str(error)
        if metrics is not None and not isinstance(metrics, dict):
            metrics = None
        if codec == "arrow_ipc":
            buf = bytearray()
            while True:
                frame = await _recv_frame(handle)
                if frame is None:
                    break
                buf.extend(frame)
            payload_bytes = bytes(buf)
        else:
            if payload is None:
                payload_bytes = None
            elif isinstance(payload, (bytes, bytearray, memoryview)):
                payload_bytes = bytes(payload)
            else:
                payload_bytes = bytes(str(payload), "utf-8")
        return DbResponse(
            status=status,
            codec=codec,
            payload=payload_bytes,
            error=error,
            metrics=metrics,
        )
    finally:
        try:
            drop(handle)
        except Exception:
            pass


async def db_query(request: bytes, cancel_token: int | None = None) -> DbResponse:
    query = _require_intrinsic(_molt_db_query_obj, "molt_db_query_obj")
    handle = query(request, cancel_token)
    return await _collect_response(handle)


async def db_exec(request: bytes, cancel_token: int | None = None) -> DbResponse:
    exec_fn = _require_intrinsic(_molt_db_exec_obj, "molt_db_exec_obj")
    handle = exec_fn(request, cancel_token)
    return await _collect_response(handle)


__all__ = ["DbResponse", "db_query", "db_exec"]
