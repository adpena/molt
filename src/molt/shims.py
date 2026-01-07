"""CPython fallback for Molt intrinsics used in tests.

If a runtime shared library is available, the shims will bind to it. Otherwise
they fall back to a minimal blocking implementation for local test execution.
"""

from __future__ import annotations

import atexit
import asyncio
import ctypes
import os
import queue
import threading
import time
from pathlib import Path
from typing import Any

from molt import net as net_mod

_loop: asyncio.AbstractEventLoop | None = None
_thread: threading.Thread | None = None
_runtime_lib: ctypes.CDLL | None = None
_QNAN = 0x7FF8_0000_0000_0000
_TAG_INT = 0x0001_0000_0000_0000
_INT_MASK = 140737488355327
_PENDING = 0x7FFD_0000_0000_0000


def _box_int(value: int) -> int:
    if value < 0:
        raise ValueError("molt shim only supports non-negative ints")
    return _QNAN + _TAG_INT + value


def _use_runtime_concurrency() -> bool:
    return os.environ.get("MOLT_SHIMS_RUNTIME_CONCURRENCY") == "1"


def _run_loop(loop: asyncio.AbstractEventLoop) -> None:
    asyncio.set_event_loop(loop)
    loop.run_forever()


def _shutdown() -> None:
    if _loop is None:
        return
    _loop.call_soon_threadsafe(_loop.stop)
    if _thread is not None:
        _thread.join(1)


def _ensure_loop() -> asyncio.AbstractEventLoop:
    global _loop, _thread
    if _loop is None:
        _loop = asyncio.new_event_loop()
        _thread = threading.Thread(
            None,
            _run_loop,
            "molt-shim-loop",
            (_loop,),
            None,
        )
        _thread.daemon = True
        _thread.start()
        atexit.register(_shutdown)
    return _loop


def _find_runtime_lib() -> Path | None:
    env_path = os.environ.get("MOLT_RUNTIME_LIB")
    if env_path:
        path = Path(env_path)
        if path.exists():
            return path
    return None


def _bind_required(
    lib: ctypes.CDLL, name: str, argtypes: list[Any], restype: Any
) -> None:
    func = getattr(lib, name)
    func.argtypes = argtypes
    func.restype = restype


def _bind_optional(
    lib: ctypes.CDLL, name: str, argtypes: list[Any], restype: Any
) -> None:
    func = getattr(lib, name, None)
    if func is None:
        return
    func.argtypes = argtypes
    func.restype = restype


def _configure_runtime_lib(lib: ctypes.CDLL) -> None:
    _bind_required(lib, "molt_chan_new", [ctypes.c_uint64], ctypes.c_void_p)
    _bind_required(
        lib, "molt_chan_send", [ctypes.c_void_p, ctypes.c_longlong], ctypes.c_longlong
    )
    _bind_required(lib, "molt_chan_recv", [ctypes.c_void_p], ctypes.c_longlong)
    _bind_required(lib, "molt_spawn", [ctypes.c_void_p], None)
    _bind_required(
        lib,
        "molt_json_parse_int",
        [ctypes.c_char_p, ctypes.c_size_t],
        ctypes.c_longlong,
    )
    _bind_optional(
        lib,
        "molt_json_parse_scalar",
        [ctypes.c_char_p, ctypes.c_size_t, ctypes.POINTER(ctypes.c_uint64)],
        ctypes.c_int,
    )
    _bind_optional(
        lib,
        "molt_msgpack_parse_scalar",
        [ctypes.c_char_p, ctypes.c_size_t, ctypes.POINTER(ctypes.c_uint64)],
        ctypes.c_int,
    )
    _bind_optional(
        lib,
        "molt_cbor_parse_scalar",
        [ctypes.c_char_p, ctypes.c_size_t, ctypes.POINTER(ctypes.c_uint64)],
        ctypes.c_int,
    )
    _bind_optional(lib, "molt_handle_resolve", [ctypes.c_uint64], ctypes.c_uint64)
    _bind_optional(
        lib,
        "molt_bytes_from_bytes",
        [ctypes.c_char_p, ctypes.c_size_t, ctypes.POINTER(ctypes.c_uint64)],
        ctypes.c_int,
    )
    _bind_optional(lib, "molt_stream_new", [ctypes.c_size_t], ctypes.c_void_p)
    _bind_optional(
        lib,
        "molt_stream_send",
        [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t],
        ctypes.c_longlong,
    )
    _bind_optional(lib, "molt_stream_recv", [ctypes.c_void_p], ctypes.c_longlong)
    _bind_optional(lib, "molt_stream_close", [ctypes.c_void_p], None)
    _bind_optional(
        lib,
        "molt_ws_pair",
        [
            ctypes.c_size_t,
            ctypes.POINTER(ctypes.c_void_p),
            ctypes.POINTER(ctypes.c_void_p),
        ],
        ctypes.c_int,
    )
    _bind_optional(
        lib,
        "molt_ws_connect",
        [ctypes.c_char_p, ctypes.c_size_t, ctypes.POINTER(ctypes.c_void_p)],
        ctypes.c_int,
    )
    _bind_optional(lib, "molt_ws_set_connect_hook", [ctypes.c_size_t], None)
    _bind_optional(
        lib,
        "molt_ws_new_with_hooks",
        [ctypes.c_size_t, ctypes.c_size_t, ctypes.c_size_t, ctypes.c_void_p],
        ctypes.c_void_p,
    )
    _bind_optional(
        lib,
        "molt_ws_send",
        [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t],
        ctypes.c_longlong,
    )
    _bind_optional(lib, "molt_ws_recv", [ctypes.c_void_p], ctypes.c_longlong)
    _bind_optional(lib, "molt_ws_close", [ctypes.c_void_p], None)


def _open_runtime_lib(lib_path: Path) -> ctypes.CDLL | None:
    try:
        return ctypes.CDLL(str(lib_path))
    except OSError:
        return None


def load_runtime() -> ctypes.CDLL | None:
    global _runtime_lib
    if _runtime_lib is not None:
        return _runtime_lib

    lib_path = _find_runtime_lib()
    if lib_path is None:
        return None

    lib = _open_runtime_lib(lib_path)
    if lib is None:
        return None

    _configure_runtime_lib(lib)
    _runtime_lib = lib
    return _runtime_lib


def _chan_ptr(chan: Any) -> ctypes.c_void_p | None:
    if isinstance(chan, ctypes.c_void_p):
        return chan
    if isinstance(chan, int):
        return ctypes.c_void_p(chan)
    return None


def molt_spawn(task: Any) -> None:
    if _use_runtime_concurrency():
        lib = load_runtime()
        if lib is not None:
            task_ptr = _chan_ptr(task)
            if task_ptr is not None:
                lib.molt_spawn(task_ptr)
                return
    loop = _ensure_loop()
    if asyncio.iscoroutine(task):
        asyncio.run_coroutine_threadsafe(task, loop)
        return
    if callable(task):
        asyncio.run_coroutine_threadsafe(task(), loop)
        return
    raise TypeError("molt_spawn expects a coroutine or callable")


def molt_block_on(task: Any) -> Any:
    loop = _ensure_loop()
    if asyncio.iscoroutine(task) or isinstance(task, asyncio.Future):
        fut = asyncio.run_coroutine_threadsafe(task, loop)
        return fut.result()
    if callable(task):
        return molt_block_on(task())
    raise TypeError("molt_block_on expects a coroutine or callable")


def molt_async_sleep(_delay: float = 0.0) -> Any:
    async def _sleep() -> None:
        await asyncio.sleep(0)
        return None

    return _sleep()


def molt_chan_new(maxsize: int = 0) -> Any:
    if _use_runtime_concurrency():
        lib = load_runtime()
        if lib is not None:
            ptr = lib.molt_chan_new(_box_int(maxsize))
            return ctypes.c_void_p(ptr)
    return queue.Queue(maxsize)


def molt_chan_send(chan: Any, val: Any) -> int:
    if _use_runtime_concurrency():
        lib = load_runtime()
        if lib is not None:
            chan_ptr = _chan_ptr(chan)
            if chan_ptr is not None:
                for _ in range(1000):
                    res = int(lib.molt_chan_send(chan_ptr, int(val)))
                    if res != _PENDING:
                        return res
                    time.sleep(0)
                raise RuntimeError("molt_chan_send pending")
    if isinstance(chan, queue.Queue):
        chan.put(val)
        return 0
    raise TypeError("molt_chan_send expected a channel handle")


def molt_chan_recv(chan: Any) -> Any:
    if _use_runtime_concurrency():
        lib = load_runtime()
        if lib is not None:
            chan_ptr = _chan_ptr(chan)
            if chan_ptr is not None:
                for _ in range(1000):
                    res = int(lib.molt_chan_recv(chan_ptr))
                    if res != _PENDING:
                        return res
                    time.sleep(0)
                raise RuntimeError("molt_chan_recv pending")
    if isinstance(chan, queue.Queue):
        getter = chan.get
        return getter()
    raise TypeError("molt_chan_recv expected a channel handle")


def install() -> None:
    import builtins

    setattr(builtins, "molt_spawn", molt_spawn)
    setattr(builtins, "molt_chan_new", molt_chan_new)
    setattr(builtins, "molt_chan_send", molt_chan_send)
    setattr(builtins, "molt_chan_recv", molt_chan_recv)
    setattr(builtins, "molt_block_on", molt_block_on)
    setattr(builtins, "molt_async_sleep", molt_async_sleep)
    setattr(builtins, "molt_stream", net_mod.stream)
    setattr(builtins, "molt_stream_channel", net_mod.stream_channel)
    setattr(builtins, "molt_ws_pair", net_mod.ws_pair)
    setattr(builtins, "molt_ws_connect", net_mod.ws_connect)
