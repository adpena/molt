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
    "hexversion",
    "api_version",
    "abiflags",
    "flags",
    "implementation",
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
    "UnraisableHookArgs",
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
_MOLT_SYS_HEXVERSION = _as_callable(
    _require_intrinsic("molt_sys_hexversion", globals())
)
_MOLT_SYS_API_VERSION = _as_callable(
    _require_intrinsic("molt_sys_api_version", globals())
)
_MOLT_SYS_ABIFLAGS = _as_callable(_require_intrinsic("molt_sys_abiflags", globals()))
_MOLT_SYS_IMPLEMENTATION_PAYLOAD = _as_callable(
    _require_intrinsic("molt_sys_implementation_payload", globals())
)
_MOLT_SYS_FLAGS_PAYLOAD = _as_callable(
    _require_intrinsic("molt_sys_flags_payload", globals())
)
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


class UnraisableHookArgs:
    __slots__ = ("exc_type", "exc_value", "exc_traceback", "err_msg", "object")

    def __init__(
        self,
        exc_type: object,
        exc_value: object,
        exc_traceback: object,
        err_msg: object,
        object: object,  # noqa: A002
    ) -> None:
        self.exc_type = exc_type
        self.exc_value = exc_value
        self.exc_traceback = exc_traceback
        self.err_msg = err_msg
        self.object = object


def _expect_int(value: object, intrinsic_name: str, field: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise RuntimeError(f"{intrinsic_name} returned invalid value for {field}")
    return value


def _expect_version_info_tuple(
    value: object, intrinsic_name: str, field: str
) -> tuple[int, int, int, str, int]:
    if not isinstance(value, (list, tuple)) or len(value) != 5:
        raise RuntimeError(f"{intrinsic_name} returned invalid value for {field}")
    major = _expect_int(value[0], intrinsic_name, f"{field}[0]")
    minor = _expect_int(value[1], intrinsic_name, f"{field}[1]")
    micro = _expect_int(value[2], intrinsic_name, f"{field}[2]")
    releaselevel_obj = value[3]
    if not _MOLT_IS_STRING_OBJ(releaselevel_obj):
        raise RuntimeError(f"{intrinsic_name} returned invalid value for {field}[3]")
    serial = _expect_int(value[4], intrinsic_name, f"{field}[4]")
    return major, minor, micro, cast(str, releaselevel_obj), serial


class _ImplementationNamespace:
    __slots__ = ("name", "cache_tag", "version", "hexversion")

    def __init__(
        self,
        name: str,
        cache_tag: str,
        version: tuple[int, int, int, str, int],
        hexversion: int,
    ) -> None:
        self.name = name
        self.cache_tag = cache_tag
        self.version = version
        self.hexversion = hexversion

    def __repr__(self) -> str:
        return (
            "namespace("
            f"name={self.name!r}, "
            f"cache_tag={self.cache_tag!r}, "
            f"version={self.version!r}, "
            f"hexversion={self.hexversion!r})"
        )


def _resolve_implementation(payload: object) -> _ImplementationNamespace:
    intrinsic_name = "molt_sys_implementation_payload"
    if not isinstance(payload, dict):
        raise RuntimeError(f"{intrinsic_name} returned invalid value")
    name_obj = payload.get("name")
    cache_tag_obj = payload.get("cache_tag")
    version_obj = payload.get("version")
    hexversion_obj = payload.get("hexversion")
    if not _MOLT_IS_STRING_OBJ(name_obj):
        raise RuntimeError(f"{intrinsic_name} returned invalid value for name")
    if not _MOLT_IS_STRING_OBJ(cache_tag_obj):
        raise RuntimeError(f"{intrinsic_name} returned invalid value for cache_tag")
    name = cast(str, name_obj)
    cache_tag = cast(str, cache_tag_obj)
    if not name:
        raise RuntimeError(f"{intrinsic_name} returned invalid value for name")
    if not cache_tag:
        raise RuntimeError(f"{intrinsic_name} returned invalid value for cache_tag")
    version = _expect_version_info_tuple(version_obj, intrinsic_name, "version")
    hexversion = _expect_int(hexversion_obj, intrinsic_name, "hexversion")
    return _ImplementationNamespace(name, cache_tag, version, hexversion)


_SYS_FLAGS_SEQUENCE_FIELDS = (
    "debug",
    "inspect",
    "interactive",
    "optimize",
    "dont_write_bytecode",
    "no_user_site",
    "no_site",
    "ignore_environment",
    "verbose",
    "bytes_warning",
    "quiet",
    "hash_randomization",
    "isolated",
    "dev_mode",
    "utf8_mode",
    "warn_default_encoding",
    "safe_path",
    "int_max_str_digits",
)
_SYS_FLAGS_SEQUENCE_INDEX = {
    name: index for index, name in enumerate(_SYS_FLAGS_SEQUENCE_FIELDS)
}
_SYS_FLAGS_GIL = 1


def _resolve_flags_payload(payload: object) -> tuple[tuple[int, ...], int]:
    intrinsic_name = "molt_sys_flags_payload"
    if not isinstance(payload, dict):
        raise RuntimeError(f"{intrinsic_name} returned invalid value")
    values: list[int] = []
    for field in _SYS_FLAGS_SEQUENCE_FIELDS:
        values.append(_expect_int(payload.get(field), intrinsic_name, field))
    gil = _expect_int(payload.get("gil"), intrinsic_name, "gil")
    return tuple(values), gil


class flags(tuple):
    __slots__ = ()
    n_fields = len(_SYS_FLAGS_SEQUENCE_FIELDS)
    n_sequence_fields = len(_SYS_FLAGS_SEQUENCE_FIELDS)
    n_unnamed_fields = 0

    def __new__(cls, values: tuple[int, ...]) -> "flags":
        if len(values) != len(_SYS_FLAGS_SEQUENCE_FIELDS):
            raise RuntimeError("molt_sys_flags_payload returned invalid value")
        return tuple.__new__(cls, values)

    def __getattr__(self, name: str) -> object:
        index = _SYS_FLAGS_SEQUENCE_INDEX.get(name)
        if index is not None:
            return self[index]
        if name == "gil":
            return _SYS_FLAGS_GIL
        raise AttributeError(name)

    def __repr__(self) -> str:
        items = ", ".join(
            f"{field}={self[index]!r}"
            for index, field in enumerate(_SYS_FLAGS_SEQUENCE_FIELDS)
        )
        return f"sys.flags({items})"


platform = _resolve_platform()
version_obj = _MOLT_SYS_VERSION()
if not _MOLT_IS_STRING_OBJ(version_obj):
    raise RuntimeError("molt_sys_version returned invalid value")
version = cast(str, version_obj)
version_info = _expect_version_info_tuple(
    _MOLT_SYS_VERSION_INFO(), "molt_sys_version_info", "version_info"
)
hexversion = _expect_int(_MOLT_SYS_HEXVERSION(), "molt_sys_hexversion", "hexversion")
api_version = _expect_int(
    _MOLT_SYS_API_VERSION(), "molt_sys_api_version", "api_version"
)
abiflags_obj = _MOLT_SYS_ABIFLAGS()
if not _MOLT_IS_STRING_OBJ(abiflags_obj):
    raise RuntimeError("molt_sys_abiflags returned invalid value")
abiflags = cast(str, abiflags_obj)
implementation = _resolve_implementation(_MOLT_SYS_IMPLEMENTATION_PAYLOAD())
if implementation.hexversion != hexversion:
    raise RuntimeError(
        "molt_sys_implementation_payload returned invalid value for hexversion"
    )
if implementation.version != version_info:
    raise RuntimeError(
        "molt_sys_implementation_payload returned invalid value for version"
    )
_flags_sequence, _SYS_FLAGS_GIL = _resolve_flags_payload(_MOLT_SYS_FLAGS_PAYLOAD())
_flags_type = flags
flags = _flags_type(_flags_sequence)
del _flags_type
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


def _resolve_stdio_handle(intrinsic: object, name: str) -> object:
    resolved = intrinsic
    if isinstance(resolved, str):
        resolved = _require_intrinsic(resolved)
    if not callable(resolved):
        raise RuntimeError(f"sys {name} intrinsic unavailable")
    handle = resolved()
    if handle is None:
        raise RuntimeError(f"sys {name} intrinsic returned invalid value")
    return handle


stdin = _resolve_stdio_handle(_MOLT_SYS_STDIN, "stdin")
stdout = _resolve_stdio_handle(_MOLT_SYS_STDOUT, "stdout")
stderr = _resolve_stdio_handle(_MOLT_SYS_STDERR, "stderr")
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
