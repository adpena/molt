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
_MOLT_IS_STRING_OBJ = _as_callable(_require_intrinsic("molt_is_string_obj", globals()))

# Compiled runtimes are the host; avoid recursive sys -> importlib -> sys.

__all__ = [
    "argv",
    "executable",
    "platform",
    "version",
    "version_info",
    "breakpointhook",
    "__breakpointhook__",
    "path",
    "meta_path",
    "path_hooks",
    "path_importer_cache",
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
    "getfilesystemencodeerrors",
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
_MOLT_SYS_GETFILESYSTEMENCODEERRORS = _as_callable(
    _require_intrinsic("molt_sys_getfilesystemencodeerrors", globals())
)
_MOLT_SYS_BOOTSTRAP_PAYLOAD = _as_callable(
    _require_intrinsic("molt_sys_bootstrap_payload", globals())
)

raw_argv = _MOLT_GETARGV()
if raw_argv is None:
    raise RuntimeError("molt_getargv returned None")
if not isinstance(raw_argv, (list, tuple)):
    raise RuntimeError(f"molt_getargv returned {type(raw_argv)!r}")
argv = list(cast("Iterable[object]", raw_argv))

_exe_val = _MOLT_SYS_EXECUTABLE()
if not isinstance(_exe_val, str):
    raise RuntimeError("molt_sys_executable returned invalid value")
executable = _exe_val


def _resolve_platform(_getter: object = _MOLT_SYS_PLATFORM) -> str:
    if not callable(_getter):
        raise RuntimeError("molt_sys_platform intrinsic unavailable")
    value = _getter()
    if not isinstance(value, str):
        raise RuntimeError("molt_sys_platform returned invalid value")
    return value


def exit(code: object = None) -> None:
    raise SystemExit(code)


def __breakpointhook__(*args: object, **kwargs: object) -> object:
    """Default breakpoint hook.

    CPython defaults to launching pdb; Molt compiled binaries do not ship an
    interactive debugger by default, so this is a fail-fast stub. Tests patch
    sys.breakpointhook to validate builtins.breakpoint dispatch.
    """

    del args, kwargs
    raise RuntimeError(
        "MOLT_COMPAT_ERROR: sys.breakpointhook is unavailable in compiled Molt binaries"
    )


breakpointhook = __breakpointhook__


platform = _resolve_platform()
version = _MOLT_SYS_VERSION()
version_info = cast(tuple[object, ...], _MOLT_SYS_VERSION_INFO())
path: list[str] = []
meta_path: list[object] = []
path_hooks: list[object] = []
path_importer_cache: dict[str, object] = {}


def _bootstrap_module_file() -> str | None:
    value = globals().get("__file__")
    if isinstance(value, str):
        return value
    return None


def _bootstrap_str_list(
    payload: dict[object, object], key: str, intrinsic_name: str
) -> list[str]:
    value = payload.get(key)
    if not isinstance(value, (list, tuple)):
        raise RuntimeError(f"{intrinsic_name} returned invalid value")
    out: list[str] = []
    for entry in value:
        if not _MOLT_IS_STRING_OBJ(entry):
            entry_type = type(entry).__name__
            raise RuntimeError(
                f"{intrinsic_name} returned invalid value (expected str entry, got {entry_type})"
            )
        out.append(entry)  # type: ignore[arg-type]
    return out


def _bootstrap_str(payload: dict[object, object], key: str, intrinsic_name: str) -> str:
    value = payload.get(key)
    if not _MOLT_IS_STRING_OBJ(value):
        value_type = type(value).__name__
        raise RuntimeError(
            f"{intrinsic_name} returned invalid value (expected str, got {value_type})"
        )
    return value  # type: ignore[return-value]


def _bootstrap_str_or_none(
    payload: dict[object, object], key: str, intrinsic_name: str
) -> str | None:
    value = payload.get(key)
    if value is None:
        return None
    if not _MOLT_IS_STRING_OBJ(value):
        value_type = type(value).__name__
        raise RuntimeError(
            f"{intrinsic_name} returned invalid value (expected str|None, got {value_type})"
        )
    return value  # type: ignore[return-value]


def _bootstrap_bool(
    payload: dict[object, object], key: str, intrinsic_name: str
) -> bool:
    value = payload.get(key)
    if not isinstance(value, bool):
        raise RuntimeError(f"{intrinsic_name} returned invalid value")
    return value


_BOOTSTRAP_MODULE_FILE = _bootstrap_module_file()
_bootstrap_payload_value = _MOLT_SYS_BOOTSTRAP_PAYLOAD(_BOOTSTRAP_MODULE_FILE)
if not isinstance(_bootstrap_payload_value, dict):
    raise RuntimeError("molt_sys_bootstrap_payload returned invalid value")

path = _bootstrap_str_list(
    _bootstrap_payload_value, "path", "molt_sys_bootstrap_payload"
)
_molt_bootstrap_pythonpath = tuple(
    _bootstrap_str_list(
        _bootstrap_payload_value, "pythonpath_entries", "molt_sys_bootstrap_payload"
    )
)
_molt_bootstrap_module_roots = tuple(
    _bootstrap_str_list(
        _bootstrap_payload_value, "module_roots_entries", "molt_sys_bootstrap_payload"
    )
)
_molt_bootstrap_venv_site_packages = tuple(
    _bootstrap_str_list(
        _bootstrap_payload_value,
        "venv_site_packages_entries",
        "molt_sys_bootstrap_payload",
    )
)
_molt_bootstrap_pwd = _bootstrap_str(
    _bootstrap_payload_value, "pwd", "molt_sys_bootstrap_payload"
)
_molt_bootstrap_include_cwd = _bootstrap_bool(
    _bootstrap_payload_value, "include_cwd", "molt_sys_bootstrap_payload"
)
_molt_bootstrap_stdlib_root = _bootstrap_str_or_none(
    _bootstrap_payload_value, "stdlib_root", "molt_sys_bootstrap_payload"
)


class _LazyStdio:
    def __init__(self, intrinsic: object, readable: bool, writable: bool) -> None:
        self._intrinsic = intrinsic
        del readable, writable
        self._handle: object | None = None

    def _resolve(self) -> object:
        if self._handle is None:
            intrinsic = self._intrinsic
            if isinstance(intrinsic, str):
                intrinsic = _require_intrinsic(intrinsic)
            if not callable(intrinsic):
                raise RuntimeError("sys stdio intrinsic unavailable")
            handle = intrinsic()
            if handle is None:
                raise RuntimeError("sys stdio intrinsic returned invalid value")
            self._handle = handle
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
_fs_encode_errors = _MOLT_SYS_GETFILESYSTEMENCODEERRORS()
if not isinstance(_fs_encode_errors, str):
    raise RuntimeError("molt_sys_getfilesystemencodeerrors returned invalid value")


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


def getrecursionlimit() -> int:
    return int(cast(int, _MOLT_GETRECURSIONLIMIT()))


def setrecursionlimit(limit: int) -> None:
    _MOLT_SETRECURSIONLIMIT(limit)
    return None


def exc_info() -> tuple[object, object, object]:
    exc = _MOLT_EXCEPTION_ACTIVE()
    if exc is None:
        exc = _MOLT_EXCEPTION_LAST()
    if exc is None:
        return None, None, None
    return type(exc), exc, getattr(exc, "__traceback__", None)


def _getframe(depth: int = 0) -> object | None:
    return _MOLT_GETFRAME(depth + 2)


def getdefaultencoding() -> str:
    return _default_encoding


def getfilesystemencoding() -> str:
    return _fs_encoding


def getfilesystemencodeerrors() -> str:
    return _fs_encode_errors


def get_asyncgen_hooks() -> object:
    hooks = _MOLT_ASYNCGEN_HOOKS_GET()
    if not isinstance(hooks, tuple) or len(hooks) != 2:
        raise RuntimeError("asyncgen hooks intrinsic returned invalid value")
    firstiter, finalizer = hooks
    return asyncgen_hooks(firstiter, finalizer)


def set_asyncgen_hooks(
    *, firstiter: object | None = None, finalizer: object | None = None
) -> None:
    _MOLT_ASYNCGEN_HOOKS_SET(firstiter, finalizer)
    return None
