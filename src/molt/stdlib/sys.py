"""Minimal sys shim for Molt."""

from __future__ import annotations

import _intrinsics as _stdlib_intrinsics
from _intrinsics import require_intrinsic as _require_intrinsic


def cast(_tp, value):  # type: ignore[override]
    return value


# Ensure sys.modules exists early to avoid circular import failures.
_existing_modules = globals().get("modules")
if _existing_modules is None:
    modules: dict[str, object] = {}
else:
    modules = _existing_modules
modules.setdefault("_intrinsics", _stdlib_intrinsics)


TYPE_CHECKING = False

if TYPE_CHECKING:
    from collections.abc import Callable, Iterable
else:

    class _TypingAlias:
        __slots__ = ()

        def __getitem__(self, _item):
            return self

    Callable = _TypingAlias()
    Iterable = _TypingAlias()


def _as_callable(value: object) -> Callable[..., object]:
    if callable(value):
        return value  # type: ignore[return-value]
    raise RuntimeError("intrinsic unavailable")


# Define early to avoid circular-import NameError during stdlib bootstrap.
_MOLT_GETFRAME = _as_callable(_require_intrinsic("molt_getframe", globals()))

# Compiled runtimes are the host; avoid recursive sys -> importlib -> sys.

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

_MOLT_GETARGV = _as_callable(_require_intrinsic("molt_getargv", globals()))
_MOLT_SYS_EXECUTABLE = _as_callable(
    _require_intrinsic("molt_sys_executable", globals())
)
_MOLT_GETRECURSIONLIMIT = _as_callable(
    _require_intrinsic("molt_getrecursionlimit", globals())
)
_MOLT_SETRECURSIONLIMIT = _as_callable(
    _require_intrinsic("molt_setrecursionlimit", globals())
)
_MOLT_EXCEPTION_ACTIVE = _as_callable(
    _require_intrinsic("molt_exception_active", globals())
)
_MOLT_EXCEPTION_LAST = _as_callable(
    _require_intrinsic("molt_exception_last", globals())
)
_MOLT_ASYNCGEN_HOOKS_GET = _as_callable(
    _require_intrinsic("molt_asyncgen_hooks_get", globals())
)
_MOLT_ASYNCGEN_HOOKS_SET = _as_callable(
    _require_intrinsic("molt_asyncgen_hooks_set", globals())
)
_MOLT_SYS_VERSION_INFO = _as_callable(
    _require_intrinsic("molt_sys_version_info", globals())
)
_MOLT_SYS_VERSION = _as_callable(_require_intrinsic("molt_sys_version", globals()))
_MOLT_SYS_PLATFORM = _as_callable(_require_intrinsic("molt_sys_platform", globals()))
_MOLT_SYS_STDIN = _as_callable(_require_intrinsic("molt_sys_stdin", globals()))
_MOLT_SYS_STDOUT = _as_callable(_require_intrinsic("molt_sys_stdout", globals()))
_MOLT_SYS_STDERR = _as_callable(_require_intrinsic("molt_sys_stderr", globals()))
_MOLT_ENV_GET = _as_callable(_require_intrinsic("molt_env_get", globals()))

if _MOLT_GETARGV is None:
    raise RuntimeError("molt_getargv intrinsic missing")
raw_argv = _MOLT_GETARGV()
if raw_argv is None:
    raise RuntimeError("molt_getargv returned None")
if not isinstance(raw_argv, (list, tuple)):
    raise RuntimeError(f"molt_getargv returned {type(raw_argv)!r}")
argv = list(cast("Iterable[object]", raw_argv))

_exe_val = _MOLT_SYS_EXECUTABLE()
executable = _exe_val if isinstance(_exe_val, str) else (argv[0] if argv else "")


def _resolve_platform(
    _getter: object = _MOLT_SYS_PLATFORM,
    _resolver: object = _require_intrinsic,
) -> str:
    if callable(_getter):
        value = _getter()
    else:
        # Re-resolve if the cached intrinsic was shadowed during bootstrap.
        value = _as_callable(_resolver("molt_sys_platform", globals()))()
    return value if isinstance(value, str) else "molt"


def exit(code: object = None) -> None:
    raise SystemExit(code)


platform = _resolve_platform()
version = _MOLT_SYS_VERSION()
version_info = cast(tuple[object, ...], _MOLT_SYS_VERSION_INFO())
path = []
raw_path = _MOLT_ENV_GET("PYTHONPATH", "")
if isinstance(raw_path, str) and raw_path:
    sep = ";" if platform.startswith("win") else ":"
    path = [part for part in raw_path.split(sep) if part]


def _path_sep() -> str:
    return "\\" if platform.startswith("win") else "/"


def _path_dirname(value: str) -> str:
    sep = _path_sep()
    alt = "/" if sep == "\\" else "\\"
    raw = value.replace(alt, sep).rstrip(sep)
    if not raw:
        return sep if value.startswith(sep) else ""
    idx = raw.rfind(sep)
    if idx < 0:
        return ""
    if idx == 0:
        return sep
    return raw[:idx]


def _path_basename(value: str) -> str:
    sep = _path_sep()
    alt = "/" if sep == "\\" else "\\"
    raw = value.replace(alt, sep).rstrip(sep)
    if not raw:
        return ""
    idx = raw.rfind(sep)
    if idx < 0:
        return raw
    return raw[idx + 1 :]


def _path_join(*parts: str) -> str:
    sep = _path_sep()
    cleaned = [part for part in parts if part]
    if not cleaned:
        return ""
    out = cleaned[0].rstrip("/\\")
    for part in cleaned[1:]:
        out = f"{out}{sep}{part.lstrip('/\\')}"
    return out


def _append_stdlib_path(paths: list[str]) -> None:
    file_path = globals().get("__file__")
    if isinstance(file_path, str) and file_path:
        stdlib_root = _path_dirname(file_path)
        if stdlib_root and stdlib_root not in paths:
            paths.append(stdlib_root)


def _read_env_flag(name: str) -> str:
    value = _MOLT_ENV_GET(name, "")
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
    pwd = _read_env_flag("PWD")
    if pwd and pwd not in paths:
        paths.append(pwd)
    if "" not in paths:
        paths.insert(0, "")


def _append_host_site_packages(paths: list[str]) -> None:
    trusted = str(_MOLT_ENV_GET("MOLT_TRUSTED", ""))
    if trusted.strip().lower() not in {"1", "true", "yes", "on"}:
        return
    exe_path = executable
    if not exe_path:
        return
    exe_dir = _path_dirname(exe_path)
    candidates: list[str] = []
    if platform.startswith("win"):
        base = (
            _path_dirname(exe_dir)
            if _path_basename(exe_dir).lower() == "scripts"
            else exe_dir
        )
        candidates.append(_path_join(base, "Lib", "site-packages"))
    else:
        base = _path_dirname(exe_dir) if _path_basename(exe_dir) == "bin" else exe_dir
        major, minor = int(version_info[0]), int(version_info[1])
        candidates.append(
            _path_join(base, "lib", f"python{major}.{minor}", "site-packages")
        )
        candidates.append(
            _path_join(base, "lib", f"python{major}.{minor}", "dist-packages")
        )
    for candidate in candidates:
        if candidate and candidate not in paths:
            paths.append(candidate)


_append_stdlib_path(path)
_append_module_roots(path)
_append_cwd_path(path)
_append_host_site_packages(path)


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


class _LazyStdio:
    def __init__(self, intrinsic: object, readable: bool, writable: bool) -> None:
        self._intrinsic = intrinsic
        self._readable = readable
        self._writable = writable
        self._handle: object | None = None

    def _resolve(self) -> object:
        if self._handle is None:
            intrinsic = self._intrinsic
            if isinstance(intrinsic, str):
                intrinsic = _require_intrinsic(intrinsic)
            if callable(intrinsic):
                self._handle = intrinsic()
            else:
                self._handle = _NullIO(self._readable, self._writable)
        return self._handle

    def __getattr__(self, name: str) -> object:
        return getattr(self._resolve(), name)

    def __iter__(self):
        return iter(cast("Iterable[object]", self._resolve()))

    def __next__(self):
        return next(cast("Iterator[object]", self._resolve()))

    def __enter__(self):
        target = self._resolve()
        enter = getattr(target, "__enter__", None)
        if enter is None:
            return target
        return enter()

    def __exit__(self, exc_type, exc, tb):
        target = self._resolve()
        exit_fn = getattr(target, "__exit__", None)
        if exit_fn is None:
            return False
        return exit_fn(exc_type, exc, tb)


stdin = _LazyStdio(_MOLT_SYS_STDIN, True, False)
stdout = _LazyStdio(_MOLT_SYS_STDOUT, False, True)
stderr = _LazyStdio(_MOLT_SYS_STDERR, False, True)
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
    exc = None
    if _MOLT_EXCEPTION_ACTIVE is not None:
        exc = _MOLT_EXCEPTION_ACTIVE()
    if exc is None:
        if _MOLT_EXCEPTION_LAST is not None:
            exc = _MOLT_EXCEPTION_LAST()
    if exc is None:
        return None, None, None
    return type(exc), exc, getattr(exc, "__traceback__", None)


_FRAME_PATCHED_ATTR = "__molt_frame_patched__"


def _frame_attr(frame: object, name: str) -> object | None:
    try:
        return getattr(frame, name)
    except Exception:
        return None


def _resolve_frame_globals(frame: object) -> dict[str, object] | None:
    val = _frame_attr(frame, "f_globals")
    if isinstance(val, dict):
        return cast(dict[str, object], val)
    back = _frame_attr(frame, "f_back")
    if back is not None:
        back_globals = _resolve_frame_globals(back)
        if isinstance(back_globals, dict):
            return back_globals
    code = _frame_attr(frame, "f_code")
    filename = None
    if code is not None:
        try:
            filename = getattr(code, "co_filename", None)
        except Exception:
            filename = None
    if filename:
        for mod in modules.values():
            try:
                mod_file = getattr(mod, "__file__", None)
            except Exception:
                continue
            if mod_file == filename:
                mod_dict = getattr(mod, "__dict__", None)
                if isinstance(mod_dict, dict):
                    return mod_dict
    main_mod = modules.get("__main__")
    if main_mod is not None:
        main_dict = getattr(main_mod, "__dict__", None)
        if isinstance(main_dict, dict):
            return main_dict
    return None


def _resolve_frame_locals(
    frame: object, globals_dict: dict[str, object] | None
) -> dict[str, object]:
    if isinstance(globals_dict, dict):
        code = _frame_attr(frame, "f_code")
        name = None
        if code is not None:
            try:
                name = getattr(code, "co_name", None)
            except Exception:
                name = None
        if name == "<module>":
            return globals_dict
    return {}


def _patch_frame(frame: object | None, depth: int) -> object | None:
    if frame is None:
        return None
    current = frame
    current_depth = depth
    seen: set[int] = set()
    while current is not None:
        obj_id = id(current)
        if obj_id in seen:
            break
        seen.add(obj_id)
        try:
            if getattr(current, _FRAME_PATCHED_ATTR, False):
                break
            setattr(current, _FRAME_PATCHED_ATTR, True)
        except Exception:
            pass
        back = _frame_attr(current, "f_back")
        if back is None and _MOLT_GETFRAME is not None:
            try:
                back = _MOLT_GETFRAME(current_depth + 1)
            except Exception:
                back = None
        if back is not None:
            try:
                setattr(current, "f_back", back)
            except Exception:
                pass
        globals_dict = _resolve_frame_globals(current)
        if isinstance(globals_dict, dict):
            try:
                setattr(current, "f_globals", globals_dict)
            except Exception:
                pass
        locals_dict = _frame_attr(current, "f_locals")
        if not isinstance(locals_dict, dict):
            locals_dict = _resolve_frame_locals(current, globals_dict)
            try:
                setattr(current, "f_locals", locals_dict)
            except Exception:
                pass
        current = back
        current_depth += 1
    return frame


def _getframe(depth: int = 0) -> object | None:
    global _MOLT_GETFRAME
    if _MOLT_GETFRAME is None:
        _MOLT_GETFRAME = _as_callable(_require_intrinsic("molt_getframe", globals()))
    if _MOLT_GETFRAME is not None:
        try:
            frame = _MOLT_GETFRAME(depth + 2)
            return _patch_frame(frame, depth + 2)
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
