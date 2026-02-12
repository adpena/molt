"""Capability-gated multiprocessing support for Molt."""

from __future__ import annotations

from collections.abc import Iterable, Iterator, Sequence
from dataclasses import dataclass
import builtins as _builtins
import importlib
import os
import sys
import time
from typing import Any, Callable, cast

from _intrinsics import require_intrinsic as _intrinsics_require


def _require_intrinsic(func: Any | None, name: str) -> Callable[..., Any]:
    if not callable(func):
        raise RuntimeError(f"Missing intrinsic: {name}")
    return cast(Callable[..., Any], func)


__all__ = [
    "Array",
    "Pool",
    "Process",
    "Queue",
    "Pipe",
    "TimeoutError",
    "Value",
    "get_all_start_methods",
    "get_context",
    "get_start_method",
    "set_start_method",
]

_STDIO_INHERIT = 0
_STDIO_PIPE = 1
_STDIO_DEVNULL = 2

_ENTRY_OVERRIDE_ENV = "MOLT_ENTRY_MODULE"
_MP_ENTRY_ENV = "MOLT_MP_ENTRY"
_MP_MAIN_PATH_ENV = "MOLT_MP_MAIN_PATH"
_MP_SPAWN_ENV = "MOLT_MP_SPAWN"
_MP_START_METHOD_ENV = "MOLT_MP_START_METHOD"
_MP_ENTRY_OVERRIDE = "multiprocessing.spawn"

_MOLT_PROCESS_SPAWN = _intrinsics_require("molt_process_spawn", globals())
_MOLT_PROCESS_WAIT = _intrinsics_require("molt_process_wait_future", globals())
_MOLT_PROCESS_POLL = _intrinsics_require("molt_process_poll", globals())
_MOLT_PROCESS_PID = _intrinsics_require("molt_process_pid", globals())
_MOLT_PROCESS_RETURN = _intrinsics_require("molt_process_returncode", globals())
_MOLT_PROCESS_KILL = _intrinsics_require("molt_process_kill", globals())
_MOLT_PROCESS_TERM = _intrinsics_require("molt_process_terminate", globals())
_MOLT_PROCESS_STDIN = _intrinsics_require("molt_process_stdin", globals())
_MOLT_PROCESS_STDOUT = _intrinsics_require("molt_process_stdout", globals())
_MOLT_PROCESS_STDERR = _intrinsics_require("molt_process_stderr", globals())
_MOLT_PROCESS_DROP = _intrinsics_require("molt_process_drop", globals())
_MOLT_STREAM_SEND = _intrinsics_require("molt_stream_send_obj", globals())
_MOLT_STREAM_RECV = _intrinsics_require("molt_stream_recv", globals())
_MOLT_STREAM_CLOSE = _intrinsics_require("molt_stream_close", globals())
_MOLT_STREAM_DROP = _intrinsics_require("molt_stream_drop", globals())
_MOLT_ENV_GET = _intrinsics_require("molt_env_get", globals())
_MOLT_ENV_SNAPSHOT = _intrinsics_require("molt_env_snapshot", globals())
_MOLT_PENDING = _intrinsics_require("molt_pending", globals())
_MOLT_CAP_REQUIRE = _intrinsics_require("molt_capabilities_require", globals())
_MOLT_STRUCT_PACK = _intrinsics_require("molt_struct_pack", globals())
_MOLT_STRUCT_UNPACK = _intrinsics_require("molt_struct_unpack", globals())
_PENDING_SENTINEL: Any | None = None

_MAX_MESSAGE = 64 * 1024 * 1024
_MAX_DEPTH = 100

_TAG_NONE = 0x00
_TAG_FALSE = 0x01
_TAG_TRUE = 0x02
_TAG_INT = 0x03
_TAG_FLOAT = 0x04
_TAG_BYTES = 0x05
_TAG_STR = 0x06
_TAG_LIST = 0x07
_TAG_TUPLE = 0x08
_TAG_DICT = 0x09
_TAG_SET = 0x0A
_TAG_FROZENSET = 0x0B
_TAG_COMPLEX = 0x0C
_TAG_FUNC = 0x40
_TAG_QUEUE = 0x41
_TAG_PIPE = 0x42
_TAG_VALUE = 0x43
_TAG_ARRAY = 0x44
_TAG_EXC = 0x45

TimeoutError = _builtins.TimeoutError

_MSG_RUN = "run"
_MSG_WORKER = "worker"
_MSG_TASK = "task"
_MSG_TASK_RESULT = "task_result"
_MSG_TASK_ERROR = "task_error"
_MSG_QUEUE_PUT = "queue_put"
_MSG_PIPE_SEND = "pipe_send"
_MSG_VALUE_SET = "value_set"
_MSG_ARRAY_SET = "array_set"
_MSG_CLOSE = "close"


def _pending_sentinel() -> Any:
    global _PENDING_SENTINEL
    if _PENDING_SENTINEL is None:
        _PENDING_SENTINEL = _MOLT_PENDING()
    return _PENDING_SENTINEL


def _is_pending(value: Any) -> bool:
    sentinel = _pending_sentinel()
    return value is sentinel or value == sentinel


@dataclass(frozen=True)
class _FunctionRef:
    module: str
    qualname: str


@dataclass(frozen=True)
class _QueueRef:
    queue_id: int


@dataclass(frozen=True)
class _PipeRef:
    pipe_id: int
    readable: bool
    writable: bool


@dataclass(frozen=True)
class _ValueRef:
    value_id: int
    typecode: str
    value: Any


@dataclass(frozen=True)
class _ArrayRef:
    array_id: int
    typecode: str
    values: list[Any]


class _ExceptionRef:
    __slots__ = ("module", "name", "message")

    def __init__(self, module: str, name: str, message: str) -> None:
        self.module = module
        self.name = name
        self.message = message


class _RemoteError(Exception):
    pass


def _require_process_capability() -> None:
    _MOLT_CAP_REQUIRE("process.exec")


def _get_env_value(key: str, default: str) -> str:
    getter = _require_intrinsic(_MOLT_ENV_GET, "molt_env_get")
    return str(getter(key, default))


def _mp_debug_enabled() -> bool:
    return _get_env_value("MOLT_MP_DEBUG", "") == "1"


def _mp_debug(message: str) -> None:
    if not _mp_debug_enabled():
        return
    try:
        print(message, file=sys.stderr)
    except Exception:
        pass


def _spawn_trace_enabled() -> bool:
    return _get_env_value("MOLT_MP_TRACE_SPAWN", "") == "1"


def _spawn_trace(message: str) -> None:
    if not _spawn_trace_enabled():
        return
    try:
        err = open(2, "wb", closefd=False)
        err.write((message + "\n").encode())
        err.flush()
    except Exception:
        pass


def _build_spawn_env() -> dict[str, str]:
    snapshot = _require_intrinsic(_MOLT_ENV_SNAPSHOT, "molt_env_snapshot")()
    if not isinstance(snapshot, dict):
        raise RuntimeError("molt_env_snapshot must return a dict")
    return {str(key): str(val) for key, val in snapshot.items()}


def _exception_ref_from_exc(exc: BaseException) -> _ExceptionRef:
    exc_type = type(exc)
    module = getattr(exc_type, "__module__", "builtins")
    name = getattr(exc_type, "__name__", exc_type.__class__.__name__)
    if not isinstance(module, str):
        module = "builtins"
    try:
        message = str(exc)
    except Exception:
        message = repr(exc)
    return _ExceptionRef(str(module), str(name), message)


def _now() -> float:
    return time.monotonic()


def _resolve_entry_module_name() -> str:
    main_mod = sys.modules.get("__main__")
    if main_mod is None:
        return "__main__"
    spec = getattr(main_mod, "__spec__", None)
    spec_name = getattr(spec, "name", None) if spec is not None else None
    _mp_debug(
        "mp: resolve_entry"
        + f" spec={spec_name!r}"
        + f" file={getattr(main_mod, '__file__', None)!r}"
    )
    if isinstance(spec_name, str) and spec_name and spec_name != "__main__":
        return spec_name
    main_path = getattr(main_mod, "__file__", None)
    if isinstance(main_path, str) and main_path:
        resolved = _module_name_from_path(main_path)
        if resolved is not None and resolved != "__main__":
            return resolved
    for name, mod in sys.modules.items():
        if name == "__main__" or mod is not main_mod:
            continue
        if name == "__mp_main__":
            continue
        return name
    return "__main__"


def _module_name_from_path(path: str) -> str | None:
    try:
        abs_path = os.path.abspath(path)
    except Exception:
        return None
    roots = list(sys.path)
    if "" in roots:
        try:
            roots[roots.index("")] = os.getcwd()
        except Exception:
            pass
    best_parts: list[str] | None = None
    for root in roots:
        if not root:
            continue
        try:
            root_abs = os.path.abspath(root)
        except Exception:
            continue
        if abs_path == root_abs:
            continue
        if not abs_path.startswith(root_abs.rstrip(os.sep) + os.sep):
            continue
        try:
            rel = os.path.relpath(abs_path, root_abs)
        except Exception:
            continue
        if rel.startswith(".."):
            continue
        if rel.endswith(".py"):
            rel = rel[:-3]
        parts = [p for p in rel.split(os.sep) if p]
        if not parts:
            continue
        if parts[-1] == "__init__":
            parts = parts[:-1]
        if not parts:
            continue
        if best_parts is None or len(parts) > len(best_parts):
            best_parts = parts
    if best_parts is None:
        return None
    return ".".join(best_parts)


try:
    _COMPLEX_TYPE = type(0j)
except Exception:
    _COMPLEX_TYPE = None


def _complex_from_parts(real: float, imag: float) -> Any:
    if _COMPLEX_TYPE is not None:
        try:
            return _COMPLEX_TYPE(real, imag)
        except Exception:
            pass
    return real + imag * 1j


def _encode_varint(value: int, out: bytearray) -> None:
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            break


def _decode_varint(data: bytes, idx: int) -> tuple[int, int]:
    shift = 0
    value = 0
    while True:
        if idx >= len(data):
            raise ValueError("truncated varint")
        byte = data[idx]
        idx += 1
        value |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            return value, idx
        shift += 7
        if shift > 10_000:
            raise ValueError("varint too large")


def _encode_int(value: int, out: bytearray) -> None:
    if value < 0:
        out.append(1)
        _encode_varint(-value, out)
    else:
        out.append(0)
        _encode_varint(value, out)


def _decode_int(data: bytes, idx: int) -> tuple[int, int]:
    if idx >= len(data):
        raise ValueError("truncated int")
    sign = data[idx]
    idx += 1
    magnitude, idx = _decode_varint(data, idx)
    return (-magnitude if sign else magnitude), idx


def _encode_bytes(value: bytes, out: bytearray) -> None:
    _encode_varint(len(value), out)
    out.extend(value)


def _pack_f64(value: float) -> bytes:
    packed = _MOLT_STRUCT_PACK("<d", (float(value),))
    if isinstance(packed, (bytes, bytearray)):
        return bytes(packed)
    raise RuntimeError("invalid struct pack payload for float")


def _unpack_f64(payload: bytes) -> float:
    unpacked = _MOLT_STRUCT_UNPACK("<d", payload)
    if not isinstance(unpacked, tuple) or len(unpacked) != 1:
        raise RuntimeError("invalid struct unpack payload for float")
    return float(unpacked[0])


def _decode_bytes(data: bytes, idx: int) -> tuple[bytes, int]:
    length, idx = _decode_varint(data, idx)
    end = idx + length
    if end > len(data):
        raise ValueError("truncated bytes")
    return data[idx:end], end


def _encode_value(value: Any, out: bytearray, hub: "_Hub | None", depth: int) -> None:
    if depth > _MAX_DEPTH:
        raise ValueError("object too deep")
    if value is None:
        out.append(_TAG_NONE)
        return
    if value is False:
        out.append(_TAG_FALSE)
        return
    if value is True:
        out.append(_TAG_TRUE)
        return
    if isinstance(value, _FunctionRef):
        out.append(_TAG_FUNC)
        _encode_value(value.module, out, hub, depth + 1)
        _encode_value(value.qualname, out, hub, depth + 1)
        return
    if isinstance(value, _QueueRef):
        out.append(_TAG_QUEUE)
        _encode_varint(value.queue_id, out)
        return
    if isinstance(value, _PipeRef):
        out.append(_TAG_PIPE)
        _encode_varint(value.pipe_id, out)
        out.append(1 if value.readable else 0)
        out.append(1 if value.writable else 0)
        return
    if isinstance(value, _ValueRef):
        out.append(_TAG_VALUE)
        _encode_varint(value.value_id, out)
        _encode_value(value.typecode, out, hub, depth + 1)
        _encode_value(value.value, out, hub, depth + 1)
        return
    if isinstance(value, _ArrayRef):
        out.append(_TAG_ARRAY)
        _encode_varint(value.array_id, out)
        _encode_value(value.typecode, out, hub, depth + 1)
        _encode_value(value.values, out, hub, depth + 1)
        return
    if isinstance(value, _ExceptionRef):
        out.append(_TAG_EXC)
        _encode_value(value.module, out, hub, depth + 1)
        _encode_value(value.name, out, hub, depth + 1)
        _encode_value(value.message, out, hub, depth + 1)
        return
    if hub is not None:
        if isinstance(value, _Queue):
            out.append(_TAG_QUEUE)
            _encode_varint(hub.register_queue(value), out)
            return
        if isinstance(value, PipeConnection):
            out.append(_TAG_PIPE)
            local_conn = value._peer if value._peer is not None else value
            _encode_varint(hub.register_pipe(local_conn), out)
            out.append(1 if value.readable else 0)
            out.append(1 if value.writable else 0)
            return
        if isinstance(value, SharedValue):
            out.append(_TAG_VALUE)
            _encode_varint(hub.register_value(value), out)
            _encode_value(value.typecode, out, hub, depth + 1)
            _encode_value(value.value, out, hub, depth + 1)
            return
        if isinstance(value, SharedArray):
            out.append(_TAG_ARRAY)
            _encode_varint(hub.register_array(value), out)
            _encode_value(value.typecode, out, hub, depth + 1)
            _encode_value(list(value), out, hub, depth + 1)
            return
        if callable(value):
            module = getattr(value, "__module__", None)
            qual = getattr(value, "__qualname__", None)
            if isinstance(module, str) and isinstance(qual, str):
                out.append(_TAG_FUNC)
                _encode_value(module, out, hub, depth + 1)
                _encode_value(qual, out, hub, depth + 1)
                return
    if isinstance(value, int):
        out.append(_TAG_INT)
        _encode_int(value, out)
        return
    if isinstance(value, float):
        out.append(_TAG_FLOAT)
        out.extend(_pack_f64(value))
        return
    if _COMPLEX_TYPE is not None and isinstance(value, _COMPLEX_TYPE):
        out.append(_TAG_COMPLEX)
        out.extend(_pack_f64(value.real))
        out.extend(_pack_f64(value.imag))
        return
    if isinstance(value, bytes):
        out.append(_TAG_BYTES)
        _encode_bytes(value, out)
        return
    if isinstance(value, bytearray):
        out.append(_TAG_BYTES)
        _encode_bytes(bytes(value), out)
        return
    if isinstance(value, str):
        out.append(_TAG_STR)
        encoded = value.encode("utf-8")
        _encode_bytes(encoded, out)
        return
    if isinstance(value, list):
        out.append(_TAG_LIST)
        _encode_varint(len(value), out)
        for item in value:
            _encode_value(item, out, hub, depth + 1)
        return
    if isinstance(value, tuple):
        out.append(_TAG_TUPLE)
        _encode_varint(len(value), out)
        for item in value:
            _encode_value(item, out, hub, depth + 1)
        return
    if isinstance(value, dict):
        out.append(_TAG_DICT)
        _encode_varint(len(value), out)
        for key, item in value.items():
            _encode_value(key, out, hub, depth + 1)
            _encode_value(item, out, hub, depth + 1)
        return
    if isinstance(value, set):
        out.append(_TAG_SET)
        items = sorted(value, key=repr)
        _encode_varint(len(items), out)
        for item in items:
            _encode_value(item, out, hub, depth + 1)
        return
    if isinstance(value, frozenset):
        out.append(_TAG_FROZENSET)
        items = sorted(value, key=repr)
        _encode_varint(len(items), out)
        for item in items:
            _encode_value(item, out, hub, depth + 1)
        return
    raise TypeError(f"cannot serialize {type(value).__name__}")


def _decode_value(
    data: bytes, idx: int, hub: "_Hub | None", depth: int
) -> tuple[Any, int]:
    if depth > _MAX_DEPTH:
        raise ValueError("object too deep")
    if idx >= len(data):
        raise ValueError("truncated payload")
    tag = data[idx]
    idx += 1
    if tag == _TAG_NONE:
        return None, idx
    if tag == _TAG_FALSE:
        return False, idx
    if tag == _TAG_TRUE:
        return True, idx
    if tag == _TAG_INT:
        value, idx = _decode_int(data, idx)
        return value, idx
    if tag == _TAG_FLOAT:
        end = idx + 8
        if end > len(data):
            raise ValueError("truncated float")
        return _unpack_f64(data[idx:end]), end
    if tag == _TAG_COMPLEX:
        end = idx + 16
        if end > len(data):
            raise ValueError("truncated complex")
        real = _unpack_f64(data[idx : idx + 8])
        imag = _unpack_f64(data[idx + 8 : end])
        return _complex_from_parts(real, imag), end
    if tag == _TAG_BYTES:
        value, idx = _decode_bytes(data, idx)
        return value, idx
    if tag == _TAG_STR:
        raw, idx = _decode_bytes(data, idx)
        return raw.decode("utf-8"), idx
    if tag == _TAG_LIST:
        length, idx = _decode_varint(data, idx)
        items: list[Any] = []
        for _ in range(length):
            item, idx = _decode_value(data, idx, hub, depth + 1)
            items.append(item)
        return items, idx
    if tag == _TAG_TUPLE:
        length, idx = _decode_varint(data, idx)
        items: list[Any] = []
        for _ in range(length):
            item, idx = _decode_value(data, idx, hub, depth + 1)
            items.append(item)
        return tuple(items), idx
    if tag == _TAG_DICT:
        length, idx = _decode_varint(data, idx)
        out: dict[Any, Any] = {}
        for _ in range(length):
            key, idx = _decode_value(data, idx, hub, depth + 1)
            value, idx = _decode_value(data, idx, hub, depth + 1)
            out[key] = value
        return out, idx
    if tag == _TAG_SET:
        length, idx = _decode_varint(data, idx)
        items: list[Any] = []
        for _ in range(length):
            item, idx = _decode_value(data, idx, hub, depth + 1)
            items.append(item)
        return set(items), idx
    if tag == _TAG_FROZENSET:
        length, idx = _decode_varint(data, idx)
        items: list[Any] = []
        for _ in range(length):
            item, idx = _decode_value(data, idx, hub, depth + 1)
            items.append(item)
        return frozenset(items), idx
    if tag == _TAG_FUNC:
        module, idx = _decode_value(data, idx, hub, depth + 1)
        qual, idx = _decode_value(data, idx, hub, depth + 1)
        if not isinstance(module, str) or not isinstance(qual, str):
            raise ValueError("invalid function reference")
        if hub is None:
            return _FunctionRef(module, qual), idx
        return hub.resolve_function(module, qual), idx
    if tag == _TAG_QUEUE:
        queue_id, idx = _decode_varint(data, idx)
        if hub is None:
            return _QueueRef(queue_id), idx
        return hub.queue_proxy(queue_id), idx
    if tag == _TAG_PIPE:
        pipe_id, idx = _decode_varint(data, idx)
        if idx + 2 > len(data):
            raise ValueError("truncated pipe ref")
        readable = data[idx] == 1
        writable = data[idx + 1] == 1
        idx += 2
        if hub is None:
            return _PipeRef(pipe_id, readable, writable), idx
        return hub.pipe_proxy(pipe_id, readable, writable), idx
    if tag == _TAG_VALUE:
        value_id, idx = _decode_varint(data, idx)
        typecode, idx = _decode_value(data, idx, hub, depth + 1)
        payload, idx = _decode_value(data, idx, hub, depth + 1)
        if not isinstance(typecode, str):
            raise ValueError("invalid value ref")
        if hub is None:
            return _ValueRef(value_id, typecode, payload), idx
        return hub.value_proxy(value_id, typecode, payload), idx
    if tag == _TAG_ARRAY:
        array_id, idx = _decode_varint(data, idx)
        typecode, idx = _decode_value(data, idx, hub, depth + 1)
        payload, idx = _decode_value(data, idx, hub, depth + 1)
        if not isinstance(typecode, str):
            raise ValueError("invalid array ref")
        if not isinstance(payload, list):
            raise ValueError("invalid array payload")
        if hub is None:
            return _ArrayRef(array_id, typecode, payload), idx
        return hub.array_proxy(array_id, typecode, payload), idx
    if tag == _TAG_EXC:
        module, idx = _decode_value(data, idx, hub, depth + 1)
        name, idx = _decode_value(data, idx, hub, depth + 1)
        message, idx = _decode_value(data, idx, hub, depth + 1)
        if not isinstance(module, str) or not isinstance(name, str):
            raise ValueError("invalid exception ref")
        if not isinstance(message, str):
            message = str(message)
        if hub is None:
            return _ExceptionRef(module, name, message), idx
        return hub.resolve_exception(module, name, message), idx
    raise ValueError(f"unknown tag {tag}")


def _encode_message(message: Any, hub: "_Hub | None") -> bytes:
    out = bytearray()
    _encode_value(message, out, hub, 0)
    if len(out) > _MAX_MESSAGE:
        raise ValueError("message too large")
    return bytes(out)


def _decode_message(data: bytes, hub: "_Hub | None") -> Any:
    value, idx = _decode_value(data, 0, hub, 0)
    if idx != len(data):
        raise ValueError("trailing bytes")
    return value


def _bind_hub(value: Any, hub: "_Hub", depth: int, seen: set[int]) -> None:
    if depth > _MAX_DEPTH:
        return
    obj_id = id(value)
    if obj_id in seen:
        return
    seen.add(obj_id)
    if isinstance(value, _Queue):
        hub.register_queue(value)
        return
    if isinstance(value, PipeConnection):
        local_conn = value._peer if value._peer is not None else value
        hub.register_pipe(local_conn)
        return
    if isinstance(value, SharedValue):
        hub.register_value(value)
        return
    if isinstance(value, SharedArray):
        hub.register_array(value)
        return
    if isinstance(value, (list, tuple, set, frozenset)):
        for item in value:
            _bind_hub(item, hub, depth + 1, seen)
        return
    if isinstance(value, dict):
        for key, item in value.items():
            _bind_hub(key, hub, depth + 1, seen)
            _bind_hub(item, hub, depth + 1, seen)
        return


def _stream_send_frame(stream: Any, payload: bytes) -> None:
    if _MOLT_STREAM_SEND is None:
        raise RuntimeError("stream send unavailable")
    if not callable(_MOLT_STREAM_SEND):
        raise TypeError("stream send unavailable")
    length = len(payload)
    if length > _MAX_MESSAGE:
        raise ValueError("message too large")
    header = length.to_bytes(4, "big")
    data = header + payload
    while True:
        res = _MOLT_STREAM_SEND(stream, data)
        if not _is_pending(res):
            return
        time.sleep(0)


def _stream_poll_frame(stream: Any, buf: bytearray) -> bytes | None:
    if len(buf) >= 4:
        length = int.from_bytes(buf[:4], "big")
        if length > _MAX_MESSAGE:
            raise ValueError("message too large")
        if len(buf) >= 4 + length:
            payload = bytes(buf[4 : 4 + length])
            del buf[: 4 + length]
            return payload
    if _MOLT_STREAM_RECV is None:
        raise RuntimeError("stream recv unavailable")
    chunk = _MOLT_STREAM_RECV(stream)
    if chunk is None or _is_pending(chunk):
        return None
    if isinstance(chunk, (bytes, bytearray)):
        buf.extend(chunk)
        return _stream_poll_frame(stream, buf)
    return None


def _stream_recv_frame(
    stream: Any, buf: bytearray, timeout: float | None
) -> bytes | None:
    deadline = None if timeout is None else _now() + timeout
    while True:
        payload = _stream_poll_frame(stream, buf)
        if payload is not None:
            return payload
        if deadline is not None and _now() >= deadline:
            return None
        time.sleep(0.001)


class _StreamTransport:
    def __init__(self, recv_stream: Any, send_stream: Any) -> None:
        self._recv = recv_stream
        self._send = send_stream
        self._buf = bytearray()

    def close(self) -> None:
        if _MOLT_STREAM_CLOSE is not None:
            try:
                _MOLT_STREAM_CLOSE(self._send)
            except Exception:
                pass
        if _MOLT_STREAM_DROP is not None:
            try:
                _MOLT_STREAM_DROP(self._send)
            except Exception:
                pass
        if _MOLT_STREAM_DROP is not None:
            try:
                _MOLT_STREAM_DROP(self._recv)
            except Exception:
                pass

    def _send_bytes(self, data: bytes) -> None:
        if _MOLT_STREAM_SEND is None:
            raise RuntimeError("stream send unavailable")
        if not callable(_MOLT_STREAM_SEND):
            _spawn_trace(
                f"stream_send type={type(_MOLT_STREAM_SEND).__name__} not callable"
            )
            raise TypeError("stream send unavailable")
        while True:
            res = _MOLT_STREAM_SEND(self._send, data)
            if not _is_pending(res):
                return
            time.sleep(0)

    def send_frame(self, payload: bytes) -> None:
        length = len(payload)
        if length > _MAX_MESSAGE:
            raise ValueError("message too large")
        header = length.to_bytes(4, "big")
        self._send_bytes(header + payload)

    def _try_recv_chunk(self) -> bool:
        if _MOLT_STREAM_RECV is None:
            raise RuntimeError("stream recv unavailable")
        chunk = _MOLT_STREAM_RECV(self._recv)
        if _is_pending(chunk):
            return False
        if chunk is None:
            return False
        if isinstance(chunk, (bytes, bytearray)):
            self._buf.extend(chunk)
            return True
        return False

    def poll_frame(self) -> bytes | None:
        if len(self._buf) >= 4:
            length = int.from_bytes(self._buf[:4], "big")
            if length > _MAX_MESSAGE:
                raise ValueError("message too large")
            if len(self._buf) >= 4 + length:
                payload = bytes(self._buf[4 : 4 + length])
                del self._buf[: 4 + length]
                return payload
        if not self._try_recv_chunk():
            return None
        return self.poll_frame()

    def recv_frame(self, timeout: float | None) -> bytes | None:
        deadline = None if timeout is None else _now() + timeout
        while True:
            payload = self.poll_frame()
            if payload is not None:
                return payload
            if deadline is not None and _now() >= deadline:
                return None
            time.sleep(0.001)


class _FdTransport:
    def __init__(self, reader: Any, writer: Any) -> None:
        self._reader = reader
        self._writer = writer
        self._buf = bytearray()

    def send_frame(self, payload: bytes) -> None:
        length = len(payload)
        if length > _MAX_MESSAGE:
            raise ValueError("message too large")
        header = length.to_bytes(4, "big")
        self._writer.write(header + payload)
        self._writer.flush()

    def _read_exact(self, size: int) -> bytes:
        out = bytearray()
        while len(out) < size:
            chunk = self._reader.read(size - len(out))
            if not chunk:
                raise EOFError
            out.extend(chunk)
        return bytes(out)

    def recv_frame(self) -> bytes:
        if len(self._buf) < 4:
            self._buf.extend(self._read_exact(4 - len(self._buf)))
        length = int.from_bytes(self._buf[:4], "big")
        if length > _MAX_MESSAGE:
            raise ValueError("message too large")
        needed = 4 + length
        if len(self._buf) < needed:
            self._buf.extend(self._read_exact(needed - len(self._buf)))
        payload = bytes(self._buf[4:needed])
        del self._buf[:needed]
        return payload


class _Hub:
    def __init__(
        self,
        transport: Any,
        role: str,
        recv_stream: Any | None = None,
        send_stream: Any | None = None,
    ) -> None:
        self._transport = transport
        self._role = role
        self._next_id = 1
        self._queues: dict[int, _Queue] = {}
        self._pipes: dict[int, PipeConnection] = {}
        self._values: dict[int, SharedValue] = {}
        self._arrays: dict[int, SharedArray] = {}
        self._task_error: BaseException | None = None
        self._recv_stream = recv_stream
        self._send_stream = send_stream
        self._stream_buf = bytearray()

    def _transport_ref(self) -> Any:
        try:
            return getattr(self, "_transport")
        except Exception:
            return self._transport

    def register_queue(self, queue: "_Queue") -> int:
        if queue._hub is None:
            queue._hub = self
            self._queues[queue._id] = queue
        elif queue._hub is not self:
            raise RuntimeError("Queue used across processes")
        return queue._id

    def register_pipe(self, conn: "PipeConnection") -> int:
        if conn._hub is None:
            conn._hub = self
            self._pipes[conn._id] = conn
        elif conn._hub is not self:
            raise RuntimeError("Pipe used across processes")
        return conn._id

    def register_value(self, value: "SharedValue") -> int:
        if value._hub is None:
            value._hub = self
            self._values[value._id] = value
        elif value._hub is not self:
            raise RuntimeError("Value used across processes")
        return value._id

    def register_array(self, array: "SharedArray") -> int:
        if array._hub is None:
            array._hub = self
            self._arrays[array._id] = array
        elif array._hub is not self:
            raise RuntimeError("Array used across processes")
        return array._id

    def queue_proxy(self, queue_id: int) -> "QueueProxy":
        return QueueProxy(queue_id, self)

    def pipe_proxy(self, pipe_id: int, readable: bool, writable: bool) -> "PipeProxy":
        proxy = PipeProxy(pipe_id, readable, writable, self)
        self._pipes[pipe_id] = proxy  # type: ignore[assignment]
        return proxy

    def value_proxy(self, value_id: int, typecode: str, value: Any) -> "ValueProxy":
        return ValueProxy(value_id, typecode, value, self)

    def array_proxy(
        self, array_id: int, typecode: str, values: list[Any]
    ) -> "ArrayProxy":
        return ArrayProxy(array_id, typecode, values, self)

    def resolve_function(self, module: str, qualname: str) -> Callable[..., Any]:
        _mp_debug(
            "mp: resolve_function"
            + f" module={module!r}"
            + f" qualname={qualname!r}"
            + f" has_main={('__main__' in sys.modules)!r}"
            + f" has_mp_main={('__mp_main__' in sys.modules)!r}"
        )
        mod = None
        if module == "__main__":
            mod = sys.modules.get("__main__") or sys.modules.get("__mp_main__")
            if mod is None:
                entry_module = _get_env_value(_MP_ENTRY_ENV, "")
                if entry_module:
                    try:
                        mod = __import__(entry_module, fromlist=["*"])
                        _mp_debug(
                            f"mp: resolve_function loaded entry module {entry_module!r}"
                        )
                    except Exception as exc:
                        _mp_debug(
                            "mp: resolve_function entry import failed "
                            + f"{type(exc).__name__}: {exc}"
                        )
                        mod = None
        if mod is None:
            mod = sys.modules.get(module)
        if mod is None:
            try:
                mod = __import__(module, fromlist=["*"])
            except Exception:
                mod = importlib.import_module(module)
        target: Any = mod
        for part in qualname.split("."):
            target = getattr(target, part)
        if not callable(target):
            raise TypeError("decoded function not callable")
        return target

    def resolve_exception(self, module: str, name: str, message: str) -> Exception:
        mod: Any | None = None
        if module in {"builtins", "__builtin__"}:
            mod = _builtins
        if mod is None:
            try:
                mod = sys.modules.get(module)
            except Exception:
                mod = None
        if mod is None:
            try:
                mod = importlib.import_module(module)
            except Exception:
                mod = None
        exc_type = getattr(mod, name, RuntimeError) if mod is not None else RuntimeError
        try:
            return exc_type(message)
        except Exception:
            return _RemoteError(message)

    def send(self, message: Any) -> None:
        _bind_hub(message, self, 0, set())
        payload = _encode_message(message, self)
        if self._send_stream is not None:
            _stream_send_frame(self._send_stream, payload)
            return
        self._transport_ref().send_frame(payload)

    def recv(self, timeout: float | None = None) -> Any | None:
        if self._recv_stream is not None:
            payload = _stream_recv_frame(self._recv_stream, self._stream_buf, timeout)
            if payload is None:
                return None
        else:
            transport = self._transport_ref()
            if isinstance(transport, _StreamTransport):
                payload = transport.recv_frame(timeout)
                if payload is None:
                    return None
            else:
                payload = transport.recv_frame()
        return _decode_message(payload, self)

    def poll(self) -> Any | None:
        if self._recv_stream is not None:
            payload = _stream_poll_frame(self._recv_stream, self._stream_buf)
        else:
            transport = self._transport_ref()
            if isinstance(transport, _StreamTransport):
                payload = transport.poll_frame()
            else:
                poller = getattr(transport, "poll_frame", None)
                if not callable(poller):
                    return None
                payload = poller()
        if payload is None:
            return None
        return _decode_message(payload, self)

    def handle_message(self, message: Any) -> bool:
        if not isinstance(message, tuple) or not message:
            return False
        kind = message[0]
        if kind == _MSG_QUEUE_PUT:
            queue_id, payload = message[1], message[2]
            queue = self._queues.get(int(queue_id))
            if queue is not None:
                queue._buffer.append(payload)
            _mp_debug(f"mp: handle queue_put id={queue_id!r} queue={queue is not None}")
            return True
        if kind == _MSG_PIPE_SEND:
            pipe_id, payload = message[1], message[2]
            pipe = self._pipes.get(int(pipe_id))
            if pipe is not None and hasattr(pipe, "_buffer"):
                pipe._buffer.append(payload)
            return True
        if kind == _MSG_VALUE_SET:
            value_id, payload = message[1], message[2]
            value = self._values.get(int(value_id))
            if value is not None:
                value._value = payload
            return True
        if kind == _MSG_ARRAY_SET:
            array_id, idx, payload = message[1], message[2], message[3]
            array = self._arrays.get(int(array_id))
            if array is not None:
                array._values[int(idx)] = payload
            return True
        if kind == _MSG_TASK_ERROR:
            exc = message[3] if len(message) > 3 else _RemoteError("child error")
            if isinstance(exc, BaseException):
                self._task_error = exc
            else:
                self._task_error = _RemoteError(str(exc))
            _mp_debug(f"mp: task_error {self._task_error!r}")
            return True
        return False

    def close(self) -> None:
        transport = self._transport_ref()
        if isinstance(transport, _StreamTransport):
            transport.close()


class _Queue:
    def __init__(self, maxsize: int = 0) -> None:
        self._id = _next_object_id()
        self._buffer: list[Any] = []
        self._hub: _Hub | None = None
        self._maxsize = maxsize

    def put(self, obj: Any, block: bool = True, timeout: float | None = None) -> None:
        if self._hub is None:
            self._buffer.append(obj)
            return
        if self._hub._role != "child":
            raise NotImplementedError("parent Queue.put not supported")
        _mp_debug(f"mp: queue.put id={self._id!r} child->parent")
        self._hub.send((_MSG_QUEUE_PUT, self._id, obj))

    def get(self, block: bool = True, timeout: float | None = None) -> Any:
        if self._buffer:
            return self._buffer.pop(0)
        if self._hub is None:
            if not block:
                raise TimeoutError("queue empty")
            deadline = None if timeout is None else _now() + timeout
            while True:
                if self._buffer:
                    return self._buffer.pop(0)
                if deadline is not None and _now() >= deadline:
                    raise TimeoutError("queue empty")
                time.sleep(0.001)
        if self._hub._role != "parent":
            raise NotImplementedError("child Queue.get not supported")
        deadline = None if timeout is None else _now() + timeout
        while True:
            if self._hub._task_error is not None:
                raise self._hub._task_error
            if self._buffer:
                return self._buffer.pop(0)
            msg = self._hub.poll()
            if msg is None:
                if deadline is not None and _now() >= deadline:
                    raise TimeoutError("queue empty")
                time.sleep(0.001)
                continue
            self._hub.handle_message(msg)

    def close(self) -> None:
        return None

    def join_thread(self) -> None:
        return None


class QueueProxy:
    def __init__(self, queue_id: int, hub: _Hub) -> None:
        self._id = queue_id
        self._hub = hub

    def put(self, obj: Any, block: bool = True, timeout: float | None = None) -> None:
        _mp_debug(f"mp: queue_proxy.put id={self._id!r} child->parent")
        self._hub.send((_MSG_QUEUE_PUT, self._id, obj))

    def get(self, block: bool = True, timeout: float | None = None) -> Any:
        raise NotImplementedError("queue proxy get not supported")

    def close(self) -> None:
        return None

    def join_thread(self) -> None:
        return None


class PipeConnection:
    def __init__(self, pipe_id: int, readable: bool, writable: bool) -> None:
        self._id = pipe_id
        self._hub: _Hub | None = None
        self._peer: "PipeConnection | None" = None
        self.readable = readable
        self.writable = writable
        self._buffer: list[Any] = []

    def _ensure_hub(self) -> _Hub:
        if self._hub is None and self._peer is not None and self._peer._hub is not None:
            self._hub = self._peer._hub
            self._hub._pipes[self._id] = self
        if self._hub is None:
            raise RuntimeError("Pipe not bound")
        return self._hub

    def send(self, obj: Any) -> None:
        if not self.writable:
            raise OSError("connection is read-only")
        hub = self._ensure_hub()
        hub.send((_MSG_PIPE_SEND, self._id, obj))

    def recv(self) -> Any:
        if not self.readable:
            raise OSError("connection is write-only")
        if self._buffer:
            return self._buffer.pop(0)
        hub = self._ensure_hub()
        while True:
            msg = hub.poll()
            if msg is None:
                time.sleep(0.001)
                continue
            hub.handle_message(msg)
            if self._buffer:
                return self._buffer.pop(0)

    def close(self) -> None:
        return None


class PipeProxy:
    def __init__(self, pipe_id: int, readable: bool, writable: bool, hub: _Hub) -> None:
        self._id = pipe_id
        self._hub = hub
        self.readable = readable
        self.writable = writable
        self._buffer: list[Any] = []

    def send(self, obj: Any) -> None:
        if not self.writable:
            raise OSError("connection is read-only")
        self._hub.send((_MSG_PIPE_SEND, self._id, obj))

    def recv(self) -> Any:
        if not self.readable:
            raise OSError("connection is write-only")
        while True:
            msg = self._hub.poll()
            if msg is None:
                time.sleep(0.001)
                continue
            self._hub.handle_message(msg)
            if self._buffer:
                return self._buffer.pop(0)

    def close(self) -> None:
        return None


class SharedValue:
    def __init__(self, typecode: str, value: Any) -> None:
        self._id = _next_object_id()
        self.typecode = typecode
        self._value = value
        self._hub: _Hub | None = None

    @property
    def value(self) -> Any:
        return self._value

    @value.setter
    def value(self, new_value: Any) -> None:
        self._value = new_value
        if self._hub is not None and self._hub._role == "child":
            self._hub.send((_MSG_VALUE_SET, self._id, new_value))


class ValueProxy:
    def __init__(self, value_id: int, typecode: str, value: Any, hub: _Hub) -> None:
        self._id = value_id
        self.typecode = typecode
        self._value = value
        self._hub = hub

    @property
    def value(self) -> Any:
        return self._value

    @value.setter
    def value(self, new_value: Any) -> None:
        self._value = new_value
        self._hub.send((_MSG_VALUE_SET, self._id, new_value))


class SharedArray:
    def __init__(self, typecode: str, values: Iterable[Any]) -> None:
        self._id = _next_object_id()
        self.typecode = typecode
        self._values = list(values)
        self._hub: _Hub | None = None

    def __len__(self) -> int:
        return len(self._values)

    def __getitem__(self, idx: int) -> Any:
        return self._values[idx]

    def __setitem__(self, idx: int, value: Any) -> None:
        self._values[idx] = value
        if self._hub is not None and self._hub._role == "child":
            self._hub.send((_MSG_ARRAY_SET, self._id, idx, value))

    def __iter__(self) -> Iterator[Any]:
        return iter(self._values)


class ArrayProxy:
    def __init__(
        self, array_id: int, typecode: str, values: list[Any], hub: _Hub
    ) -> None:
        self._id = array_id
        self.typecode = typecode
        self._values = list(values)
        self._hub = hub

    def __len__(self) -> int:
        return len(self._values)

    def __getitem__(self, idx: int) -> Any:
        return self._values[idx]

    def __setitem__(self, idx: int, value: Any) -> None:
        self._values[idx] = value
        self._hub.send((_MSG_ARRAY_SET, self._id, idx, value))

    def __iter__(self) -> Iterator[Any]:
        return iter(self._values)


_NEXT_OBJECT_ID = 1


def _next_object_id() -> int:
    global _NEXT_OBJECT_ID
    value = _NEXT_OBJECT_ID
    _NEXT_OBJECT_ID += 1
    return value


class Process:
    def __init__(
        self,
        group: Any | None = None,
        target: Callable[..., Any] | None = None,
        name: str | None = None,
        args: Sequence[Any] = (),
        kwargs: dict[str, Any] | None = None,
        daemon: bool | None = None,
        ctx: "Context | None" = None,
    ) -> None:
        self._target = target
        self._args = tuple(args)
        self._kwargs = kwargs or {}
        self.name = name
        self.daemon = daemon
        self._ctx = ctx
        self._started = False
        self._exitcode: int | None = None
        self._handle: Any | None = None
        self._hub: _Hub | None = None
        self._transport: _StreamTransport | None = None

    def start(self) -> None:
        if self._started:
            raise RuntimeError("process already started")
        self._started = True
        if _MOLT_PROCESS_SPAWN is None:
            self._run_inline()
            return
        method = _effective_start_method(self._ctx)
        if method != "spawn":
            _mp_debug(f"mp: start method {method!r} uses spawn semantics")
        _require_process_capability()
        entry_module = _resolve_entry_module_name()
        env = _build_spawn_env()
        env[_ENTRY_OVERRIDE_ENV] = _MP_ENTRY_OVERRIDE
        env[_MP_ENTRY_ENV] = entry_module
        env[_MP_SPAWN_ENV] = "1"
        env[_MP_START_METHOD_ENV] = method
        main_mod = sys.modules.get("__main__")
        main_path = (
            getattr(main_mod, "__file__", None) if main_mod is not None else None
        )
        if not isinstance(main_path, str) or not main_path:
            argv0 = sys.argv[0] if sys.argv else None
            if isinstance(argv0, str) and argv0:
                main_path = argv0
        _mp_debug(f"mp: process spawn entry={entry_module!r} main_path={main_path!r}")
        if isinstance(main_path, str) and main_path:
            try:
                env[_MP_MAIN_PATH_ENV] = os.path.abspath(main_path)
            except Exception:
                pass
        trusted = _get_env_value("MOLT_TRUSTED", "")
        if trusted:
            env["MOLT_TRUSTED"] = trusted
        caps = _get_env_value("MOLT_CAPABILITIES", "")
        if caps:
            env["MOLT_CAPABILITIES"] = caps
        mp_debug = _get_env_value("MOLT_MP_DEBUG", "")
        if mp_debug:
            env["MOLT_MP_DEBUG"] = mp_debug
        _mp_debug(
            f"mp: spawn start entry={entry_module!r} overlay={env.get('MOLT_ENV_OVERLAY')!r}"
        )
        args = [sys.argv[0]]
        try:
            cwd = os.getcwd()
        except Exception:
            cwd = None
        try:
            spawn_proc = _require_intrinsic(_MOLT_PROCESS_SPAWN, "molt_process_spawn")
            handle = spawn_proc(
                args,
                env,
                cwd,
                _STDIO_PIPE,
                _STDIO_PIPE,
                _STDIO_INHERIT,
            )
        except RuntimeError:
            self._run_inline()
            return
        if handle is None:
            raise RuntimeError("process spawn failed")
        self._handle = handle
        stdin_stream = _require_intrinsic(_MOLT_PROCESS_STDIN, "molt_process_stdin")(
            handle
        )
        stdout_stream = _require_intrinsic(_MOLT_PROCESS_STDOUT, "molt_process_stdout")(
            handle
        )
        self._transport = _StreamTransport(stdout_stream, stdin_stream)
        self._hub = _Hub(
            self._transport,
            "parent",
            recv_stream=stdout_stream,
            send_stream=stdin_stream,
        )
        message = (_MSG_RUN, self._target, self._args, self._kwargs)
        self._hub.send(message)

    def _run_inline(self) -> None:
        try:
            if self._target is not None:
                self._target(*self._args, **self._kwargs)
            self._exitcode = 0
        except BaseException:
            self._exitcode = 1

    def join(self, timeout: float | None = None) -> None:
        if not self._started:
            return
        if self._handle is None:
            return
        deadline = None if timeout is None else _now() + timeout
        proc_return = _require_intrinsic(
            _MOLT_PROCESS_RETURN, "molt_process_returncode"
        )
        while True:
            code = proc_return(self._handle)
            if code is not None:
                self._exitcode = int(code)
                break
            if deadline is not None and _now() >= deadline:
                return
            if self._hub is not None:
                msg = self._hub.poll()
                if msg is not None:
                    self._hub.handle_message(msg)
            time.sleep(0.001)
        if self._hub is not None:
            while True:
                msg = self._hub.poll()
                if msg is None:
                    break
                self._hub.handle_message(msg)
        if self._transport is not None:
            self._transport.close()

    def is_alive(self) -> bool:
        if not self._started:
            return False
        if self._handle is None:
            return self._exitcode is None
        proc_return = _require_intrinsic(
            _MOLT_PROCESS_RETURN, "molt_process_returncode"
        )
        return proc_return(self._handle) is None

    def terminate(self) -> None:
        if self._handle is not None and _MOLT_PROCESS_TERM is not None:
            _MOLT_PROCESS_TERM(self._handle)

    def kill(self) -> None:
        if self._handle is not None and _MOLT_PROCESS_KILL is not None:
            _MOLT_PROCESS_KILL(self._handle)

    @property
    def exitcode(self) -> int | None:
        if self._handle is None:
            return self._exitcode
        proc_return = _require_intrinsic(
            _MOLT_PROCESS_RETURN, "molt_process_returncode"
        )
        code = proc_return(self._handle)
        return None if code is None else int(code)

    @property
    def pid(self) -> int:
        if self._handle is None:
            return 0
        try:
            proc_pid = _require_intrinsic(_MOLT_PROCESS_PID, "molt_process_pid")
            return int(proc_pid(self._handle))
        except Exception:
            return 0


class AsyncResult:
    def __init__(
        self,
        pool: "Pool",
        task_ids: list[int],
        results: list[Any],
        *,
        unwrap_single: bool,
    ) -> None:
        self._pool = pool
        self._task_ids = task_ids
        self._results = results
        self._ready = False
        self._success = True
        self._error: Exception | None = None
        self._unwrap_single = unwrap_single

    def _mark_error(self, exc: Exception) -> None:
        if self._error is None:
            self._error = exc
            self._success = False

    def _mark_ready(self) -> None:
        self._ready = True

    def ready(self) -> bool:
        return self._ready

    def successful(self) -> bool:
        if not self._ready:
            raise ValueError("result not ready")
        return self._success

    def get(self, timeout: float | None = None) -> Any:
        self._pool._wait_for(self, timeout)
        if self._error is not None:
            raise self._error
        if self._unwrap_single and len(self._results) == 1:
            return self._results[0]
        return list(self._results)


class IMapIterator:
    def __init__(self, pool: "Pool", ordered: bool, total: int) -> None:
        self._pool = pool
        self._ordered = ordered
        self._total = total
        self._next_index = 0
        self._done = 0
        self._buffer: dict[int, Any] = {}
        self._errors: dict[int, Exception] = {}

    def _push_result(self, idx: int, value: Any) -> None:
        self._buffer[idx] = value

    def _push_error(self, idx: int, exc: Exception) -> None:
        self._errors[idx] = exc

    def __iter__(self) -> "IMapIterator":
        return self

    def __next__(self) -> Any:
        return self.next(timeout=None)

    def next(self, timeout: float | None = None) -> Any:
        if self._done >= self._total:
            raise StopIteration
        deadline = None if timeout is None else _now() + timeout
        while True:
            if self._ordered:
                if self._next_index in self._errors:
                    exc = self._errors.pop(self._next_index)
                    self._next_index += 1
                    self._done += 1
                    raise exc
                if self._next_index in self._buffer:
                    value = self._buffer.pop(self._next_index)
                    self._next_index += 1
                    self._done += 1
                    return value
            else:
                if self._errors:
                    idx, exc = self._errors.popitem()
                    self._done += 1
                    raise exc
                if self._buffer:
                    idx, value = self._buffer.popitem()
                    self._done += 1
                    return value
            self._pool._poll_results()
            if deadline is not None and _now() >= deadline:
                raise TimeoutError("result not ready")
            time.sleep(0.001)


class Pool:
    def __init__(
        self,
        processes: int | None = None,
        initializer: Callable[..., Any] | None = None,
        initargs: Sequence[Any] = (),
        maxtasksperchild: int | None = None,
        context: "Context | None" = None,
    ) -> None:
        if maxtasksperchild is not None:
            if not isinstance(maxtasksperchild, int) or maxtasksperchild <= 0:
                raise ValueError("maxtasksperchild must be a positive int or None")
        if processes is None:
            processes = 1
        self._processes = max(1, int(processes))
        self._initializer = initializer
        self._initargs = tuple(initargs)
        self._ctx = context
        self._maxtasksperchild = maxtasksperchild
        self._start_method = _effective_start_method(context)
        self._workers: list[_Worker] = []
        self._worker_index = 0
        self._next_task_id = 1
        self._pending: dict[int, tuple[Any, int, _Worker]] = {}
        self._closing = False
        self._start_workers()

    def _start_workers(self) -> None:
        for _ in range(self._processes):
            worker = _Worker(
                self._initializer,
                self._initargs,
                self._maxtasksperchild,
                self._start_method,
            )
            self._workers.append(worker)

    def _refresh_workers(self) -> None:
        if self._closing:
            return
        refreshed: list[_Worker] = []
        for worker in self._workers:
            if worker.is_alive():
                refreshed.append(worker)
                continue
            self._consume_worker_messages(worker)
            self._handle_worker_exit(worker)
            worker.close()
            refreshed.append(
                _Worker(
                    self._initializer,
                    self._initargs,
                    self._maxtasksperchild,
                    self._start_method,
                )
            )
        self._workers = refreshed

    def _handle_worker_exit(self, worker: "_Worker") -> None:
        lost: list[tuple[Any, int]] = []
        for task_id, entry in list(self._pending.items()):
            target, idx, owner = entry
            if owner is worker:
                del self._pending[int(task_id)]
                lost.append((target, idx))
        if not lost:
            return
        err = RuntimeError("worker exited")
        for target, idx in lost:
            if isinstance(target, AsyncResult):
                target._mark_error(err)
            elif isinstance(target, IMapIterator):
                target._push_error(int(idx), err)

    def _choose_worker(self) -> "_Worker":
        if not self._workers:
            raise RuntimeError("pool has no workers")
        start = self._worker_index % len(self._workers)
        for offset in range(len(self._workers)):
            worker = self._workers[(start + offset) % len(self._workers)]
            if worker.can_accept_task():
                self._worker_index = (start + offset + 1) % len(self._workers)
                return worker
        raise RuntimeError("no available worker")

    def _dispatch_task(
        self,
        func: Callable[..., Any],
        args: Sequence[Any],
        index: int,
        kwargs: dict[str, Any] | None = None,
    ) -> tuple[int, "_Worker"]:
        while True:
            self._refresh_workers()
            try:
                worker = self._choose_worker()
                break
            except RuntimeError:
                self._poll_results()
                time.sleep(0.001)
        task_id = self._next_task_id
        self._next_task_id += 1
        worker.note_task_sent()
        worker.send((_MSG_TASK, task_id, index, func, tuple(args), dict(kwargs or {})))
        return task_id, worker

    def _consume_worker_messages(self, worker: "_Worker") -> None:
        msg = worker.poll()
        while msg is not None:
            if not isinstance(msg, tuple) or not msg:
                msg = worker.poll()
                continue
            kind = msg[0]
            if kind == _MSG_TASK_RESULT:
                task_id, index, payload = msg[1], msg[2], msg[3]
                entry = self._pending.get(int(task_id))
                if entry is not None:
                    target, idx, owner = entry
                    if isinstance(target, AsyncResult):
                        target._results[idx] = payload
                        del self._pending[int(task_id)]
                    elif isinstance(target, IMapIterator):
                        target._push_result(int(index), payload)
                        del self._pending[int(task_id)]
                    owner.note_task_completed()
                msg = worker.poll()
                continue
            if kind == _MSG_TASK_ERROR:
                task_id, index, payload = msg[1], msg[2], msg[3]
                exc = (
                    payload
                    if isinstance(payload, Exception)
                    else RuntimeError(str(payload))
                )
                entry = self._pending.get(int(task_id))
                if entry is not None:
                    target, idx, owner = entry
                    if isinstance(target, AsyncResult):
                        target._mark_error(exc)
                        del self._pending[int(task_id)]
                    elif isinstance(target, IMapIterator):
                        target._push_error(int(index), exc)
                        del self._pending[int(task_id)]
                    owner.note_task_completed()
                msg = worker.poll()
                continue
            worker.hub().handle_message(msg)
            msg = worker.poll()

    def _poll_results(self) -> None:
        for worker in self._workers:
            self._consume_worker_messages(worker)
        self._refresh_workers()

    def _wait_for(self, async_result: AsyncResult, timeout: float | None) -> None:
        deadline = None if timeout is None else _now() + timeout
        while True:
            self._poll_results()
            if all(task_id not in self._pending for task_id in async_result._task_ids):
                async_result._mark_ready()
                return
            if deadline is not None and _now() >= deadline:
                raise TimeoutError("result not ready")
            time.sleep(0.001)

    def apply_async(
        self,
        func: Callable[..., Any],
        args: Sequence[Any] = (),
        kwds: dict[str, Any] | None = None,
    ) -> AsyncResult:
        results = [None]
        task_id, worker = self._dispatch_task(func, args, 0, kwds)
        async_result = AsyncResult(self, [task_id], results, unwrap_single=True)
        self._pending[task_id] = (async_result, 0, worker)
        return async_result

    def map_async(
        self,
        func: Callable[..., Any],
        iterable: Iterable[Any],
        chunksize: int | None = None,
    ) -> AsyncResult:
        items = list(iterable)
        results: list[Any] = [None] * len(items)
        task_ids: list[int] = []
        async_result = AsyncResult(self, task_ids, results, unwrap_single=False)
        for idx, item in enumerate(items):
            task_id, worker = self._dispatch_task(func, (item,), idx)
            task_ids.append(task_id)
            self._pending[task_id] = (async_result, idx, worker)
        return async_result

    def map(
        self,
        func: Callable[..., Any],
        iterable: Iterable[Any],
        chunksize: int | None = None,
    ) -> list[Any]:
        return self.map_async(func, iterable, chunksize).get(timeout=None)

    def starmap_async(
        self,
        func: Callable[..., Any],
        iterable: Iterable[Sequence[Any]],
        chunksize: int | None = None,
    ) -> AsyncResult:
        items = list(iterable)
        results: list[Any] = [None] * len(items)
        task_ids: list[int] = []
        async_result = AsyncResult(self, task_ids, results, unwrap_single=False)
        for idx, item in enumerate(items):
            task_id, worker = self._dispatch_task(func, item, idx)
            task_ids.append(task_id)
            self._pending[task_id] = (async_result, idx, worker)
        return async_result

    def starmap(
        self,
        func: Callable[..., Any],
        iterable: Iterable[Sequence[Any]],
        chunksize: int | None = None,
    ) -> list[Any]:
        return self.starmap_async(func, iterable, chunksize).get(timeout=None)

    def imap(
        self,
        func: Callable[..., Any],
        iterable: Iterable[Any],
        chunksize: int = 1,
    ) -> IMapIterator:
        items = list(iterable)
        iterator = IMapIterator(self, ordered=True, total=len(items))
        for idx, item in enumerate(items):
            task_id, worker = self._dispatch_task(func, (item,), idx)
            self._pending[task_id] = (iterator, idx, worker)
        return iterator

    def imap_unordered(
        self,
        func: Callable[..., Any],
        iterable: Iterable[Any],
        chunksize: int = 1,
    ) -> IMapIterator:
        items = list(iterable)
        iterator = IMapIterator(self, ordered=False, total=len(items))
        for idx, item in enumerate(items):
            task_id, worker = self._dispatch_task(func, (item,), idx)
            self._pending[task_id] = (iterator, idx, worker)
        return iterator

    def close(self) -> None:
        if self._closing:
            return
        self._closing = True
        for worker in self._workers:
            worker.send((_MSG_CLOSE,))

    def terminate(self) -> None:
        self._closing = True
        for worker in self._workers:
            worker.terminate()

    def join(self) -> None:
        for worker in self._workers:
            worker.join()

    def __enter__(self) -> "Pool":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()
        self.join()


class _Worker:
    def __init__(
        self,
        initializer: Callable[..., Any] | None,
        initargs: Sequence[Any],
        maxtasksperchild: int | None,
        start_method: str,
    ) -> None:
        self._handle: Any | None = None
        self._hub: _Hub | None = None
        self._transport: _StreamTransport | None = None
        self._maxtasksperchild = maxtasksperchild
        self._tasks_sent = 0
        self._tasks_completed = 0
        self._start_method = start_method
        self._spawn(initializer, initargs, start_method)

    def _spawn(
        self,
        initializer: Callable[..., Any] | None,
        initargs: Sequence[Any],
        start_method: str,
    ) -> None:
        if _MOLT_PROCESS_SPAWN is None:
            raise RuntimeError("worker spawn unavailable")
        _require_process_capability()
        entry_module = _resolve_entry_module_name()
        env = _build_spawn_env()
        env[_ENTRY_OVERRIDE_ENV] = _MP_ENTRY_OVERRIDE
        env[_MP_ENTRY_ENV] = entry_module
        env[_MP_SPAWN_ENV] = "1"
        env[_MP_START_METHOD_ENV] = start_method
        main_mod = sys.modules.get("__main__")
        main_path = (
            getattr(main_mod, "__file__", None) if main_mod is not None else None
        )
        if not isinstance(main_path, str) or not main_path:
            argv0 = sys.argv[0] if sys.argv else None
            if isinstance(argv0, str) and argv0:
                main_path = argv0
        _mp_debug(f"mp: worker spawn entry={entry_module!r} main_path={main_path!r}")
        if isinstance(main_path, str) and main_path:
            try:
                env[_MP_MAIN_PATH_ENV] = os.path.abspath(main_path)
            except Exception:
                pass
        trusted = _get_env_value("MOLT_TRUSTED", "")
        if trusted:
            env["MOLT_TRUSTED"] = trusted
        caps = _get_env_value("MOLT_CAPABILITIES", "")
        if caps:
            env["MOLT_CAPABILITIES"] = caps
        mp_debug = _get_env_value("MOLT_MP_DEBUG", "")
        if mp_debug:
            env["MOLT_MP_DEBUG"] = mp_debug
        args = [sys.argv[0]]
        try:
            cwd = os.getcwd()
        except Exception:
            cwd = None
        spawn_proc = _require_intrinsic(_MOLT_PROCESS_SPAWN, "molt_process_spawn")
        handle = spawn_proc(
            args,
            env,
            cwd,
            _STDIO_PIPE,
            _STDIO_PIPE,
            _STDIO_INHERIT,
        )
        if handle is None:
            raise RuntimeError("worker spawn failed")
        self._handle = handle
        stdin_stream = _require_intrinsic(_MOLT_PROCESS_STDIN, "molt_process_stdin")(
            handle
        )
        stdout_stream = _require_intrinsic(_MOLT_PROCESS_STDOUT, "molt_process_stdout")(
            handle
        )
        self._transport = _StreamTransport(stdout_stream, stdin_stream)
        self._hub = _Hub(
            self._transport,
            "parent",
            recv_stream=stdout_stream,
            send_stream=stdin_stream,
        )
        self._hub.send(
            (_MSG_WORKER, initializer, tuple(initargs), self._maxtasksperchild)
        )

    def send(self, message: Any) -> None:
        if self._hub is None:
            return
        self._hub.send(message)

    def poll(self) -> Any | None:
        if self._hub is None:
            return None
        return self._hub.poll()

    def hub(self) -> _Hub:
        if self._hub is None:
            raise RuntimeError("worker not initialized")
        return self._hub

    def can_accept_task(self) -> bool:
        if not self.is_alive():
            return False
        if self._maxtasksperchild is None:
            return True
        return self._tasks_sent < self._maxtasksperchild

    def note_task_sent(self) -> None:
        self._tasks_sent += 1

    def note_task_completed(self) -> None:
        self._tasks_completed += 1

    def is_alive(self) -> bool:
        if self._handle is None:
            return False
        proc_return = _require_intrinsic(
            _MOLT_PROCESS_RETURN, "molt_process_returncode"
        )
        return proc_return(self._handle) is None

    def terminate(self) -> None:
        if self._handle is not None and _MOLT_PROCESS_TERM is not None:
            _MOLT_PROCESS_TERM(self._handle)

    def join(self) -> None:
        if self._handle is None:
            return
        proc_return = _require_intrinsic(
            _MOLT_PROCESS_RETURN, "molt_process_returncode"
        )
        while proc_return(self._handle) is None:
            time.sleep(0.001)
        if self._transport is not None:
            self._transport.close()

    def close(self) -> None:
        if self._transport is not None:
            self._transport.close()


ProcessType = Process
PoolType = Pool


class Context:
    def __init__(self, method: str | None) -> None:
        self._method = method or "spawn"

    def get_start_method(self) -> str:
        return self._method

    def Process(self, *args: Any, **kwargs: Any) -> ProcessType:
        return Process(*args, **kwargs, ctx=self)  # type: ignore[parameter-already-assigned]

    def Queue(self, maxsize: int = 0) -> _Queue:
        return _Queue(maxsize=maxsize)

    def Pipe(self, duplex: bool = True) -> tuple[PipeConnection, PipeConnection]:
        pipe_id = _next_object_id()
        if duplex:
            left = PipeConnection(pipe_id, True, True)
            right = PipeConnection(pipe_id, True, True)
            left._peer = right
            right._peer = left
            return (
                left,
                right,
            )
        left = PipeConnection(pipe_id, True, False)
        right = PipeConnection(pipe_id, False, True)
        left._peer = right
        right._peer = left
        return (left, right)

    def Pool(
        self,
        processes: int | None = None,
        initializer: Callable[..., Any] | None = None,
        initargs: Sequence[Any] = (),
        maxtasksperchild: int | None = None,
    ) -> PoolType:
        return Pool(
            processes=processes,
            initializer=initializer,
            initargs=initargs,
            maxtasksperchild=maxtasksperchild,
            context=self,
        )

    def Value(self, typecode: str, value: Any) -> SharedValue:
        return SharedValue(typecode, value)

    def Array(self, typecode: str, values: Iterable[Any]) -> SharedArray:
        return SharedArray(typecode, values)


_DEFAULT_START_METHOD: str | None = None


def get_all_start_methods() -> list[str]:
    if os.name == "nt":
        return ["spawn"]
    return ["spawn", "fork", "forkserver"]


def set_start_method(method: str, force: bool = False) -> None:
    global _DEFAULT_START_METHOD
    if _DEFAULT_START_METHOD is not None and not force:
        raise RuntimeError("context already set")
    if method not in get_all_start_methods():
        raise ValueError("unknown start method")
    _DEFAULT_START_METHOD = method


def get_start_method(allow_none: bool = False) -> str | None:
    if _DEFAULT_START_METHOD is None:
        return None if allow_none else "spawn"
    return _DEFAULT_START_METHOD


def get_context(method: str | None = None) -> Context:
    if method is None:
        method = get_start_method(allow_none=False)
    if method not in get_all_start_methods():
        raise ValueError("unknown start method")
    if method != "spawn":
        # TODO(runtime, owner:runtime, milestone:RT3, priority:P1, status:divergent):
        # Fork/forkserver currently map to spawn semantics; implement true fork support.
        return Context(method)
    return Context(method)


def _effective_start_method(ctx: "Context | None") -> str:
    if ctx is not None:
        return ctx.get_start_method()
    method = get_start_method(allow_none=True)
    return method if method is not None else "spawn"


def Queue(maxsize: int = 0) -> _Queue:
    return get_context("spawn").Queue(maxsize)


def Pipe(duplex: bool = True) -> tuple[PipeConnection, PipeConnection]:
    return get_context("spawn").Pipe(duplex=duplex)


def Value(typecode: str, value: Any) -> SharedValue:
    return get_context("spawn").Value(typecode, value)


def Array(typecode: str, values: Iterable[Any]) -> SharedArray:
    return get_context("spawn").Array(typecode, values)


def _spawn_main() -> None:
    _mp_debug("mp: spawn_main start")
    start_method = _get_env_value(_MP_START_METHOD_ENV, "")
    if start_method:
        global _DEFAULT_START_METHOD
        _DEFAULT_START_METHOD = start_method
    entry_module = _get_env_value(_MP_ENTRY_ENV, "__main__")
    main_path = _get_env_value(_MP_MAIN_PATH_ENV, "")
    _mp_debug(f"mp: spawn_main entry={entry_module!r} main_path={main_path!r}")
    candidate_names: list[str] = []
    if entry_module and entry_module != "__main__":
        candidate_names.append(entry_module)
    if main_path:
        resolved = _module_name_from_path(main_path)
        if resolved and resolved not in candidate_names and resolved != "__main__":
            candidate_names.append(resolved)
    for module_name in candidate_names:
        try:
            mod = importlib.import_module(module_name)
            sys.modules["__main__"] = mod
            sys.modules["__mp_main__"] = mod
            _mp_debug(
                "mp: spawn_main loaded __main__ from module " + f"{module_name!r}"
            )
            break
        except Exception as exc:
            _mp_debug(
                f"mp: spawn_main module import failed {module_name!r}: "
                + f"{type(exc).__name__}: {exc}"
            )
    reader = open(0, "rb", closefd=False)
    writer = open(1, "wb", closefd=False)
    transport = _FdTransport(reader, writer)
    hub = _Hub(transport, "child")
    try:
        msg = hub.recv(timeout=None)
        _mp_debug(f"mp: spawn_main received msg_type={type(msg).__name__}")
    except BaseException as exc:
        _mp_debug(f"mp: spawn_main recv error {type(exc).__name__}: {exc}")
        try:
            hub.send(
                (
                    _MSG_TASK_ERROR,
                    0,
                    0,
                    _exception_ref_from_exc(exc),
                )
            )
        except Exception:
            pass
        return
    if not isinstance(msg, tuple) or not msg:
        return
    kind = msg[0]
    if kind == _MSG_RUN:
        _run_child(hub, msg)
        return
    if kind == _MSG_WORKER:
        _worker_loop(hub, msg)
        return


def _run_child(hub: _Hub, msg: tuple[Any, ...]) -> None:
    target = msg[1]
    args = msg[2] if len(msg) > 2 else ()
    kwargs = msg[3] if len(msg) > 3 else {}
    if _mp_debug_enabled():
        _mp_debug(
            f"mp: run_child target_type={type(target).__name__} args_len={len(args) if hasattr(args, '__len__') else 'na'}"
        )
    try:
        if not callable(target):
            raise TypeError("target is not callable")
        if kwargs:
            target(*args, **kwargs)
        else:
            target(*args)
        sys.exit(0)
    except SystemExit as exc:
        code = exc.code
        if code is None:
            sys.exit(0)
        if isinstance(code, int):
            sys.exit(code)
        sys.exit(1)
    except BaseException as exc:
        hub.send((_MSG_TASK_ERROR, 0, 0, _exception_ref_from_exc(exc)))
        sys.exit(1)


def _worker_loop(hub: _Hub, msg: tuple[Any, ...]) -> None:
    initializer = msg[1] if len(msg) > 1 else None
    initargs = msg[2] if len(msg) > 2 else ()
    max_tasks = msg[3] if len(msg) > 3 else None
    if not isinstance(max_tasks, int) or max_tasks <= 0:
        max_tasks = None
    if callable(initializer):
        try:
            initializer(*initargs)
        except Exception as exc:
            _mp_debug(f"mp: worker initializer failed {type(exc).__name__}: {exc}")
    tasks_done = 0
    while True:
        try:
            message = hub.recv(timeout=None)
        except Exception as exc:
            _mp_debug(f"mp: worker recv error {type(exc).__name__}: {exc}")
            break
        if not isinstance(message, tuple) or not message:
            continue
        kind = message[0]
        if kind == _MSG_CLOSE:
            break
        if kind != _MSG_TASK:
            hub.handle_message(message)
            continue
        task_id, index, func, args, kwargs = (
            message[1],
            message[2],
            message[3],
            message[4],
            message[5],
        )
        try:
            if kwargs:
                result = func(*args, **kwargs)
            else:
                result = func(*args)
            hub.send((_MSG_TASK_RESULT, task_id, index, result))
        except BaseException as exc:
            hub.send(
                (
                    _MSG_TASK_ERROR,
                    task_id,
                    index,
                    _exception_ref_from_exc(exc),
                )
            )
        tasks_done += 1
        if max_tasks is not None and tasks_done >= max_tasks:
            break
    sys.exit(0)
