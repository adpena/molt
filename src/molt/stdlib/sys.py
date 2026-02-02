"""Minimal sys shim for Molt."""

from __future__ import annotations

from collections.abc import Callable, Iterable
import os as _os

try:
    from typing import cast
except Exception:

    def cast(_tp, value):  # type: ignore[override]
        return value


import builtins as _builtins


def _load_intrinsic(name: str) -> object | None:
    direct = globals().get(name)
    if direct is not None:
        return direct
    return getattr(_builtins, name, None)


def _as_callable(value: object) -> Callable[..., object] | None:
    if callable(value):
        return value  # type: ignore[return-value]
    return None


# Compiled runtimes are the host; avoid recursive sys -> importlib -> sys.
_py_sys = None

__all__ = [
    "argv",
    "executable",
    "platform",
    "version",
    "version_info",
    "path",
    "modules",
    "stdin",
    "stdout",
    "stderr",
    "__stdin__",
    "__stdout__",
    "__stderr__",
    "getrecursionlimit",
    "setrecursionlimit",
    "exc_info",
    "_getframe",
    "getdefaultencoding",
    "getfilesystemencoding",
    "asyncgen_hooks",
    "get_asyncgen_hooks",
    "set_asyncgen_hooks",
    "exit",
]

_MOLT_GETARGV = _as_callable(_load_intrinsic("_molt_getargv"))
_MOLT_SYS_EXECUTABLE = _as_callable(_load_intrinsic("_molt_sys_executable"))
_MOLT_GETRECURSIONLIMIT = _as_callable(_load_intrinsic("_molt_getrecursionlimit"))
_MOLT_SETRECURSIONLIMIT = _as_callable(_load_intrinsic("_molt_setrecursionlimit"))
_MOLT_EXCEPTION_ACTIVE = _as_callable(_load_intrinsic("_molt_exception_active"))
_MOLT_EXCEPTION_LAST = _as_callable(_load_intrinsic("_molt_exception_last"))
_MOLT_ASYNCGEN_HOOKS_GET = _as_callable(_load_intrinsic("_molt_asyncgen_hooks_get"))
_MOLT_ASYNCGEN_HOOKS_SET = _as_callable(_load_intrinsic("_molt_asyncgen_hooks_set"))
_MOLT_SYS_VERSION_INFO = _as_callable(_load_intrinsic("_molt_sys_version_info"))
_MOLT_SYS_VERSION = _as_callable(_load_intrinsic("_molt_sys_version"))
_MOLT_SYS_PLATFORM = _as_callable(_load_intrinsic("_molt_sys_platform"))
_MOLT_SYS_STDIN = _as_callable(_load_intrinsic("_molt_sys_stdin"))
_MOLT_SYS_STDOUT = _as_callable(_load_intrinsic("_molt_sys_stdout"))
_MOLT_SYS_STDERR = _as_callable(_load_intrinsic("_molt_sys_stderr"))
_MOLT_ENV_GET_RAW = _as_callable(_load_intrinsic("_molt_env_get_raw"))
_MOLT_GETFRAME = _as_callable(_load_intrinsic("_molt_getframe"))

if _MOLT_GETARGV is not None:
    try:
        raw_argv = _MOLT_GETARGV()
    except Exception:
        raw_argv = None
    if isinstance(raw_argv, Iterable):
        argv = list(raw_argv)
    else:
        argv = []
elif _py_sys is not None:
    argv = list(getattr(_py_sys, "argv", []))
else:
    argv = []

if _MOLT_SYS_EXECUTABLE is not None:
    try:
        _exe_val = _MOLT_SYS_EXECUTABLE()
    except Exception:
        _exe_val = None
    executable = _exe_val if isinstance(_exe_val, str) else (argv[0] if argv else "")
else:
    executable = argv[0] if argv else ""

_existing_modules = globals().get("modules")


def _resolve_platform() -> str:
    if _MOLT_SYS_PLATFORM is not None:
        try:
            value = _MOLT_SYS_PLATFORM()
            if isinstance(value, str):
                return value
        except Exception:
            pass
    if _py_sys is not None:
        try:
            value = getattr(_py_sys, "platform", None)
            if isinstance(value, str):
                return value
        except Exception:
            pass
    return "molt"


def exit(code: object = None) -> None:
    raise SystemExit(code)


if _py_sys is not None:
    platform = _resolve_platform()
    version = None
    version_info = None
    if _MOLT_SYS_VERSION_INFO is not None:
        try:
            version_info = _MOLT_SYS_VERSION_INFO()
        except Exception:
            version_info = None
    if _MOLT_SYS_VERSION is not None:
        try:
            version = _MOLT_SYS_VERSION()
        except Exception:
            version = None
    if version is None:
        version = getattr(_py_sys, "version", "3.12.0 (molt)")
    if version_info is None:
        version_info = getattr(_py_sys, "version_info", (3, 12, 0, "final", 0))
    path = list(getattr(_py_sys, "path", []))
    modules = getattr(_py_sys, "modules", _existing_modules or {})
    stdin = getattr(_py_sys, "stdin", None)
    stdout = getattr(_py_sys, "stdout", None)
    stderr = getattr(_py_sys, "stderr", None)
    __stdin__ = getattr(_py_sys, "__stdin__", stdin)
    __stdout__ = getattr(_py_sys, "__stdout__", stdout)
    __stderr__ = getattr(_py_sys, "__stderr__", stderr)
    _default_encoding = getattr(_py_sys, "getdefaultencoding", lambda: "utf-8")()
    _fs_encoding = getattr(_py_sys, "getfilesystemencoding", lambda: "utf-8")()
else:
    platform = _resolve_platform()
    version = None
    version_info = None
    if _MOLT_SYS_VERSION_INFO is not None:
        try:
            version_info = _MOLT_SYS_VERSION_INFO()
        except Exception:
            version_info = None
    if _MOLT_SYS_VERSION is not None:
        try:
            version = _MOLT_SYS_VERSION()
        except Exception:
            version = None
    if version is None:
        version = "3.12.0 (molt)"
    if version_info is None:
        version_info = (3, 12, 0, "final", 0)
    path = []
    if _MOLT_ENV_GET_RAW is not None:
        try:
            raw_path = _MOLT_ENV_GET_RAW("PYTHONPATH", "")
        except Exception:
            raw_path = ""
    else:
        try:
            raw_path = _os.environ.get("PYTHONPATH", "")
        except Exception:
            raw_path = ""
    if isinstance(raw_path, str) and raw_path:
        sep = ";" if platform.startswith("win") else ":"
        path = [part for part in raw_path.split(sep) if part]

    def _append_stdlib_path(paths: list[str]) -> None:
        file_path = globals().get("__file__")
        if isinstance(file_path, str) and file_path:
            stdlib_root = _os.path.dirname(file_path)
            if stdlib_root and stdlib_root not in paths:
                paths.append(stdlib_root)

    def _read_env_flag(name: str) -> str:
        if _MOLT_ENV_GET_RAW is not None:
            try:
                value = _MOLT_ENV_GET_RAW(name, "")
            except Exception:
                value = ""
        else:
            try:
                value = _os.environ.get(name, "")
            except Exception:
                value = ""
        return str(value) if value is not None else ""

    def _append_module_roots(paths: list[str]) -> None:
        raw = _read_env_flag("MOLT_MODULE_ROOTS")
        if not raw:
            return
        sep = ";" if platform.startswith("win") else ":"
        for entry in raw.split(sep):
            if entry and entry not in paths:
                paths.append(entry)

    def _append_cwd_path(paths: list[str]) -> None:
        dev_trusted = _read_env_flag("MOLT_DEV_TRUSTED").strip().lower()
        if dev_trusted in {"0", "false", "no"}:
            return
        if "" not in paths:
            paths.insert(0, "")

    def _append_host_site_packages(paths: list[str]) -> None:
        if _MOLT_ENV_GET_RAW is not None:
            try:
                trusted = str(_MOLT_ENV_GET_RAW("MOLT_TRUSTED", ""))
            except Exception:
                trusted = ""
        else:
            try:
                trusted = str(_os.environ.get("MOLT_TRUSTED", ""))
            except Exception:
                trusted = ""
        if trusted.strip().lower() not in {"1", "true", "yes", "on"}:
            return
        exe_path = executable
        if not exe_path:
            return
        exe_dir = _os.path.dirname(exe_path)
        candidates: list[str] = []
        if platform.startswith("win"):
            base = (
                _os.path.dirname(exe_dir)
                if _os.path.basename(exe_dir).lower() == "scripts"
                else exe_dir
            )
            candidates.append(_os.path.join(base, "Lib", "site-packages"))
        else:
            base = (
                _os.path.dirname(exe_dir)
                if _os.path.basename(exe_dir) == "bin"
                else exe_dir
            )
            major, minor = int(version_info[0]), int(version_info[1])
            candidates.append(
                _os.path.join(base, "lib", f"python{major}.{minor}", "site-packages")
            )
            candidates.append(
                _os.path.join(base, "lib", f"python{major}.{minor}", "dist-packages")
            )
        for candidate in candidates:
            if candidate and candidate not in paths:
                paths.append(candidate)

    _append_stdlib_path(path)
    _append_module_roots(path)
    _append_cwd_path(path)
    _append_host_site_packages(path)
    if _existing_modules is None:
        modules: dict[str, object] = {}
    else:
        modules = _existing_modules

    class _NullIO:
        def __init__(self, readable: bool, writable: bool) -> None:
            self._readable = readable
            self._writable = writable
            self.encoding = "utf-8"

        def readable(self) -> bool:
            return self._readable

        def writable(self) -> bool:
            return self._writable

        def read(self, _size: int | None = None) -> str:
            return ""

        def readline(self, _size: int | None = None) -> str:
            return ""

        def write(self, data: str | bytes | bytearray) -> int:
            try:
                return len(data)
            except Exception:
                return 0

        def writelines(self, lines: Iterable[str | bytes | bytearray]) -> None:
            for line in lines:
                self.write(line)

        def flush(self) -> None:
            return None

        def isatty(self) -> bool:
            return False

        def fileno(self) -> int:
            raise OSError("invalid file descriptor")

        def close(self) -> None:
            return None

    def _stdio_from_intrinsic(
        intrinsic: Callable[..., object] | None, fd: int, mode: str
    ) -> object | None:
        if intrinsic is not None:
            try:
                return intrinsic()
            except Exception:
                return None
        stdio = _open_stdio(fd, mode)
        if stdio is not None:
            return stdio
        return _NullIO("r" in mode, "w" in mode)

    def _open_stdio(fd: int, mode: str) -> object | None:
        try:
            return _builtins.open(fd, mode, closefd=False)
        except Exception:
            return None

    stdin = _stdio_from_intrinsic(_MOLT_SYS_STDIN, 0, "r")
    stdout = _stdio_from_intrinsic(_MOLT_SYS_STDOUT, 1, "w")
    stderr = _stdio_from_intrinsic(_MOLT_SYS_STDERR, 2, "w")
    __stdin__ = stdin
    __stdout__ = stdout
    __stderr__ = stderr
    _default_encoding = "utf-8"
    _fs_encoding = "utf-8"

_recursionlimit = 1000


class asyncgen_hooks(tuple):
    __slots__ = ()

    def __new__(
        cls, firstiter: object | None, finalizer: object | None
    ) -> "asyncgen_hooks":
        return tuple.__new__(cls, (firstiter, finalizer))

    @property
    def firstiter(self) -> object | None:
        return self[0]

    @property
    def finalizer(self) -> object | None:
        return self[1]


_ASYNCGEN_FIRSTITER: object | None = None
_ASYNCGEN_FINALIZER: object | None = None


def getrecursionlimit() -> int:
    if _MOLT_GETRECURSIONLIMIT is not None:
        return int(cast(int, _MOLT_GETRECURSIONLIMIT()))
    return _recursionlimit


def setrecursionlimit(limit: int) -> None:
    global _recursionlimit
    if _MOLT_SETRECURSIONLIMIT is not None:
        _MOLT_SETRECURSIONLIMIT(limit)
        return
    if not isinstance(limit, int):
        name = type(limit).__name__
        raise TypeError(f"'{name}' object cannot be interpreted as an integer")
    if limit < 1:
        raise ValueError("recursion limit must be greater or equal than 1")
    _recursionlimit = limit


def exc_info() -> tuple[object, object, object]:
    if _py_sys is not None:
        return _py_sys.exc_info()
    exc = None
    if _MOLT_EXCEPTION_ACTIVE is not None:
        exc = _MOLT_EXCEPTION_ACTIVE()
    if exc is None:
        if _MOLT_EXCEPTION_LAST is not None:
            exc = _MOLT_EXCEPTION_LAST()
    if exc is None:
        return None, None, None
    return type(exc), exc, getattr(exc, "__traceback__", None)


def _getframe(depth: int = 0) -> object | None:
    if _MOLT_GETFRAME is not None:
        try:
            return _MOLT_GETFRAME(depth + 2)
        except Exception:
            return None
    # TODO(introspection, owner:runtime, milestone:TC2, priority:P2, status:partial): implement sys._getframe for compiled runtimes.
    if _py_sys is not None and hasattr(_py_sys, "_getframe"):
        try:
            return _py_sys._getframe(depth + 1)
        except Exception:
            return None
    return None


def getdefaultencoding() -> str:
    return _default_encoding


def getfilesystemencoding() -> str:
    return _fs_encoding


def get_asyncgen_hooks() -> object:
    if _MOLT_ASYNCGEN_HOOKS_GET is not None:
        hooks = _MOLT_ASYNCGEN_HOOKS_GET()
        if isinstance(hooks, tuple) and len(hooks) == 2:
            firstiter, finalizer = hooks
        else:
            firstiter, finalizer = None, None
        return asyncgen_hooks(firstiter, finalizer)
    return asyncgen_hooks(_ASYNCGEN_FIRSTITER, _ASYNCGEN_FINALIZER)


def set_asyncgen_hooks(
    *, firstiter: object | None = None, finalizer: object | None = None
) -> None:
    global _ASYNCGEN_FIRSTITER, _ASYNCGEN_FINALIZER
    if _MOLT_ASYNCGEN_HOOKS_SET is not None:
        _MOLT_ASYNCGEN_HOOKS_SET(firstiter, finalizer)
        return None
    if firstiter is not None and not callable(firstiter):
        raise TypeError("firstiter must be callable or None")
    if finalizer is not None and not callable(finalizer):
        raise TypeError("finalizer must be callable or None")
    _ASYNCGEN_FIRSTITER = firstiter
    _ASYNCGEN_FINALIZER = finalizer
    return None
