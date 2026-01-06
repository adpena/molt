"""CPython fallback for Molt intrinsics used in tests.

If a runtime shared library is available, the shims will bind to it. Otherwise
they fall back to a minimal blocking implementation for local test execution.
"""

from __future__ import annotations

import atexit
import asyncio
import builtins
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
_INT_MASK = (1 << 47) - 1
_PENDING = 0x7FFD_0000_0000_0000


def _box_int(value: int) -> int:
    return _QNAN | _TAG_INT | (int(value) & _INT_MASK)


def _use_runtime_concurrency() -> bool:
    return os.environ.get("MOLT_SHIMS_RUNTIME_CONCURRENCY") == "1"


def _run_loop(loop: asyncio.AbstractEventLoop) -> None:
    asyncio.set_event_loop(loop)
    loop.run_forever()


def _ensure_loop() -> asyncio.AbstractEventLoop:
    global _loop, _thread
    if _loop is None:
        _loop = asyncio.new_event_loop()
        _thread = threading.Thread(
            target=_run_loop,
            args=(_loop,),
            name="molt-shim-loop",
            daemon=True,
        )
        _thread.start()
        atexit.register(_shutdown)
    return _loop


def _shutdown() -> None:
    if _loop is None:
        return
    _loop.call_soon_threadsafe(_loop.stop)
    if _thread is not None:
        _thread.join(timeout=1)


def _find_runtime_lib() -> Path | None:
    env_path = os.environ.get("MOLT_RUNTIME_LIB")
    if env_path:
        path = Path(env_path)
        if path.exists():
            return path

    root = Path(__file__).resolve().parents[2]
    candidates = [
        root / "target" / "release" / "libmolt_runtime.dylib",
        root / "target" / "release" / "libmolt_runtime.so",
        root / "target" / "release" / "molt_runtime.dll",
    ]
    for candidate in candidates:
        if candidate.exists():
            return candidate
    return None


def load_runtime() -> ctypes.CDLL | None:
    global _runtime_lib
    if _runtime_lib is not None:
        return _runtime_lib

    lib_path = _find_runtime_lib()
    if lib_path is None:
        return None

    try:
        lib = ctypes.CDLL(str(lib_path))
    except OSError:
        return None

    lib.molt_chan_new.argtypes = [ctypes.c_uint64]
    lib.molt_chan_new.restype = ctypes.c_void_p
    lib.molt_chan_send.argtypes = [ctypes.c_void_p, ctypes.c_longlong]
    lib.molt_chan_send.restype = ctypes.c_longlong
    lib.molt_chan_recv.argtypes = [ctypes.c_void_p]
    lib.molt_chan_recv.restype = ctypes.c_longlong
    lib.molt_spawn.argtypes = [ctypes.c_void_p]
    lib.molt_spawn.restype = None
    lib.molt_json_parse_int.argtypes = [ctypes.c_char_p, ctypes.c_size_t]
    lib.molt_json_parse_int.restype = ctypes.c_longlong
    if hasattr(lib, "molt_json_parse_scalar"):
        lib.molt_json_parse_scalar.argtypes = [
            ctypes.c_char_p,
            ctypes.c_size_t,
            ctypes.POINTER(ctypes.c_uint64),
        ]
        lib.molt_json_parse_scalar.restype = ctypes.c_int
    if hasattr(lib, "molt_msgpack_parse_scalar"):
        lib.molt_msgpack_parse_scalar.argtypes = [
            ctypes.c_char_p,
            ctypes.c_size_t,
            ctypes.POINTER(ctypes.c_uint64),
        ]
        lib.molt_msgpack_parse_scalar.restype = ctypes.c_int
    if hasattr(lib, "molt_cbor_parse_scalar"):
        lib.molt_cbor_parse_scalar.argtypes = [
            ctypes.c_char_p,
            ctypes.c_size_t,
            ctypes.POINTER(ctypes.c_uint64),
        ]
        lib.molt_cbor_parse_scalar.restype = ctypes.c_int
    if hasattr(lib, "molt_bytes_from_bytes"):
        lib.molt_bytes_from_bytes.argtypes = [
            ctypes.c_char_p,
            ctypes.c_size_t,
            ctypes.POINTER(ctypes.c_uint64),
        ]
        lib.molt_bytes_from_bytes.restype = ctypes.c_int
    if hasattr(lib, "molt_stream_new"):
        lib.molt_stream_new.argtypes = [ctypes.c_size_t]
        lib.molt_stream_new.restype = ctypes.c_void_p
    if hasattr(lib, "molt_stream_send"):
        lib.molt_stream_send.argtypes = [
            ctypes.c_void_p,
            ctypes.c_char_p,
            ctypes.c_size_t,
        ]
        lib.molt_stream_send.restype = ctypes.c_longlong
    if hasattr(lib, "molt_stream_recv"):
        lib.molt_stream_recv.argtypes = [ctypes.c_void_p]
        lib.molt_stream_recv.restype = ctypes.c_longlong
    if hasattr(lib, "molt_stream_close"):
        lib.molt_stream_close.argtypes = [ctypes.c_void_p]
        lib.molt_stream_close.restype = None
    if hasattr(lib, "molt_ws_pair"):
        lib.molt_ws_pair.argtypes = [
            ctypes.c_size_t,
            ctypes.POINTER(ctypes.c_void_p),
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.molt_ws_pair.restype = ctypes.c_int
    if hasattr(lib, "molt_ws_connect"):
        lib.molt_ws_connect.argtypes = [
            ctypes.c_char_p,
            ctypes.c_size_t,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        lib.molt_ws_connect.restype = ctypes.c_int
    if hasattr(lib, "molt_ws_set_connect_hook"):
        lib.molt_ws_set_connect_hook.argtypes = [ctypes.c_size_t]
        lib.molt_ws_set_connect_hook.restype = None
    if hasattr(lib, "molt_ws_new_with_hooks"):
        lib.molt_ws_new_with_hooks.argtypes = [
            ctypes.c_size_t,
            ctypes.c_size_t,
            ctypes.c_size_t,
            ctypes.c_void_p,
        ]
        lib.molt_ws_new_with_hooks.restype = ctypes.c_void_p
    if hasattr(lib, "molt_ws_send"):
        lib.molt_ws_send.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
        lib.molt_ws_send.restype = ctypes.c_longlong
    if hasattr(lib, "molt_ws_recv"):
        lib.molt_ws_recv.argtypes = [ctypes.c_void_p]
        lib.molt_ws_recv.restype = ctypes.c_longlong
    if hasattr(lib, "molt_ws_close"):
        lib.molt_ws_close.argtypes = [ctypes.c_void_p]
        lib.molt_ws_close.restype = None

    _runtime_lib = lib
    return _runtime_lib


def _chan_ptr(chan: Any) -> ctypes.c_void_p | None:
    if isinstance(chan, ctypes.c_void_p):
        return chan
    if isinstance(chan, int):
        return ctypes.c_void_p(chan)
    return None


def molt_spawn(task: Any) -> None:
    lib = load_runtime() if _use_runtime_concurrency() else None
    task_ptr = _chan_ptr(task)
    if lib is not None and task_ptr is not None:
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


def molt_chan_new(maxsize: int = 0) -> Any:
    lib = load_runtime() if _use_runtime_concurrency() else None
    if lib is not None:
        ptr = lib.molt_chan_new(_box_int(maxsize))
        return ctypes.c_void_p(ptr)
    return queue.Queue(maxsize=maxsize)


def molt_chan_send(chan: Any, val: Any) -> int:
    lib = load_runtime() if _use_runtime_concurrency() else None
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
    lib = load_runtime() if _use_runtime_concurrency() else None
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
        return chan.get()
    raise TypeError("molt_chan_recv expected a channel handle")


def install() -> None:
    setattr(builtins, "molt_spawn", molt_spawn)
    setattr(builtins, "molt_chan_new", molt_chan_new)
    setattr(builtins, "molt_chan_send", molt_chan_send)
    setattr(builtins, "molt_chan_recv", molt_chan_recv)
    setattr(builtins, "molt_stream", net_mod.stream)
    setattr(builtins, "molt_stream_channel", net_mod.stream_channel)
    setattr(builtins, "molt_ws_pair", net_mod.ws_pair)
    setattr(builtins, "molt_ws_connect", net_mod.ws_connect)
