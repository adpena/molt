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
import socket as _socket
import select as _select
import types as _types
from pathlib import Path
from typing import Any, cast

from molt import net as net_mod

_loop: asyncio.AbstractEventLoop | None = None
_thread: threading.Thread | None = None
_runtime_lib: ctypes.CDLL | None = None
_QNAN = 0x7FF8_0000_0000_0000
_TAG_INT = 0x0001_0000_0000_0000
_INT_MASK = 140737488355327
_PENDING = 0x7FFD_0000_0000_0000
_CANCEL_TOKENS: dict[int, dict[str, int | bool]] = {
    1: {"parent": 0, "cancelled": False, "refs": 1}
}
_CANCEL_NEXT_ID = 2
_CANCEL_CURRENT = 1
_ORIG_ASYNCIO_SLEEP: Any | None = None
_SOCKET_NEXT_ID = 1
_SOCKET_HANDLES: dict[int, "_SocketHandle"] = {}


class _SocketHandle:
    def __init__(self, sock: _socket.socket) -> None:
        self.sock = sock
        self.refs = 1
        self.closed = False

    def close(self) -> None:
        if self.closed:
            return
        self.closed = True
        try:
            self.sock.close()
        except Exception:
            pass


def _register_socket(sock: _socket.socket) -> int:
    global _SOCKET_NEXT_ID
    handle_id = _SOCKET_NEXT_ID
    _SOCKET_NEXT_ID += 1
    _SOCKET_HANDLES[handle_id] = _SocketHandle(sock)
    return handle_id


def _get_socket_handle(handle: Any) -> _SocketHandle | None:
    if isinstance(handle, _SocketHandle):
        return handle
    if isinstance(handle, int):
        entry = _SOCKET_HANDLES.get(handle)
        if entry is not None:
            return entry
        # TODO(perf, owner:tooling, milestone:TL2, priority:P2, status:planned): maintain fd->handle mapping to avoid linear scan when add_reader/add_writer pass raw fds.
        for candidate in _SOCKET_HANDLES.values():
            try:
                if candidate.sock.fileno() == handle:
                    return candidate
            except Exception:
                continue
    return None


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
    _bind_required(lib, "molt_chan_drop", [ctypes.c_void_p], None)
    _bind_required(lib, "molt_spawn", [ctypes.c_void_p], None)
    _bind_required(lib, "molt_cancel_token_new", [ctypes.c_longlong], ctypes.c_longlong)
    _bind_required(
        lib, "molt_cancel_token_clone", [ctypes.c_longlong], ctypes.c_longlong
    )
    _bind_required(
        lib, "molt_cancel_token_drop", [ctypes.c_longlong], ctypes.c_longlong
    )
    _bind_required(
        lib, "molt_cancel_token_cancel", [ctypes.c_longlong], ctypes.c_longlong
    )
    _bind_required(
        lib, "molt_cancel_token_is_cancelled", [ctypes.c_longlong], ctypes.c_longlong
    )
    _bind_required(
        lib, "molt_cancel_token_set_current", [ctypes.c_longlong], ctypes.c_longlong
    )
    _bind_required(lib, "molt_cancel_token_get_current", [], ctypes.c_longlong)
    _bind_required(lib, "molt_cancelled", [], ctypes.c_longlong)
    _bind_required(lib, "molt_cancel_current", [], ctypes.c_longlong)
    _bind_required(lib, "molt_future_cancel", [ctypes.c_longlong], ctypes.c_longlong)
    _bind_required(
        lib,
        "molt_task_register_token_owned",
        [ctypes.c_longlong, ctypes.c_longlong],
        ctypes.c_longlong,
    )
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
    _bind_required(lib, "molt_stream_new", [ctypes.c_size_t], ctypes.c_void_p)
    _bind_required(
        lib,
        "molt_stream_send",
        [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t],
        ctypes.c_longlong,
    )
    _bind_required(lib, "molt_stream_recv", [ctypes.c_void_p], ctypes.c_longlong)
    _bind_required(lib, "molt_stream_close", [ctypes.c_void_p], None)
    _bind_required(lib, "molt_stream_drop", [ctypes.c_void_p], None)
    _bind_required(
        lib,
        "molt_ws_pair",
        [
            ctypes.c_size_t,
            ctypes.POINTER(ctypes.c_void_p),
            ctypes.POINTER(ctypes.c_void_p),
        ],
        ctypes.c_int,
    )
    _bind_required(
        lib,
        "molt_ws_connect",
        [ctypes.c_char_p, ctypes.c_size_t, ctypes.POINTER(ctypes.c_void_p)],
        ctypes.c_int,
    )
    _bind_required(lib, "molt_ws_set_connect_hook", [ctypes.c_size_t], None)
    _bind_required(
        lib,
        "molt_ws_new_with_hooks",
        [ctypes.c_size_t, ctypes.c_size_t, ctypes.c_size_t, ctypes.c_void_p],
        ctypes.c_void_p,
    )
    _bind_required(
        lib,
        "molt_ws_send",
        [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t],
        ctypes.c_longlong,
    )
    _bind_required(lib, "molt_ws_recv", [ctypes.c_void_p], ctypes.c_longlong)
    _bind_required(lib, "molt_ws_close", [ctypes.c_void_p], None)
    _bind_required(lib, "molt_ws_drop", [ctypes.c_void_p], None)


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


def stream_new_handle(lib: ctypes.CDLL | None, maxsize: int) -> ctypes.c_void_p | None:
    if lib is None:
        return None
    handle = lib.molt_stream_new(maxsize)
    if isinstance(handle, ctypes.c_void_p):
        return handle
    if isinstance(handle, int):
        return ctypes.c_void_p(handle)
    return None


def ws_pair_handles(
    lib: ctypes.CDLL | None, maxsize: int
) -> tuple[ctypes.c_void_p, ctypes.c_void_p] | None:
    if lib is None:
        return None
    left_handle = ctypes.c_void_p()
    right_handle = ctypes.c_void_p()
    rc = lib.molt_ws_pair(
        maxsize,
        ctypes.byref(left_handle),
        ctypes.byref(right_handle),
    )
    if rc == 0 and left_handle.value and right_handle.value:
        return left_handle, right_handle
    return None


def ws_connect_handle(lib: ctypes.CDLL | None, url: str) -> ctypes.c_void_p | None:
    if lib is None:
        return None
    buf = url.encode("utf-8")
    handle = ctypes.c_void_p()
    rc = lib.molt_ws_connect(buf, len(buf), ctypes.byref(handle))
    if rc != 0 or not handle.value:
        return None
    return handle


def _chan_ptr(chan: Any) -> ctypes.c_void_p | None:
    if isinstance(chan, ctypes.c_void_p):
        return chan
    if isinstance(chan, int):
        return ctypes.c_void_p(chan)
    return None


def _ensure_cancel_root() -> None:
    global _CANCEL_CURRENT
    if 1 not in _CANCEL_TOKENS:
        _CANCEL_TOKENS[1] = {"parent": 0, "cancelled": False, "refs": 1}
    if _CANCEL_CURRENT <= 0:
        _CANCEL_CURRENT = 1


def _retain_token(token_id: int) -> None:
    if token_id <= 1:
        return
    entry = _CANCEL_TOKENS.get(token_id)
    if entry is not None:
        entry["refs"] = int(entry["refs"]) + 1


def _release_token(token_id: int) -> None:
    if token_id <= 1:
        return
    entry = _CANCEL_TOKENS.get(token_id)
    if entry is None:
        return
    entry["refs"] = int(entry["refs"]) - 1
    if int(entry["refs"]) <= 0:
        if token_id in _CANCEL_TOKENS:
            _CANCEL_TOKENS.pop(token_id)


def _token_is_cancelled(token_id: int) -> bool:
    _ensure_cancel_root()
    current = token_id
    depth = 0
    while current != 0 and depth < 64:
        entry = _CANCEL_TOKENS.get(current)
        if entry is None:
            return False
        if bool(entry["cancelled"]):
            return True
        current = int(entry["parent"])
        depth += 1
    return False


def molt_cancel_token_new(parent: int | None = None) -> int:
    _ensure_cancel_root()
    if parent is None:
        parent_id = _CANCEL_CURRENT
    else:
        parent_id = parent
    if parent_id < 0:
        raise ValueError("cancel token parent must be non-negative")
    global _CANCEL_NEXT_ID
    token_id = _CANCEL_NEXT_ID
    _CANCEL_NEXT_ID += 1
    _CANCEL_TOKENS[token_id] = {
        "parent": parent_id,
        "cancelled": False,
        "refs": 1,
    }
    return token_id


def molt_cancel_token_clone(token_id: int) -> None:
    _retain_token(token_id)


def molt_cancel_token_drop(token_id: int) -> None:
    _release_token(token_id)


def molt_cancel_token_cancel(token_id: int) -> None:
    entry = _CANCEL_TOKENS.get(token_id)
    if entry is not None:
        entry["cancelled"] = True


def molt_cancel_token_is_cancelled(token_id: int) -> bool:
    return _token_is_cancelled(token_id)


def molt_cancel_token_set_current(token_id: int | None) -> int:
    _ensure_cancel_root()
    global _CANCEL_CURRENT
    new_id = 1 if token_id in (None, 0) else token_id
    prev = _CANCEL_CURRENT
    _retain_token(new_id)
    _release_token(prev)
    _CANCEL_CURRENT = new_id
    return prev


def molt_cancel_token_get_current() -> int:
    _ensure_cancel_root()
    return _CANCEL_CURRENT


def molt_cancelled() -> bool:
    return _token_is_cancelled(_CANCEL_CURRENT)


def molt_cancel_current() -> None:
    entry = _CANCEL_TOKENS.get(_CANCEL_CURRENT)
    if entry is not None:
        entry["cancelled"] = True


def molt_future_cancel(future: Any) -> int:
    if _use_runtime_concurrency():
        lib = load_runtime()
        if lib is not None:
            task_ptr = _chan_ptr(future)
            if task_ptr is not None:
                lib.molt_future_cancel(task_ptr)
                return 0
    if hasattr(future, "cancel"):
        try:
            future.cancel()
        except Exception:
            return 0
    return 0


def molt_future_cancel_msg(future: Any, msg: Any) -> int:
    if _use_runtime_concurrency():
        lib = load_runtime()
        if lib is not None:
            task_ptr = _chan_ptr(future)
            if task_ptr is not None:
                try:
                    lib.molt_future_cancel_msg(task_ptr, 0)
                except Exception:
                    lib.molt_future_cancel(task_ptr)
                return 0
    if hasattr(future, "cancel"):
        try:
            future.cancel(msg)
        except TypeError:
            try:
                future.cancel()
            except Exception:
                return 0
        except Exception:
            return 0
    return 0


def molt_future_cancel_clear(future: Any) -> int:
    if _use_runtime_concurrency():
        lib = load_runtime()
        if lib is not None:
            task_ptr = _chan_ptr(future)
            if task_ptr is not None:
                try:
                    lib.molt_future_cancel_clear(task_ptr)
                except Exception:
                    return 0
                return 0
    if hasattr(future, "_cancel_message"):
        try:
            setattr(future, "_cancel_message", None)
        except Exception:
            return 0
    return 0


def molt_promise_new() -> Any:
    return None


def molt_promise_set_result(_future: Any, _result: Any) -> int:
    return 0


def molt_promise_set_exception(_future: Any, _exc: Any) -> int:
    return 0


def molt_task_register_token_owned(task: Any, token_id: int) -> int:
    if _use_runtime_concurrency():
        lib = load_runtime()
        if lib is not None:
            task_ptr = _chan_ptr(task)
            if task_ptr is not None:
                lib.molt_task_register_token_owned(task_ptr, token_id)
                return 0
    return 0


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


def molt_thread_submit(callable: Any, args: Any, kwargs: Any) -> Any:
    if args is None:
        call_args = ()
    else:
        call_args = tuple(args)
    if kwargs is None:
        call_kwargs: dict[str, Any] = {}
    else:
        call_kwargs = dict(kwargs)

    async def _runner() -> Any:
        return await asyncio.to_thread(callable, *call_args, **call_kwargs)

    return _runner()


def molt_block_on(task: Any) -> Any:
    loop = _ensure_loop()
    if asyncio.iscoroutine(task) or isinstance(task, asyncio.Future):
        fut = asyncio.run_coroutine_threadsafe(task, loop)
        return fut.result()
    if callable(task):
        return molt_block_on(task())
    raise TypeError("molt_block_on expects a coroutine or callable")


def molt_async_sleep(_delay: float = 0.0, _result: Any | None = None) -> Any:
    async def _sleep() -> Any:
        await asyncio.sleep(_delay)
        return _result

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
        try:
            chan.put_nowait(val)
            return 0
        except queue.Full:
            return _PENDING
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
        try:
            return chan.get_nowait()
        except queue.Empty:
            return _PENDING
    raise TypeError("molt_chan_recv expected a channel handle")


def molt_chan_drop(chan: Any) -> None:
    if _use_runtime_concurrency():
        lib = load_runtime()
        if lib is not None:
            chan_ptr = _chan_ptr(chan)
            if chan_ptr is not None:
                lib.molt_chan_drop(chan_ptr)
                return None
    return None


def molt_socket_new(
    family: int | None, type: int | None, proto: int | None, fileno: int | None
) -> int:
    fam = _socket.AF_INET if family is None else int(family)
    sock_type = _socket.SOCK_STREAM if type is None else int(type)
    proto_val = 0 if proto is None else int(proto)
    if fileno is None:
        sock = _socket.socket(fam, sock_type, proto_val)
    else:
        sock = _socket.socket(fam, sock_type, proto_val, fileno=fileno)
    return _register_socket(sock)


def molt_socket_close(sock: Any) -> None:
    handle = _get_socket_handle(sock)
    if handle is None:
        return None
    handle.close()
    return None


def molt_socket_drop(sock: Any) -> None:
    if not isinstance(sock, int):
        return None
    handle = _SOCKET_HANDLES.get(sock)
    if handle is None:
        return None
    if not handle.closed:
        handle.close()
    handle.refs -= 1
    if handle.refs <= 0:
        _SOCKET_HANDLES.pop(sock, None)
    return None


def molt_socket_clone(sock: Any) -> int:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    handle.refs += 1
    return int(sock)


def molt_socket_fileno(sock: Any) -> int:
    handle = _get_socket_handle(sock)
    if handle is None:
        return -1
    return handle.sock.fileno()


def molt_socket_gettimeout(sock: Any) -> float | None:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    return handle.sock.gettimeout()


def molt_socket_settimeout(sock: Any, timeout: float | None) -> None:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    handle.sock.settimeout(timeout)
    return None


def molt_socket_setblocking(sock: Any, flag: bool) -> None:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    handle.sock.setblocking(bool(flag))
    return None


def molt_socket_getblocking(sock: Any) -> bool:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    if hasattr(handle.sock, "getblocking"):
        return bool(handle.sock.getblocking())
    return handle.sock.gettimeout() != 0.0


def molt_socket_bind(sock: Any, addr: Any) -> None:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    handle.sock.bind(addr)
    return None


def molt_socket_listen(sock: Any, backlog: int) -> None:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    handle.sock.listen(int(backlog))
    return None


def molt_socket_accept(sock: Any) -> tuple[int, Any]:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    conn, addr = handle.sock.accept()
    return _register_socket(conn), addr


def molt_socket_connect(sock: Any, addr: Any) -> None:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    handle.sock.connect(addr)
    return None


def molt_socket_connect_ex(sock: Any, addr: Any) -> int:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    return int(handle.sock.connect_ex(addr))


def molt_socket_recv(sock: Any, size: int, flags: int) -> bytes:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    return handle.sock.recv(int(size), int(flags))


def molt_socket_recv_into(sock: Any, buffer: Any, size: int, flags: int) -> int:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    return int(handle.sock.recv_into(buffer, int(size), int(flags)))


def molt_socket_send(sock: Any, data: Any, flags: int) -> int:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    return int(handle.sock.send(data, int(flags)))


def molt_socket_sendall(sock: Any, data: Any, flags: int) -> None:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    handle.sock.sendall(data, int(flags))
    return None


def molt_socket_sendto(sock: Any, data: Any, flags: int, addr: Any) -> int:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    return int(handle.sock.sendto(data, int(flags), addr))


def molt_socket_recvfrom(sock: Any, size: int, flags: int) -> tuple[bytes, Any]:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    return handle.sock.recvfrom(int(size), int(flags))


def molt_socket_shutdown(sock: Any, how: int) -> None:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    handle.sock.shutdown(int(how))
    return None


def molt_socket_getsockname(sock: Any) -> Any:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    return handle.sock.getsockname()


def molt_socket_getpeername(sock: Any) -> Any:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    return handle.sock.getpeername()


def molt_socket_setsockopt(sock: Any, level: int, optname: int, value: Any) -> None:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    handle.sock.setsockopt(int(level), int(optname), value)
    return None


def molt_socket_getsockopt(sock: Any, level: int, optname: int, buflen: int) -> Any:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    if buflen:
        return handle.sock.getsockopt(int(level), int(optname), int(buflen))
    return handle.sock.getsockopt(int(level), int(optname))


def molt_socket_detach(sock: Any) -> int:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    handle.closed = True
    return handle.sock.detach()


def molt_socketpair(
    family: int | None, type: int | None, proto: int | None
) -> tuple[int, int]:
    fam = (
        _socket.AF_UNIX
        if family is None and hasattr(_socket, "AF_UNIX")
        else (_socket.AF_INET if family is None else int(family))
    )
    sock_type = _socket.SOCK_STREAM if type is None else int(type)
    proto_val = 0 if proto is None else int(proto)
    left, right = _socket.socketpair(fam, sock_type, proto_val)
    return _register_socket(left), _register_socket(right)


def molt_socket_getaddrinfo(
    host: str | bytes | None,
    port: int | str | bytes | None,
    family: int,
    type: int,
    proto: int,
    flags: int,
) -> list[tuple[int, int, int, str | None, Any]]:
    raw = _socket.getaddrinfo(
        host, port, int(family), int(type), int(proto), int(flags)
    )
    return [
        (int(fam), int(socktype), int(proto_val), canon, sockaddr)
        for fam, socktype, proto_val, canon, sockaddr in raw
    ]


def molt_socket_getnameinfo(addr: Any, flags: int) -> tuple[str, str]:
    return _socket.getnameinfo(addr, int(flags))


def molt_socket_gethostname() -> str:
    return _socket.gethostname()


def molt_socket_getservbyname(name: str, proto: str | None) -> int:
    if proto is None:
        return int(_socket.getservbyname(name))
    return int(_socket.getservbyname(name, proto))


def molt_socket_getservbyport(port: int, proto: str | None) -> str:
    if proto is None:
        return _socket.getservbyport(int(port))
    return _socket.getservbyport(int(port), proto)


def molt_socket_inet_pton(family: int, address: str) -> bytes:
    return _socket.inet_pton(int(family), address)


def molt_socket_inet_ntop(family: int, packed: bytes) -> str:
    return _socket.inet_ntop(int(family), packed)


def molt_socket_constants() -> dict[str, int]:
    names = [
        "AF_INET",
        "AF_INET6",
        "AF_UNIX",
        "SOCK_STREAM",
        "SOCK_DGRAM",
        "SOCK_RAW",
        "SOL_SOCKET",
        "SO_REUSEADDR",
        "SO_KEEPALIVE",
        "SO_SNDBUF",
        "SO_RCVBUF",
        "SO_ERROR",
        "SO_LINGER",
        "SO_BROADCAST",
        "SO_REUSEPORT",
        "IPPROTO_TCP",
        "IPPROTO_UDP",
        "IPPROTO_IPV6",
        "IPV6_V6ONLY",
        "TCP_NODELAY",
        "SHUT_RD",
        "SHUT_WR",
        "SHUT_RDWR",
        "AI_PASSIVE",
        "AI_CANONNAME",
        "AI_NUMERICHOST",
        "AI_NUMERICSERV",
        "NI_NUMERICHOST",
        "NI_NUMERICSERV",
        "MSG_PEEK",
        "MSG_DONTWAIT",
        "EAI_AGAIN",
        "EAI_FAIL",
        "EAI_FAMILY",
        "EAI_NONAME",
        "EAI_SERVICE",
        "EAI_SOCKTYPE",
    ]
    out: dict[str, int] = {}
    for name in names:
        if hasattr(_socket, name):
            out[name] = int(getattr(_socket, name))
    return out


def molt_socket_has_ipv6() -> bool:
    return bool(getattr(_socket, "has_ipv6", False))


def molt_io_wait_new(sock: Any, events: int, timeout: float | None) -> Any:
    handle = _get_socket_handle(sock)
    if handle is None:
        raise TypeError("invalid socket handle")
    if events == 0:
        raise ValueError("events must be non-zero")
    loop = asyncio.get_running_loop()
    fut: asyncio.Future[int] = loop.create_future()
    fd = handle.sock.fileno()

    def cleanup() -> None:
        try:
            if events & 1:
                loop.remove_reader(fd)
            if events & 2:
                loop.remove_writer(fd)
        except Exception:
            pass

    def compute_mask() -> int:
        # TODO(perf, owner:tooling, milestone:TL2, priority:P2, status:planned): avoid per-callback select calls by caching readiness or using a shared selector.
        mask = 0
        try:
            if events & 1:
                rlist, _, _ = _select.select([handle.sock], [], [], 0)
                if rlist:
                    mask |= 1
            if events & 2:
                _, wlist, _ = _select.select([], [handle.sock], [], 0)
                if wlist:
                    mask |= 2
        except Exception:
            mask |= 4
        return mask if mask else events

    def on_ready() -> None:
        if fut.done():
            return
        fut.set_result(compute_mask())
        cleanup()

    if timeout is not None and timeout <= 0:
        fut.set_exception(TimeoutError())
        return fut

    if events & 1:
        loop.add_reader(fd, on_ready)
    if events & 2:
        loop.add_writer(fd, on_ready)

    if timeout is not None:

        def on_timeout() -> None:
            if fut.done():
                return
            fut.set_exception(TimeoutError())
            cleanup()

        loop.call_later(float(timeout), on_timeout)

    fut.add_done_callback(lambda _fut: cleanup())
    return fut


def _molt_class_new(name: str) -> type:
    if not isinstance(name, str):
        raise TypeError("class name must be str")
    return type(name, (), {})


def _molt_class_set_base(cls: type, base: object) -> type | None:
    if not isinstance(cls, type):
        raise TypeError("class must be a type object")
    if base is None:
        bases = ()
    elif isinstance(base, tuple):
        bases = base
    else:
        bases = (base,)
    for entry in bases:
        if not isinstance(entry, type):
            raise TypeError("base must be a type object or tuple of types")
    if not bases:
        bases = (object,)
    bases = cast(tuple[type, ...], bases)
    if cls.__bases__ == bases:
        return None
    try:
        cls.__bases__ = bases
        return None
    except TypeError:
        namespace = {
            key: value
            for key, value in cls.__dict__.items()
            if key not in {"__dict__", "__weakref__"}
        }
        return type(cls.__name__, bases, namespace)


def _molt_class_apply_set_name(cls: type) -> None:
    if not isinstance(cls, type):
        raise TypeError("class must be a type object")
    for name, value in cls.__dict__.items():
        set_name = getattr(value, "__set_name__", None)
        if set_name is None:
            continue
        set_name(cls, name)
    return None


def _molt_module_new(name: str) -> _types.ModuleType:
    if not isinstance(name, str):
        raise TypeError("module name must be str")
    return _types.ModuleType(name)


def install() -> None:
    import builtins

    global _ORIG_ASYNCIO_SLEEP
    if _ORIG_ASYNCIO_SLEEP is None:
        _ORIG_ASYNCIO_SLEEP = asyncio.sleep

        async def _molt_sleep(delay: float = 0.0, result: Any | None = None) -> Any:
            return await _ORIG_ASYNCIO_SLEEP(delay, result)

        asyncio.sleep = cast(Any, _molt_sleep)

    setattr(builtins, "molt_spawn", molt_spawn)
    setattr(builtins, "molt_chan_new", molt_chan_new)
    setattr(builtins, "molt_chan_send", molt_chan_send)
    setattr(builtins, "molt_chan_recv", molt_chan_recv)
    setattr(builtins, "molt_chan_drop", molt_chan_drop)
    setattr(builtins, "molt_block_on", molt_block_on)
    setattr(builtins, "molt_async_sleep", molt_async_sleep)
    setattr(builtins, "molt_cancel_token_new", molt_cancel_token_new)
    setattr(builtins, "molt_cancel_token_clone", molt_cancel_token_clone)
    setattr(builtins, "molt_cancel_token_drop", molt_cancel_token_drop)
    setattr(builtins, "molt_cancel_token_cancel", molt_cancel_token_cancel)
    setattr(builtins, "molt_cancel_token_is_cancelled", molt_cancel_token_is_cancelled)
    setattr(builtins, "molt_cancel_token_set_current", molt_cancel_token_set_current)
    setattr(builtins, "molt_cancel_token_get_current", molt_cancel_token_get_current)
    setattr(builtins, "molt_cancelled", molt_cancelled)
    setattr(builtins, "molt_cancel_current", molt_cancel_current)
    setattr(builtins, "molt_future_cancel", molt_future_cancel)
    setattr(builtins, "molt_future_cancel_msg", molt_future_cancel_msg)
    setattr(builtins, "molt_future_cancel_clear", molt_future_cancel_clear)
    setattr(builtins, "molt_promise_new", molt_promise_new)
    setattr(builtins, "molt_promise_set_result", molt_promise_set_result)
    setattr(builtins, "molt_promise_set_exception", molt_promise_set_exception)
    setattr(builtins, "molt_task_register_token_owned", molt_task_register_token_owned)
    setattr(builtins, "molt_thread_submit", molt_thread_submit)
    setattr(builtins, "_molt_class_new", _molt_class_new)
    setattr(builtins, "_molt_class_set_base", _molt_class_set_base)
    setattr(builtins, "_molt_class_apply_set_name", _molt_class_apply_set_name)
    setattr(builtins, "_molt_module_new", _molt_module_new)
    setattr(builtins, "_molt_io_wait_new", molt_io_wait_new)
    setattr(builtins, "_molt_socket_new", molt_socket_new)
    setattr(builtins, "_molt_socket_close", molt_socket_close)
    setattr(builtins, "_molt_socket_drop", molt_socket_drop)
    setattr(builtins, "_molt_socket_clone", molt_socket_clone)
    setattr(builtins, "_molt_socket_fileno", molt_socket_fileno)
    setattr(builtins, "_molt_socket_gettimeout", molt_socket_gettimeout)
    setattr(builtins, "_molt_socket_settimeout", molt_socket_settimeout)
    setattr(builtins, "_molt_socket_setblocking", molt_socket_setblocking)
    setattr(builtins, "_molt_socket_getblocking", molt_socket_getblocking)
    setattr(builtins, "_molt_socket_bind", molt_socket_bind)
    setattr(builtins, "_molt_socket_listen", molt_socket_listen)
    setattr(builtins, "_molt_socket_accept", molt_socket_accept)
    setattr(builtins, "_molt_socket_connect", molt_socket_connect)
    setattr(builtins, "_molt_socket_connect_ex", molt_socket_connect_ex)
    setattr(builtins, "_molt_socket_recv", molt_socket_recv)
    setattr(builtins, "_molt_socket_recv_into", molt_socket_recv_into)
    setattr(builtins, "_molt_socket_send", molt_socket_send)
    setattr(builtins, "_molt_socket_sendall", molt_socket_sendall)
    setattr(builtins, "_molt_socket_sendto", molt_socket_sendto)
    setattr(builtins, "_molt_socket_recvfrom", molt_socket_recvfrom)
    setattr(builtins, "_molt_socket_shutdown", molt_socket_shutdown)
    setattr(builtins, "_molt_socket_getsockname", molt_socket_getsockname)
    setattr(builtins, "_molt_socket_getpeername", molt_socket_getpeername)
    setattr(builtins, "_molt_socket_setsockopt", molt_socket_setsockopt)
    setattr(builtins, "_molt_socket_getsockopt", molt_socket_getsockopt)
    setattr(builtins, "_molt_socket_detach", molt_socket_detach)
    setattr(builtins, "_molt_socketpair", molt_socketpair)
    setattr(builtins, "_molt_socket_getaddrinfo", molt_socket_getaddrinfo)
    setattr(builtins, "_molt_socket_getnameinfo", molt_socket_getnameinfo)
    setattr(builtins, "_molt_socket_gethostname", molt_socket_gethostname)
    setattr(builtins, "_molt_socket_getservbyname", molt_socket_getservbyname)
    setattr(builtins, "_molt_socket_getservbyport", molt_socket_getservbyport)
    setattr(builtins, "_molt_socket_inet_pton", molt_socket_inet_pton)
    setattr(builtins, "_molt_socket_inet_ntop", molt_socket_inet_ntop)
    setattr(builtins, "_molt_socket_constants", molt_socket_constants)
    setattr(builtins, "_molt_socket_has_ipv6", molt_socket_has_ipv6)
    setattr(builtins, "molt_stream", net_mod.stream)
    setattr(builtins, "molt_stream_channel", net_mod.stream_channel)
    setattr(builtins, "molt_ws_pair", net_mod.ws_pair)
    setattr(builtins, "molt_ws_connect", net_mod.ws_connect)
