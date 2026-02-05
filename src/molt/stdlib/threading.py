"""Capability-gated threading for Molt."""

from __future__ import annotations

from typing import Any, Callable, cast
import os
import struct
import sys
import time

from _intrinsics import require_intrinsic as _require_intrinsic


_MOLT_THREAD_SPAWN = _require_intrinsic("molt_thread_spawn", globals())
_MOLT_THREAD_JOIN = _require_intrinsic("molt_thread_join", globals())
_MOLT_THREAD_IS_ALIVE = _require_intrinsic("molt_thread_is_alive", globals())
_MOLT_THREAD_IDENT = _require_intrinsic("molt_thread_ident", globals())
_MOLT_THREAD_NATIVE_ID = _require_intrinsic("molt_thread_native_id", globals())
_MOLT_THREAD_CURRENT_IDENT = _require_intrinsic("molt_thread_current_ident", globals())
_MOLT_THREAD_CURRENT_NATIVE_ID = _require_intrinsic(
    "molt_thread_current_native_id", globals()
)
_MOLT_THREAD_DROP = _require_intrinsic("molt_thread_drop", globals())
_MOLT_LOCK_NEW = _require_intrinsic("molt_lock_new", globals())
_MOLT_LOCK_ACQUIRE = _require_intrinsic("molt_lock_acquire", globals())
_MOLT_LOCK_RELEASE = _require_intrinsic("molt_lock_release", globals())
_MOLT_LOCK_LOCKED = _require_intrinsic("molt_lock_locked", globals())
_MOLT_LOCK_DROP = _require_intrinsic("molt_lock_drop", globals())
_MOLT_RLOCK_NEW = _require_intrinsic("molt_rlock_new", globals())
_MOLT_RLOCK_ACQUIRE = _require_intrinsic("molt_rlock_acquire", globals())
_MOLT_RLOCK_RELEASE = _require_intrinsic("molt_rlock_release", globals())
_MOLT_RLOCK_LOCKED = _require_intrinsic("molt_rlock_locked", globals())
_MOLT_RLOCK_DROP = _require_intrinsic("molt_rlock_drop", globals())
_MOLT_MODULE_CACHE_SET = _require_intrinsic("molt_module_cache_set", globals())


def _require_callable(value: object, name: str) -> Callable[..., object]:
    if not callable(value):
        raise RuntimeError(f"{name} intrinsic unavailable")
    return value


def _expect_int(value: object) -> int:
    return int(cast(int, value))


_HAVE_INTRINSICS = all(
    callable(fn)
    for fn in (
        _MOLT_THREAD_SPAWN,
        _MOLT_THREAD_JOIN,
        _MOLT_THREAD_IS_ALIVE,
        _MOLT_THREAD_IDENT,
        _MOLT_THREAD_NATIVE_ID,
        _MOLT_THREAD_CURRENT_IDENT,
        _MOLT_THREAD_CURRENT_NATIVE_ID,
        _MOLT_THREAD_DROP,
        _MOLT_LOCK_NEW,
        _MOLT_LOCK_ACQUIRE,
        _MOLT_LOCK_RELEASE,
        _MOLT_LOCK_LOCKED,
        _MOLT_LOCK_DROP,
        _MOLT_RLOCK_NEW,
        _MOLT_RLOCK_ACQUIRE,
        _MOLT_RLOCK_RELEASE,
        _MOLT_RLOCK_LOCKED,
        _MOLT_RLOCK_DROP,
    )
)


def _register_module_cache() -> None:
    module = sys.modules.get(__name__)
    if module is None:
        return
    _MOLT_MODULE_CACHE_SET(__name__, module)
    if __name__ != "threading":
        _MOLT_MODULE_CACHE_SET("threading", module)


if not _HAVE_INTRINSICS:
    raise RuntimeError("threading intrinsics unavailable")
else:
    _register_module_cache()

    __all__ = [
        "Thread",
        "Lock",
        "RLock",
        "Condition",
        "Event",
        "Semaphore",
        "BoundedSemaphore",
        "Barrier",
        "BrokenBarrierError",
        "Timer",
        "local",
        "current_thread",
        "main_thread",
        "get_ident",
        "get_native_id",
        "active_count",
        "enumerate",
        "TIMEOUT_MAX",
        "ExceptHookArgs",
        "excepthook",
        "settrace",
        "setprofile",
        "stack_size",
    ]

    _thread_spawn = _require_callable(_MOLT_THREAD_SPAWN, "molt_thread_spawn")
    _thread_join = _require_callable(_MOLT_THREAD_JOIN, "molt_thread_join")
    _thread_is_alive = _require_callable(_MOLT_THREAD_IS_ALIVE, "molt_thread_is_alive")
    _thread_ident = _require_callable(_MOLT_THREAD_IDENT, "molt_thread_ident")
    _thread_native_id = _require_callable(
        _MOLT_THREAD_NATIVE_ID, "molt_thread_native_id"
    )
    _thread_current_ident = _require_callable(
        _MOLT_THREAD_CURRENT_IDENT, "molt_thread_current_ident"
    )
    _thread_current_native_id = _require_callable(
        _MOLT_THREAD_CURRENT_NATIVE_ID, "molt_thread_current_native_id"
    )
    _thread_drop = _require_callable(_MOLT_THREAD_DROP, "molt_thread_drop")
    _lock_new = _require_callable(_MOLT_LOCK_NEW, "molt_lock_new")
    _lock_acquire = _require_callable(_MOLT_LOCK_ACQUIRE, "molt_lock_acquire")
    _lock_release = _require_callable(_MOLT_LOCK_RELEASE, "molt_lock_release")
    _lock_locked = _require_callable(_MOLT_LOCK_LOCKED, "molt_lock_locked")
    _lock_drop = _require_callable(_MOLT_LOCK_DROP, "molt_lock_drop")
    _rlock_new = _require_callable(_MOLT_RLOCK_NEW, "molt_rlock_new")
    _rlock_acquire = _require_callable(_MOLT_RLOCK_ACQUIRE, "molt_rlock_acquire")
    _rlock_release = _require_callable(_MOLT_RLOCK_RELEASE, "molt_rlock_release")
    _rlock_locked = _require_callable(_MOLT_RLOCK_LOCKED, "molt_rlock_locked")
    _rlock_drop = _require_callable(_MOLT_RLOCK_DROP, "molt_rlock_drop")

    # TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): threading parity with shared-memory semantics + full primitives.

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

    _MAX_DEPTH = 64

    _THREAD_COUNTER = 0
    _THREAD_TOKEN_COUNTER = 0
    _TIMER_TOKEN_COUNTER = 0
    _THREADS: list["Thread"] = []
    _MAIN_THREAD: "Thread" | None = None
    _THREAD_BY_IDENT: dict[int, "Thread"] = {}
    _THREAD_BY_TOKEN: dict[int, "Thread"] = {}
    _TIMER_BY_TOKEN: dict[int, "Timer"] = {}
    _THREAD_SHARED_PAYLOADS: dict[
        int, tuple[Callable[..., Any], tuple[Any, ...], dict[str, Any]]
    ] = {}

    def _shared_runtime_enabled() -> bool:
        trusted = os.getenv("MOLT_TRUSTED", "").strip().lower()
        if trusted in {"1", "true", "yes", "on"}:
            return True
        caps = os.getenv("MOLT_CAPABILITIES", "")
        return "thread.shared" in {
            cap.strip() for cap in caps.split(",") if cap.strip()
        }

    _THREAD_SHARED_RUNTIME = _shared_runtime_enabled()

    _TRACE_HOOK: Callable[[Any, str, Any], Any] | None = None
    _PROFILE_HOOK: Callable[[Any, str, Any], Any] | None = None
    _STACK_SIZE = 0
    _NO_CONTEXT = object()

    class ExceptHookArgs:
        def __init__(
            self,
            exc_type: type[BaseException],
            exc_value: BaseException,
            exc_traceback: Any,
            thread: "Thread",
        ) -> None:
            self.exc_type = exc_type
            self.exc_value = exc_value
            self.exc_traceback = exc_traceback
            self.thread = thread

    def _default_excepthook(args: ExceptHookArgs) -> None:
        import traceback

        print(f"Exception in thread {args.thread.name}:", file=sys.stderr)
        traceback.print_exception(args.exc_type, args.exc_value, args.exc_traceback)

    excepthook: Callable[[ExceptHookArgs], Any] | None = _default_excepthook

    def settrace(func: Callable[[Any, str, Any], Any] | None) -> None:
        global _TRACE_HOOK
        _TRACE_HOOK = func

    def setprofile(func: Callable[[Any, str, Any], Any] | None) -> None:
        global _PROFILE_HOOK
        _PROFILE_HOOK = func

    def stack_size(size: int | None = None) -> int:
        global _STACK_SIZE
        if size is None:
            return _STACK_SIZE
        try:
            new_size = int(size)
        except (TypeError, ValueError) as exc:
            raise TypeError("size must be 0 or a positive integer") from exc
        if new_size < 0:
            raise ValueError("size must be 0 or a positive integer")
        prev = _STACK_SIZE
        _STACK_SIZE = new_size
        return prev

    def _next_thread_name() -> str:
        global _THREAD_COUNTER
        _THREAD_COUNTER += 1
        return f"Thread-{_THREAD_COUNTER}"

    def _next_thread_token() -> int:
        global _THREAD_TOKEN_COUNTER
        _THREAD_TOKEN_COUNTER += 1
        return _THREAD_TOKEN_COUNTER

    def _next_timer_token() -> int:
        global _TIMER_TOKEN_COUNTER
        _TIMER_TOKEN_COUNTER += 1
        return _TIMER_TOKEN_COUNTER

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

    def _decode_bytes(data: bytes, idx: int) -> tuple[bytes, int]:
        length, idx = _decode_varint(data, idx)
        end = idx + length
        if end > len(data):
            raise ValueError("truncated bytes")
        return data[idx:end], end

    def _encode_value(value: Any, out: bytearray, depth: int) -> None:
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
        if isinstance(value, int):
            out.append(_TAG_INT)
            _encode_int(value, out)
            return
        if isinstance(value, float):
            out.append(_TAG_FLOAT)
            out.extend(struct.pack("<d", value))
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
            _encode_bytes(value.encode("utf-8"), out)
            return
        if isinstance(value, list):
            out.append(_TAG_LIST)
            _encode_varint(len(value), out)
            for item in value:
                _encode_value(item, out, depth + 1)
            return
        if isinstance(value, tuple):
            out.append(_TAG_TUPLE)
            _encode_varint(len(value), out)
            for item in value:
                _encode_value(item, out, depth + 1)
            return
        if isinstance(value, dict):
            out.append(_TAG_DICT)
            _encode_varint(len(value), out)
            for key, item in value.items():
                _encode_value(key, out, depth + 1)
                _encode_value(item, out, depth + 1)
            return
        if isinstance(value, set):
            out.append(_TAG_SET)
            items = sorted(value, key=repr)
            _encode_varint(len(items), out)
            for item in items:
                _encode_value(item, out, depth + 1)
            return
        if isinstance(value, frozenset):
            out.append(_TAG_FROZENSET)
            items = sorted(value, key=repr)
            _encode_varint(len(items), out)
            for item in items:
                _encode_value(item, out, depth + 1)
            return
        raise TypeError(f"cannot serialize {type(value).__name__}")

    def _decode_value(data: bytes, idx: int, depth: int) -> tuple[Any, int]:
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
            return struct.unpack("<d", data[idx:end])[0], end
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
                item, idx = _decode_value(data, idx, depth + 1)
                items.append(item)
            return items, idx
        if tag == _TAG_TUPLE:
            length, idx = _decode_varint(data, idx)
            items: list[Any] = []
            for _ in range(length):
                item, idx = _decode_value(data, idx, depth + 1)
                items.append(item)
            return tuple(items), idx
        if tag == _TAG_DICT:
            length, idx = _decode_varint(data, idx)
            out: dict[Any, Any] = {}
            for _ in range(length):
                key, idx = _decode_value(data, idx, depth + 1)
                value, idx = _decode_value(data, idx, depth + 1)
                out[key] = value
            return out, idx
        if tag == _TAG_SET:
            length, idx = _decode_varint(data, idx)
            items: list[Any] = []
            for _ in range(length):
                item, idx = _decode_value(data, idx, depth + 1)
                items.append(item)
            return set(items), idx
        if tag == _TAG_FROZENSET:
            length, idx = _decode_varint(data, idx)
            items: list[Any] = []
            for _ in range(length):
                item, idx = _decode_value(data, idx, depth + 1)
                items.append(item)
            return frozenset(items), idx
        raise ValueError(f"unsupported tag {tag}")

    def _encode_payload(payload: Any) -> bytes:
        out = bytearray()
        _encode_value(payload, out, 0)
        return bytes(out)

    def _decode_payload(data: bytes) -> Any:
        value, idx = _decode_value(data, 0, 0)
        if idx != len(data):
            raise ValueError("trailing payload data")
        return value

    def _resolve_entry_module_name() -> str:
        main_mod = sys.modules.get("__main__")
        if main_mod is None:
            return "__main__"
        for name, mod in sys.modules.items():
            if name != "__main__" and mod is main_mod:
                return name
        spec = getattr(main_mod, "__spec__", None)
        spec_name = getattr(spec, "name", None) if spec is not None else None
        if isinstance(spec_name, str) and spec_name:
            return spec_name
        main_path = getattr(main_mod, "__file__", None)
        if isinstance(main_path, str) and main_path:
            resolved = _module_name_from_path(main_path)
            if resolved is not None:
                return resolved
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
            return ".".join(parts)
        return None

    def _resolve_target(
        target: Callable[..., Any] | None,
    ) -> tuple[str | None, str | None]:
        if target is None:
            return None, None
        if not callable(target):
            raise TypeError("thread target must be callable")
        if _THREAD_SHARED_RUNTIME:
            return None, None
        module = getattr(target, "__module__", None)
        qualname = getattr(target, "__qualname__", None)
        if module == "__main__":
            if not _THREAD_SHARED_RUNTIME:
                resolved = _resolve_entry_module_name()
                if resolved != "__main__":
                    module = resolved
        elif _THREAD_SHARED_RUNTIME:
            main_mod = sys.modules.get("__main__")
            main_file = (
                getattr(main_mod, "__file__", None) if main_mod is not None else None
            )
            code = getattr(target, "__code__", None)
            target_file = (
                getattr(code, "co_filename", None) if code is not None else None
            )
            if isinstance(main_file, str) and isinstance(target_file, str):
                try:
                    if os.path.samefile(main_file, target_file):
                        module = "__main__"
                except Exception:
                    if main_file == target_file:
                        module = "__main__"
            if module != "__main__" and main_mod is not None and module is not None:
                sys.modules.setdefault(str(module), main_mod)
        if not isinstance(module, str) or not isinstance(qualname, str):
            raise TypeError("thread target must be a module-level callable")
        if "<locals>" in qualname:
            raise TypeError("thread target must be a module-level callable")
        return module, qualname

    def _resolve_qualname(module: str, qualname: str) -> Callable[..., Any]:
        if module == "__main__" and _THREAD_SHARED_RUNTIME:
            mod = sys.modules.get("__main__")
            if mod is None:
                raise ModuleNotFoundError("module '__main__' not found")
        else:
            mod = __import__(module, fromlist=["*"])
        obj: Any = mod
        for part in qualname.split("."):
            obj = getattr(obj, part)
        if not callable(obj):
            raise TypeError("thread target is not callable")
        return obj

    def get_ident() -> int:
        return _expect_int(_thread_current_ident())

    def get_native_id() -> int:
        return _expect_int(_thread_current_native_id())

    def _check_timeout_max(timeout_val: float) -> None:
        if timeout_val > TIMEOUT_MAX:
            raise OverflowError("timestamp out of range for platform time_t")

    def _invoke_thread_hooks() -> None:
        if _TRACE_HOOK is not None:
            try:
                _TRACE_HOOK(None, "call", None)
            except Exception:
                pass
        if _PROFILE_HOOK is not None:
            try:
                _PROFILE_HOOK(None, "call", None)
            except Exception:
                pass

    def _call_excepthook(thread: "Thread", exc: BaseException) -> None:
        hook = excepthook
        if hook is None:
            return
        args = ExceptHookArgs(type(exc), exc, exc.__traceback__, thread)
        try:
            hook(args)
        except Exception:
            try:
                _default_excepthook(args)
            except Exception:
                pass

    class Lock:
        def __init__(self) -> None:
            self._handle: Any | None = _lock_new()

        def acquire(self, blocking: bool = True, timeout: float = -1.0) -> bool:
            if timeout is None:
                raise TypeError(
                    "'NoneType' object cannot be interpreted as an integer or float"
                )
            try:
                timeout_val = float(timeout)
            except (TypeError, ValueError) as exc:
                raise TypeError(
                    f"'{type(timeout).__name__}' object cannot be interpreted as an integer or float"
                ) from exc
            if not blocking:
                if timeout_val != -1.0:
                    raise ValueError("can't specify a timeout for a non-blocking call")
            elif timeout_val < 0.0 and timeout_val != -1.0:
                raise ValueError("timeout value must be a non-negative number")
            if blocking and timeout_val != -1.0:
                _check_timeout_max(timeout_val)
            if self._handle is None:
                raise RuntimeError("lock is not initialized")
            acquired = bool(_lock_acquire(self._handle, bool(blocking), timeout_val))
            return acquired

        def release(self) -> None:
            if self._handle is None:
                raise RuntimeError("lock is not initialized")
            _lock_release(self._handle)

        def locked(self) -> bool:
            if self._handle is None:
                return False
            return bool(_lock_locked(self._handle))

        def _is_owned(self) -> bool:
            return self.locked()

        def _release_save(self) -> None:
            self.release()

        def _acquire_restore(self, _state: Any) -> None:
            self.acquire()

        def __enter__(self) -> Lock:
            self.acquire()
            return self

        def __exit__(self, _exc_type: Any, _exc: Any, _tb: Any) -> bool:
            self.release()
            return False

        def _drop(self) -> None:
            if self._handle is None:
                return
            _lock_drop(self._handle)
            self._handle = None

        def __del__(self) -> None:
            if getattr(self, "_handle", None) is None:
                return
            self._drop()

    class RLock:
        def __init__(self) -> None:
            self._handle: Any | None = _rlock_new()
            self._owner: int | None = None
            self._count = 0

        def acquire(self, blocking: bool = True, timeout: float = -1.0) -> bool:
            if timeout is None:
                raise TypeError(
                    "'NoneType' object cannot be interpreted as an integer or float"
                )
            try:
                timeout_val = float(timeout)
            except (TypeError, ValueError) as exc:
                raise TypeError(
                    f"'{type(timeout).__name__}' object cannot be interpreted as an integer or float"
                ) from exc
            if not blocking:
                if timeout_val != -1.0:
                    raise ValueError("can't specify a timeout for a non-blocking call")
            elif timeout_val < 0.0 and timeout_val != -1.0:
                raise ValueError("timeout value must be a non-negative number")
            if blocking and timeout_val != -1.0:
                _check_timeout_max(timeout_val)
            if self._handle is None:
                raise RuntimeError("rlock is not initialized")
            acquired = bool(_rlock_acquire(self._handle, bool(blocking), timeout_val))
            if acquired:
                ident = get_ident()
                if self._owner == ident:
                    self._count += 1
                else:
                    self._owner = ident
                    self._count = 1
            return acquired

        def release(self) -> None:
            if self._handle is None:
                raise RuntimeError("rlock is not initialized")
            if self._owner != get_ident():
                raise RuntimeError("cannot release un-acquired lock")
            _rlock_release(self._handle)
            if self._count > 0:
                self._count -= 1
            if self._count == 0:
                self._owner = None

        def _is_owned(self) -> bool:
            return self._owner == get_ident()

        def _release_save(self) -> int:
            if self._owner != get_ident():
                raise RuntimeError("cannot release un-acquired lock")
            count = self._count
            for _ in range(count):
                self.release()
            return count

        def _acquire_restore(self, count: int | None) -> None:
            if not count:
                self.acquire()
                return
            for _ in range(count):
                self.acquire()

        def __enter__(self) -> RLock:
            self.acquire()
            return self

        def __exit__(self, _exc_type: Any, _exc: Any, _tb: Any) -> bool:
            self.release()
            return False

        def _drop(self) -> None:
            if self._handle is None:
                return
            _rlock_drop(self._handle)
            self._handle = None

        def __del__(self) -> None:
            if getattr(self, "_handle", None) is None:
                return
            self._drop()

    class Condition:
        def __init__(self, lock: Any | None = None) -> None:
            if lock is None:
                lock = RLock()
            self._lock = lock
            self._waiters: list[Any] = []

        def acquire(self, *args: Any, **kwargs: Any) -> bool:
            return bool(self._lock.acquire(*args, **kwargs))

        def release(self) -> None:
            self._lock.release()

        def __enter__(self) -> Condition:
            self.acquire()
            return self

        def __exit__(self, _exc_type: Any, _exc: Any, _tb: Any) -> bool:
            self.release()
            return False

        def _is_owned(self) -> bool:
            if hasattr(self._lock, "_is_owned"):
                return bool(self._lock._is_owned())
            acquired = self._lock.acquire(False)
            if acquired:
                self._lock.release()
                return False
            return True

        def _release_save(self) -> Any:
            if hasattr(self._lock, "_release_save"):
                return self._lock._release_save()
            self._lock.release()
            return None

        def _acquire_restore(self, state: Any) -> None:
            if hasattr(self._lock, "_acquire_restore"):
                self._lock._acquire_restore(state)
                return
            self._lock.acquire()

        def wait(self, timeout: float | None = None) -> bool:
            if not self._is_owned():
                raise RuntimeError("cannot wait on un-acquired lock")
            timeout_val: float | None
            if timeout is None:
                timeout_val = None
            else:
                try:
                    timeout_val = float(timeout)
                except (TypeError, ValueError) as exc:
                    raise TypeError(
                        f"'{type(timeout).__name__}' object cannot be interpreted as an integer or float"
                    ) from exc
                if timeout_val < 0.0:
                    raise ValueError("timeout value must be a non-negative number")
                _check_timeout_max(timeout_val)
            from molt.concurrency import channel

            waiter = channel(1)
            self._waiters.append(waiter)
            saved = self._release_save()
            try:
                if timeout_val is None:
                    waiter.recv()
                    return True
                if timeout_val == 0.0:
                    ok, _ = waiter.try_recv()
                    if not ok and waiter in self._waiters:
                        self._waiters.remove(waiter)
                    return ok
                deadline = time.monotonic() + timeout_val
                while True:
                    ok, _ = waiter.try_recv()
                    if ok:
                        return True
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        if waiter in self._waiters:
                            self._waiters.remove(waiter)
                        return False
                    time.sleep(min(remaining, 0.05))
            finally:
                try:
                    waiter.close()
                finally:
                    self._acquire_restore(saved)

        def wait_for(
            self, predicate: Callable[[], bool], timeout: float | None = None
        ) -> bool:
            end = None if timeout is None else time.monotonic() + float(timeout)
            result = predicate()
            while not result:
                if end is None:
                    self.wait(None)
                else:
                    remaining = end - time.monotonic()
                    if remaining <= 0:
                        break
                    self.wait(remaining)
                result = predicate()
            return result

        def notify(self, n: int = 1) -> None:
            if not self._is_owned():
                raise RuntimeError("cannot notify on un-acquired lock")
            if n <= 0:
                return
            for _ in range(min(n, len(self._waiters))):
                waiter = self._waiters.pop(0)
                try:
                    if hasattr(waiter, "try_send"):
                        waiter.try_send(True)
                    else:
                        waiter.send(True)
                except RuntimeError:
                    pass

        def notify_all(self) -> None:
            self.notify(len(self._waiters))

        notifyAll = notify_all

    class Event:
        def __init__(self) -> None:
            self._cond = Condition(Lock())
            self._flag = False

        def is_set(self) -> bool:
            return self._flag

        def set(self) -> None:
            with self._cond:
                self._flag = True
                self._cond.notify_all()

        def clear(self) -> None:
            with self._cond:
                self._flag = False

        def wait(self, timeout: float | None = None) -> bool:
            with self._cond:
                if self._flag:
                    return True
                return self._cond.wait_for(lambda: self._flag, timeout)

    class Semaphore:
        def __init__(self, value: int = 1) -> None:
            if value < 0:
                raise ValueError("semaphore initial value must be >= 0")
            self._value = int(value)
            self._cond = Condition(Lock())

        def acquire(self, blocking: bool = True, timeout: float | None = None) -> bool:
            if timeout is None:
                timeout_val = None
            else:
                try:
                    timeout_val = float(timeout)
                except (TypeError, ValueError) as exc:
                    raise TypeError(
                        f"'{type(timeout).__name__}' object cannot be interpreted as an integer or float"
                    ) from exc
                if timeout_val < 0.0:
                    raise ValueError("timeout value must be a non-negative number")
                _check_timeout_max(timeout_val)
            if not blocking:
                if timeout_val not in (None, 0.0):
                    raise ValueError("can't specify a timeout for a non-blocking call")
                timeout_val = 0.0
            with self._cond:
                if timeout_val == 0.0:
                    if self._value > 0:
                        self._value -= 1
                        return True
                    return False
                end = None if timeout_val is None else time.monotonic() + timeout_val
                while self._value <= 0:
                    if end is None:
                        self._cond.wait()
                    else:
                        remaining = end - time.monotonic()
                        if remaining <= 0:
                            return False
                        self._cond.wait(remaining)
                self._value -= 1
                return True

        def release(self, n: int = 1) -> None:
            if n < 1:
                raise ValueError("semaphore release count must be >= 1")
            with self._cond:
                self._value += n
                self._cond.notify(n)

    class BoundedSemaphore(Semaphore):
        def __init__(self, value: int = 1) -> None:
            super().__init__(value)
            self._initial = int(value)

        def release(self, n: int = 1) -> None:
            if n < 1:
                raise ValueError("semaphore release count must be >= 1")
            self._cond.acquire()
            try:
                if self._value + n > self._initial:
                    raise ValueError("Semaphore released too many times")
                self._value += n
                self._cond.notify(n)
            finally:
                self._cond.release()

    class BrokenBarrierError(RuntimeError):
        pass

    class Barrier:
        def __init__(
            self,
            parties: int,
            action: Callable[[], Any] | None = None,
            timeout: float | None = None,
        ) -> None:
            parties = int(parties)
            if parties <= 0:
                raise ValueError("barrier parties must be greater than zero")
            self._parties = parties
            self._action = action
            self._timeout = timeout
            self._cond = Condition(Lock())
            self._count = 0
            self._generation = 0
            self._broken = False

        @property
        def parties(self) -> int:
            return self._parties

        @property
        def n_waiting(self) -> int:
            return self._count

        @property
        def broken(self) -> bool:
            return self._broken

        def abort(self) -> None:
            with self._cond:
                self._break()

        def reset(self) -> None:
            with self._cond:
                if self._count > 0:
                    self._break()
                self._broken = False
                self._next_generation()

        def wait(self, timeout: float | None = None) -> int:
            if timeout is None:
                timeout_val = self._timeout
            else:
                timeout_val = float(timeout)
                if timeout_val < 0.0:
                    timeout_val = 0.0
            with self._cond:
                if self._broken:
                    raise BrokenBarrierError
                gen = self._generation
                index = self._count
                self._count += 1
                if self._count == self._parties:
                    if self._action is not None:
                        try:
                            self._action()
                        except Exception:
                            self._break()
                            raise
                    self._next_generation()
                    return index
                end = None if timeout_val is None else time.monotonic() + timeout_val
                while True:
                    if end is None:
                        self._cond.wait()
                    else:
                        remaining = end - time.monotonic()
                        if remaining <= 0:
                            self._break()
                            raise BrokenBarrierError
                        self._cond.wait(remaining)
                    if self._broken:
                        raise BrokenBarrierError
                    if gen != self._generation:
                        return index

        def _break(self) -> None:
            self._broken = True
            self._cond.notify_all()

        def _next_generation(self) -> None:
            self._count = 0
            self._generation += 1
            self._cond.notify_all()

    class local:
        def __init__(self) -> None:
            object.__setattr__(self, "_storage", {})

        def _get_dict(self) -> dict[str, Any]:
            storage: dict[int, dict[str, Any]] = object.__getattribute__(
                self, "_storage"
            )
            ident = get_ident()
            slot = storage.get(ident)
            if slot is None:
                slot = {}
                storage[ident] = slot
            return slot

        @property
        def __dict__(self) -> dict[str, Any]:
            return self._get_dict()

        def __getattr__(self, name: str) -> Any:
            data = self._get_dict()
            try:
                return data[name]
            except KeyError as exc:
                raise AttributeError(name) from exc

        def __setattr__(self, name: str, value: Any) -> None:
            if name == "_storage":
                object.__setattr__(self, name, value)
                return
            data = self._get_dict()
            data[name] = value

        def __delattr__(self, name: str) -> None:
            data = self._get_dict()
            if name not in data:
                raise AttributeError(name)
            del data[name]

    class Thread:
        def __init__(
            self,
            _group: Any | None = None,
            target: Callable[..., Any] | None = None,
            name: str | None = None,
            args: tuple[Any, ...] = (),
            kwargs: dict[str, Any] | None = None,
            *,
            daemon: bool | None = None,
            context: Any = _NO_CONTEXT,
        ) -> None:
            if _group is not None:
                raise ValueError("group argument must be None for now")
            if context is not _NO_CONTEXT:
                raise TypeError(
                    "Thread.__init__() got an unexpected keyword argument 'context'"
                )
            self._target = target
            self._args = tuple(args)
            self._kwargs = dict(kwargs) if kwargs else {}
            self._handle: Any | None = None
            self._started = False
            self._ident_cache: int | None = None
            self._native_id_cache: int | None = None
            self._token: int | None = None
            if daemon is not None:
                self._daemon = bool(daemon)
            elif _MAIN_THREAD is None and not _THREAD_BY_IDENT:
                self._daemon = False
            else:
                self._daemon = current_thread().daemon
            self._name = name or _next_thread_name()

        def __repr__(self) -> str:
            status = "started" if self._started else "initial"
            return f"<Thread {self._name} ({status})>"

        @property
        def name(self) -> str:
            return self._name

        @name.setter
        def name(self, value: str) -> None:
            if self._started:
                raise RuntimeError("cannot set name after start")
            self._name = str(value)

        @property
        def daemon(self) -> bool:
            return self._daemon

        @daemon.setter
        def daemon(self, value: bool) -> None:
            if self._started:
                raise RuntimeError("cannot set daemon after start")
            self._daemon = bool(value)

        @property
        def ident(self) -> int | None:
            if self._ident_cache is not None:
                return self._ident_cache
            if not self._started:
                return None
            if self._handle is None:
                return None
            if self._ident_cache is None:
                ident = _thread_ident(self._handle)
                self._ident_cache = _expect_int(ident) if ident is not None else None
            return self._ident_cache

        @property
        def native_id(self) -> int | None:
            if self._native_id_cache is not None:
                return self._native_id_cache
            if not self._started:
                return None
            if self._handle is None:
                return None
            if self._native_id_cache is None:
                ident = _thread_native_id(self._handle)
                self._native_id_cache = (
                    _expect_int(ident) if ident is not None else None
                )
            return self._native_id_cache

        def is_alive(self) -> bool:
            if not self._started:
                return False
            if self._handle is None:
                ident = self._ident_cache
                if ident is None:
                    return False
                return _THREAD_BY_IDENT.get(ident) is self
            return bool(_thread_is_alive(self._handle))

        def start(self) -> None:
            if self._started:
                raise RuntimeError("threads can only be started once")
            module, qualname = _resolve_target(self._target)
            token = _next_thread_token()
            self._token = token
            _THREAD_BY_TOKEN[token] = self
            if _THREAD_SHARED_RUNTIME and self._target is not None:
                _THREAD_SHARED_PAYLOADS[token] = (
                    self._target,
                    self._args,
                    self._kwargs,
                )
            payload_args = self._args
            payload_kwargs = self._kwargs
            if _THREAD_SHARED_RUNTIME:
                payload_args = ()
                payload_kwargs = {}
            payload = (
                token,
                module,
                qualname,
                payload_args,
                payload_kwargs,
                self._name,
                self._daemon,
            )
            try:
                blob = _encode_payload(payload)
            except (TypeError, ValueError) as exc:
                raise TypeError(
                    "thread payload must be serializable "
                    "(None/bool/int/float/bytes/str/list/tuple/dict/set/frozenset); "
                    f"{exc}"
                ) from exc
            handle = _thread_spawn(blob)
            self._handle = handle
            self._started = True
            ident = _thread_ident(self._handle)
            if ident is not None:
                _register_thread_ident(self, _expect_int(ident))
            _THREADS.append(self)

        def join(self, timeout: float | None = None) -> None:
            if not self._started:
                raise RuntimeError("cannot join thread before it is started")
            if self is current_thread():
                raise RuntimeError("cannot join current thread")
            if self._handle is None:
                return None
            if timeout is None:
                _thread_join(self._handle, None)
                return None
            try:
                timeout_val = float(timeout)
            except (TypeError, ValueError) as exc:
                raise TypeError(
                    f"'{type(timeout).__name__}' object cannot be interpreted as an integer or float"
                ) from exc
            if timeout_val < 0.0:
                raise ValueError("timeout value must be a non-negative number")
            _thread_join(self._handle, timeout_val)

        def run(self) -> None:
            if self._target is None:
                return None
            self._target(*self._args, **self._kwargs)

        def _drop(self) -> None:
            if self._handle is None:
                return
            _thread_drop(self._handle)
            self._handle = None

        def __del__(self) -> None:
            if getattr(self, "_handle", None) is None:
                return
            self._drop()

        def setDaemon(self, value: bool) -> None:
            self.daemon = value

        def isDaemon(self) -> bool:
            return self.daemon

        def getName(self) -> str:
            return self.name

        def setName(self, name: str) -> None:
            self.name = name

    def _timer_worker(
        token: int,
        interval: float,
        module: str,
        qualname: str,
        args: tuple[Any, ...],
        kwargs: dict[str, Any],
    ) -> None:
        timer = _TIMER_BY_TOKEN.pop(token, None)
        if timer is not None:
            if not timer.finished.wait(timer.interval):
                timer.function(*timer.args, **timer.kwargs)
            timer.finished.set()
            return
        time.sleep(float(interval))
        target = _resolve_qualname(str(module), str(qualname))
        if not isinstance(args, tuple):
            args = tuple(args)
        if not isinstance(kwargs, dict):
            kwargs = dict(kwargs)
        target(*args, **kwargs)

    class Timer(Thread):
        def __init__(
            self,
            interval: float,
            function: Callable[..., Any],
            args: tuple[Any, ...] | None = None,
            kwargs: dict[str, Any] | None = None,
        ) -> None:
            super().__init__(target=None)
            self.interval = float(interval)
            self.function = function
            self.args = tuple(args) if args else ()
            self.kwargs = dict(kwargs) if kwargs else {}
            self.finished = Event()
            self._timer_token: int | None = None

        def cancel(self) -> None:
            self.finished.set()

        def start(self) -> None:
            module, qualname = _resolve_target(self.function)
            token = _next_timer_token()
            self._timer_token = token
            _TIMER_BY_TOKEN[token] = self
            self._target = _timer_worker
            self._args = (
                token,
                self.interval,
                module,
                qualname,
                self.args,
                self.kwargs,
            )
            self._kwargs = {}
            super().start()

        def run(self) -> None:
            if not self.finished.wait(self.interval):
                self.function(*self.args, **self.kwargs)
            self.finished.set()

    def current_thread() -> Thread:
        ident = get_ident()
        thread = _THREAD_BY_IDENT.get(ident)
        if thread is not None:
            return thread
        if _MAIN_THREAD is not None:
            return _MAIN_THREAD
        return _bootstrap_main_thread()

    def main_thread() -> Thread:
        if _MAIN_THREAD is None:
            return _bootstrap_main_thread()
        return _MAIN_THREAD

    def enumerate() -> list[Thread]:
        active = [t for t in _THREADS if t.is_alive()]
        cur = current_thread()
        if cur not in active:
            active.insert(0, cur)
        return list(active)

    def active_count() -> int:
        return len(enumerate())

    TIMEOUT_MAX = 9223372036.0

    def _bootstrap_main_thread() -> Thread:
        global _MAIN_THREAD
        thread = Thread(target=None, name="MainThread", daemon=False)
        thread._started = True
        _register_thread_ident(thread, get_ident(), get_native_id())
        _MAIN_THREAD = thread
        return thread

    def _register_thread_ident(
        thread: Thread, ident: int, native_id: int | None = None
    ) -> None:
        thread._ident_cache = ident
        if native_id is None:
            native_id = get_native_id()
        thread._native_id_cache = native_id
        _THREAD_BY_IDENT[ident] = thread

    def _molt_thread_run(payload: bytes) -> None:
        decoded = _decode_payload(payload)
        if not isinstance(decoded, tuple) or len(decoded) not in (6, 7):
            raise TypeError("invalid thread payload")
        token: int | None
        if len(decoded) == 7:
            token, module, qualname, args, kwargs, name, daemon = decoded
        else:
            token = None
            module, qualname, args, kwargs, name, daemon = decoded
        ident = get_ident()
        thread = _THREAD_BY_TOKEN.pop(token, None) if token is not None else None
        created = False
        if thread is None:
            thread = Thread(target=None, name=str(name) if name is not None else None)
            created = True
        thread._started = True
        if name is not None:
            thread._name = str(name)
        if daemon is not None:
            thread._daemon = bool(daemon)
        _register_thread_ident(thread, ident, get_native_id())
        if created:
            _THREADS.append(thread)
        entry = None
        if _THREAD_SHARED_RUNTIME and token is not None:
            entry = _THREAD_SHARED_PAYLOADS.pop(token, None)
        try:
            _invoke_thread_hooks()
            if module is None or qualname is None:
                if thread is not None:
                    if (
                        entry is not None
                        and thread._target is None
                        and type(thread) is Thread
                    ):
                        target, args, kwargs = entry
                        target(*args, **kwargs)
                    else:
                        thread.run()
                elif entry is not None:
                    target, args, kwargs = entry
                    target(*args, **kwargs)
                return None
            if not isinstance(args, tuple):
                raise TypeError("thread args must be a tuple")
            if not isinstance(kwargs, dict):
                raise TypeError("thread kwargs must be a dict")
            target = _resolve_qualname(str(module), str(qualname))
            target(*args, **kwargs)
        except BaseException as exc:
            _call_excepthook(thread, exc)
        finally:
            if thread is not _MAIN_THREAD:
                _THREAD_BY_IDENT.pop(ident, None)

    _bootstrap_main_thread()
