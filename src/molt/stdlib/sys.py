"""Minimal sys shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


def cast(_tp, value):  # type: ignore[override]
    return value


# Ensure sys.modules exists early to avoid circular import failures.
_existing_modules = globals().get("modules")
if _existing_modules is None:
    # Try the new intrinsic first; fall back to a plain dict.
    try:
        _modules_intrinsic = _require_intrinsic("molt_sys_modules")
    except RuntimeError:
        _modules_intrinsic = None
    if callable(_modules_intrinsic):
        _new_modules = _modules_intrinsic()
        if isinstance(_new_modules, dict):
            modules: dict[str, object] = _new_modules
        else:
            modules = {}
    else:
        modules = {}
else:
    modules = _existing_modules
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


def _noop(*_args: object, **_kwargs: object) -> None:
    """Universal no-op fallback for unavailable intrinsics."""
    return None


def _safe_intrinsic(name: str, default: object = None) -> Callable[..., object]:
    """Resolve an intrinsic, returning *default* (or _noop) on failure.

    This NEVER raises during import, making bootstrap infallible on all
    targets including WASM where the registry may be populated lazily.
    """
    try:
        fn = _require_intrinsic(name)
        if callable(fn):
            return fn  # type: ignore[return-value]
    except RuntimeError:
        pass
    if default is not None:
        return default  # type: ignore[return-value]
    return _noop


def _noop_getframe(_depth: int = 0) -> object:
    """Fallback for when molt_getframe intrinsic is unavailable (e.g. WASM/node)."""
    return None


def _noop_is_string_obj(val: object) -> bool:
    """Fallback for when molt_is_string_obj intrinsic is unavailable (e.g. WASM/node)."""
    return isinstance(val, str)


# Define early to avoid circular-import NameError during stdlib bootstrap.
# _safe_intrinsic never raises — WASM builds won't crash when the lazy
# resolver hasn't wired these yet.
_MOLT_GETFRAME = _safe_intrinsic("molt_getframe", _noop_getframe)
# Always use isinstance-based check: the molt_is_string_obj intrinsic
# fails on WASM when the string was allocated by the compiler (its header
# type_id can diverge from TYPE_ID_STRING for interned/constant strings).
_MOLT_IS_STRING_OBJ = _noop_is_string_obj

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
    "get_asyncgen_hooks",
    "set_asyncgen_hooks",
    "exit",
    "maxsize",
    "maxunicode",
    "byteorder",
    "prefix",
    "exec_prefix",
    "base_prefix",
    "base_exec_prefix",
    "platlibdir",
    "float_info",
    "int_info",
    "hash_info",
    "thread_info",
    "intern",
    "getsizeof",
    "stdlib_module_names",
    "builtin_module_names",
    "orig_argv",
    "copyright",
    "displayhook",
    "__displayhook__",
    "excepthook",
    "__excepthook__",
    "unraisablehook",
    "__unraisablehook__",
    "dont_write_bytecode",
    "float_repr_style",
    "pycache_prefix",
    "warnoptions",
    "_xoptions",
    "get_int_max_str_digits",
    "set_int_max_str_digits",
    "is_finalizing",
    "getrefcount",
    "getswitchinterval",
    "setswitchinterval",
    "settrace",
    "gettrace",
    "setprofile",
    "getprofile",
    "call_tracing",
    "exception",
    "addaudithook",
    "audit",
]

_MOLT_GETARGV = _safe_intrinsic("molt_getargv", lambda: [])
_MOLT_SYS_EXECUTABLE = _safe_intrinsic("molt_sys_executable", lambda: "")
_MOLT_GETRECURSIONLIMIT = _safe_intrinsic("molt_getrecursionlimit", lambda: 1000)
_MOLT_SETRECURSIONLIMIT = _safe_intrinsic("molt_setrecursionlimit")
_MOLT_EXCEPTION_ACTIVE = _safe_intrinsic("molt_exception_active")
_MOLT_EXCEPTION_LAST = _safe_intrinsic("molt_exception_last")
_MOLT_ASYNCGEN_HOOKS_GET = _safe_intrinsic("molt_asyncgen_hooks_get", lambda: (None, None))
_MOLT_ASYNCGEN_HOOKS_SET = _safe_intrinsic("molt_asyncgen_hooks_set")
_MOLT_SYS_VERSION_INFO = _safe_intrinsic("molt_sys_version_info", lambda: (3, 12, 0, "final", 0))
_MOLT_SYS_VERSION = _safe_intrinsic("molt_sys_version", lambda: "3.12.0 (molt)")
_MOLT_SYS_HEXVERSION = _safe_intrinsic("molt_sys_hexversion", lambda: 0x030C00F0)
_MOLT_SYS_API_VERSION = _safe_intrinsic("molt_sys_api_version", lambda: 0)
_MOLT_SYS_ABIFLAGS = _safe_intrinsic("molt_sys_abiflags", lambda: "")
_MOLT_SYS_IMPLEMENTATION_PAYLOAD = _safe_intrinsic("molt_sys_implementation_payload")
_MOLT_SYS_FLAGS_PAYLOAD = _safe_intrinsic("molt_sys_flags_payload")
_MOLT_SYS_PLATFORM = _safe_intrinsic("molt_sys_platform", lambda: "wasm32")
_MOLT_SYS_IS_FINALIZING = _safe_intrinsic("molt_sys_is_finalizing", lambda: False)
_MOLT_SYS_GETREFCOUNT = _safe_intrinsic("molt_sys_getrefcount", lambda _obj: 1)
_MOLT_SYS_SETTRACE = _safe_intrinsic("molt_sys_settrace")
_MOLT_SYS_GETTRACE = _safe_intrinsic("molt_sys_gettrace")
_MOLT_SYS_SETPROFILE = _safe_intrinsic("molt_sys_setprofile")
_MOLT_SYS_GETPROFILE = _safe_intrinsic("molt_sys_getprofile")
_MOLT_SYS_STDIN = _safe_intrinsic("molt_sys_stdin")
_MOLT_SYS_STDOUT = _safe_intrinsic("molt_sys_stdout")
_MOLT_SYS_STDERR = _safe_intrinsic("molt_sys_stderr")
_MOLT_SYS_GETFILESYSTEMENCODEERRORS = _safe_intrinsic(
    "molt_sys_getfilesystemencodeerrors", lambda: "surrogateescape"
)
_MOLT_SYS_BOOTSTRAP_PAYLOAD = _safe_intrinsic("molt_sys_bootstrap_payload")
_MOLT_SYS_MAXSIZE = _safe_intrinsic("molt_sys_maxsize", lambda: 2**63 - 1)
_MOLT_SYS_MAXUNICODE = _safe_intrinsic("molt_sys_maxunicode", lambda: 0x10FFFF)
_MOLT_SYS_BYTEORDER = _safe_intrinsic("molt_sys_byteorder", lambda: "little")
_MOLT_SYS_PREFIX = _safe_intrinsic("molt_sys_prefix", lambda: "")
_MOLT_SYS_EXEC_PREFIX = _safe_intrinsic("molt_sys_exec_prefix", lambda: "")
_MOLT_SYS_BASE_PREFIX = _safe_intrinsic("molt_sys_base_prefix", lambda: "")
_MOLT_SYS_BASE_EXEC_PREFIX = _safe_intrinsic("molt_sys_base_exec_prefix", lambda: "")
_MOLT_SYS_PLATLIBDIR = _safe_intrinsic("molt_sys_platlibdir", lambda: "lib")
_MOLT_SYS_FLOAT_INFO = _safe_intrinsic("molt_sys_float_info")
_MOLT_SYS_INT_INFO = _safe_intrinsic("molt_sys_int_info")
_MOLT_SYS_HASH_INFO = _safe_intrinsic("molt_sys_hash_info")
_MOLT_SYS_THREAD_INFO = _safe_intrinsic("molt_sys_thread_info")
_MOLT_SYS_INTERN = _safe_intrinsic("molt_sys_intern", lambda s: s)
_MOLT_SYS_GETSIZEOF = _safe_intrinsic("molt_sys_getsizeof", lambda obj, _default=None: 0)
_MOLT_SYS_STDLIB_MODULE_NAMES = _safe_intrinsic("molt_sys_stdlib_module_names", lambda: frozenset())
_MOLT_SYS_BUILTIN_MODULE_NAMES = _safe_intrinsic("molt_sys_builtin_module_names", lambda: ())
_MOLT_SYS_ORIG_ARGV = _safe_intrinsic("molt_sys_orig_argv", lambda: [])
_MOLT_SYS_COPYRIGHT = _safe_intrinsic("molt_sys_copyright", lambda: "")
_MOLT_TRACEBACK_FORMAT_EXCEPTION = _safe_intrinsic("molt_traceback_format_exception")
_MOLT_SYS_GETDEFAULTENCODING = _safe_intrinsic("molt_sys_getdefaultencoding", lambda: "utf-8")
_MOLT_SYS_GETFILESYSTEMENCODING = _safe_intrinsic("molt_sys_getfilesystemencoding", lambda: "utf-8")
_MOLT_SYS_GETSWITCHINTERVAL = _safe_intrinsic("molt_sys_getswitchinterval", lambda: 0.005)
_MOLT_SYS_SETSWITCHINTERVAL = _safe_intrinsic("molt_sys_setswitchinterval")
_MOLT_SYS_GET_INT_MAX_STR_DIGITS = _safe_intrinsic("molt_sys_get_int_max_str_digits", lambda: 4300)
_MOLT_SYS_SET_INT_MAX_STR_DIGITS = _safe_intrinsic("molt_sys_set_int_max_str_digits")
_MOLT_SYS_CALL_TRACING_VALIDATE = _safe_intrinsic("molt_sys_call_tracing_validate")
_MOLT_SYS_ADDAUDITHOOK = _safe_intrinsic("molt_sys_addaudithook")
_MOLT_SYS_AUDIT_HOOK_COUNT = _safe_intrinsic("molt_sys_audit_hook_count", lambda: 0)
_MOLT_SYS_AUDIT_GET_HOOKS = _safe_intrinsic("molt_sys_audit_get_hooks", lambda: [])
_MOLT_SYS_EXIT = _safe_intrinsic("molt_sys_exit")
_MOLT_SYS_DISPLAYHOOK_WRITE = _safe_intrinsic("molt_sys_displayhook_write")
_MOLT_SYS_EXCEPTHOOK_WRITE = _safe_intrinsic("molt_sys_excepthook_write")
_MOLT_SYS_ARGV_NEW = _safe_intrinsic("molt_sys_argv")
_MOLT_SYS_MODULES_NEW = _safe_intrinsic("molt_sys_modules")
_MOLT_SYS_PATH_NEW = _safe_intrinsic("molt_sys_path")

# Use the safe intrinsic resolved above — _safe_intrinsic never raises,
# so this works on both native and WASM without needing a direct builtins
# import (which fails when sys is imported transitively at runtime).
raw_argv = _MOLT_GETARGV()
if isinstance(raw_argv, (list, tuple)):
    argv = list(raw_argv)
else:
    argv = []

executable = ""  # Not available on WASM


def _resolve_platform(_getter: object = _MOLT_SYS_PLATFORM) -> str:
    if not callable(_getter):
        raise RuntimeError("molt_sys_platform intrinsic unavailable")
    value = _getter()
    if not isinstance(value, str):
        raise RuntimeError("molt_sys_platform returned invalid value")
    return value


def exit(code: object = None) -> None:
    _MOLT_SYS_EXIT(code)
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
    if not isinstance(releaselevel_obj, str):
        raise RuntimeError(f"{intrinsic_name} returned invalid value for {field}[3]")
    serial = _expect_int(value[4], intrinsic_name, f"{field}[4]")
    return major, minor, micro, (releaselevel_obj), serial


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
    if not isinstance(name_obj, str):
        raise RuntimeError(f"{intrinsic_name} returned invalid value for name")
    if not isinstance(cache_tag_obj, str):
        raise RuntimeError(f"{intrinsic_name} returned invalid value for cache_tag")
    name = (name_obj)
    cache_tag = (cache_tag_obj)
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
    name: int(index) for index, name in enumerate(_SYS_FLAGS_SEQUENCE_FIELDS)
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


# --- version_info structured tuple ---

_VERSION_INFO_FIELDS = ("major", "minor", "micro", "releaselevel", "serial")
_VERSION_INFO_INDEX = {name: int(i) for i, name in enumerate(_VERSION_INFO_FIELDS)}


class version_info(tuple):
    __slots__ = ()
    n_fields = len(_VERSION_INFO_FIELDS)
    n_sequence_fields = len(_VERSION_INFO_FIELDS)
    n_unnamed_fields = 0

    def __new__(cls, values: object) -> "version_info":
        return tuple.__new__(cls, values)

    def __getattr__(self, name: str) -> object:
        index = _VERSION_INFO_INDEX.get(name)
        if index is not None:
            return self[index]
        raise AttributeError(name)

    def __repr__(self) -> str:
        items = ", ".join(
            f"{field}={self[index]!r}"
            for index, field in enumerate(_VERSION_INFO_FIELDS)
        )
        return f"sys.version_info({items})"


# --- float_info structured tuple ---

_FLOAT_INFO_FIELDS = (
    "max",
    "max_exp",
    "max_10_exp",
    "min",
    "min_exp",
    "min_10_exp",
    "dig",
    "mant_dig",
    "epsilon",
    "radix",
    "rounds",
)
_FLOAT_INFO_INDEX = {name: int(i) for i, name in enumerate(_FLOAT_INFO_FIELDS)}


class float_info(tuple):
    __slots__ = ()
    n_fields = len(_FLOAT_INFO_FIELDS)
    n_sequence_fields = len(_FLOAT_INFO_FIELDS)
    n_unnamed_fields = 0

    def __new__(cls, values: object) -> "float_info":
        return tuple.__new__(cls, values)

    def __getattr__(self, name: str) -> object:
        index = _FLOAT_INFO_INDEX.get(name)
        if index is not None:
            return self[index]
        raise AttributeError(name)

    def __repr__(self) -> str:
        items = ", ".join(f"{f}={self[i]!r}" for i, f in enumerate(_FLOAT_INFO_FIELDS))
        return f"sys.float_info({items})"


# --- int_info structured tuple ---

_INT_INFO_FIELDS = (
    "bits_per_digit",
    "sizeof_digit",
    "default_max_str_digits",
    "str_digits_check_threshold",
)
_INT_INFO_INDEX = {name: int(i) for i, name in enumerate(_INT_INFO_FIELDS)}


class int_info(tuple):
    __slots__ = ()
    n_fields = len(_INT_INFO_FIELDS)
    n_sequence_fields = len(_INT_INFO_FIELDS)
    n_unnamed_fields = 0

    def __new__(cls, values: object) -> "int_info":
        return tuple.__new__(cls, values)

    def __getattr__(self, name: str) -> object:
        index = _INT_INFO_INDEX.get(name)
        if index is not None:
            return self[index]
        raise AttributeError(name)

    def __repr__(self) -> str:
        items = ", ".join(f"{f}={self[i]!r}" for i, f in enumerate(_INT_INFO_FIELDS))
        return f"sys.int_info({items})"


# --- hash_info structured tuple ---

_HASH_INFO_FIELDS = (
    "width",
    "modulus",
    "inf",
    "nan",
    "imag",
    "algorithm",
    "hash_bits",
    "seed_bits",
    "cutoff",
)
_HASH_INFO_INDEX = {name: int(i) for i, name in enumerate(_HASH_INFO_FIELDS)}


class hash_info(tuple):
    __slots__ = ()
    n_fields = len(_HASH_INFO_FIELDS)
    n_sequence_fields = len(_HASH_INFO_FIELDS)
    n_unnamed_fields = 0

    def __new__(cls, values: object) -> "hash_info":
        return tuple.__new__(cls, values)

    def __getattr__(self, name: str) -> object:
        index = _HASH_INFO_INDEX.get(name)
        if index is not None:
            return self[index]
        raise AttributeError(name)

    def __repr__(self) -> str:
        items = ", ".join(f"{f}={self[i]!r}" for i, f in enumerate(_HASH_INFO_FIELDS))
        return f"sys.hash_info({items})"


# --- thread_info structured tuple ---

_THREAD_INFO_FIELDS = (
    "name",
    "lock",
    "version",
)
_THREAD_INFO_INDEX = {name: int(i) for i, name in enumerate(_THREAD_INFO_FIELDS)}


class thread_info(tuple):
    __slots__ = ()
    n_fields = len(_THREAD_INFO_FIELDS)
    n_sequence_fields = len(_THREAD_INFO_FIELDS)
    n_unnamed_fields = 0

    def __new__(cls, values: object) -> "thread_info":
        return tuple.__new__(cls, values)

    def __getattr__(self, name: str) -> object:
        index = _THREAD_INFO_INDEX.get(name)
        if index is not None:
            return self[index]
        raise AttributeError(name)

    def __repr__(self) -> str:
        items = ", ".join(f"{f}={self[i]!r}" for i, f in enumerate(_THREAD_INFO_FIELDS))
        return f"sys.thread_info({items})"


_platform_val = _MOLT_SYS_PLATFORM()
platform = _platform_val if isinstance(_platform_val, str) else "wasm32"


def _try_str_intrinsic(fn: object, fallback: str) -> str:
    """Call *fn*() and return its value when it is a str, else *fallback*."""
    try:
        val = fn()  # type: ignore[operator]
        if isinstance(val, str):
            return val
    except Exception:
        pass
    return fallback


def _try_int_intrinsic(fn: object, fallback: int) -> int:
    """Call *fn*() and return its value when it is an int, else *fallback*."""
    try:
        val = fn()  # type: ignore[operator]
        if isinstance(val, int):
            return val
    except Exception:
        pass
    return fallback


def _try_tuple_intrinsic(
    fn: object, fallback: tuple[object, ...], expected_len: int = 0
) -> tuple[object, ...] | list[object]:
    """Call *fn*() and return when it is a tuple/list, else *fallback*."""
    try:
        val = fn()  # type: ignore[operator]
        if isinstance(val, (list, tuple)):
            if expected_len == 0 or len(val) == expected_len:
                return val
    except Exception:
        pass
    return fallback


# On WASM, intrinsics that return heap-allocated objects (tuples, dicts)
# Split metadata init into a helper to reduce molt_init_sys function size.
# Cranelift generates incorrect code for functions >200KB of machine code.
def _init_metadata():
    """Initialize version/platform metadata as module globals."""
    g = globals()
    g["version"] = "3.12.0 (molt)"
    _rvi = (3, 12, 0, "final", 0)
    g["_raw_version_info"] = _rvi
    g["version_info"] = _rvi
    g["hexversion"] = 0x030C00F0
    g["api_version"] = 0
    g["abiflags"] = ""
    g["implementation"] = _ImplementationNamespace("molt", "molt-312", _rvi, 0x030C00F0)
    g["flags"] = (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 4300)
    g["path"] = []
    g["meta_path"] = []
    g["path_hooks"] = []
    g["path_importer_cache"] = {}
    g["maxsize"] = 2**63 - 1
    g["maxunicode"] = 0x10FFFF
    g["byteorder"] = "little"
    g["prefix"] = ""
    g["exec_prefix"] = ""
    g["base_prefix"] = ""
    g["base_exec_prefix"] = ""
    g["platlibdir"] = "lib"
    g["float_info"] = (1.7976931348623157e+308, 1024, 308, 2.2250738585072014e-308, -1021, -307, 15, 53, 2.220446049250313e-16, 2, 1)
    g["int_info"] = (30, 4, 4300, 640)
    g["hash_info"] = (64, 2305843009213693951, 314159, 0, 1000003, "siphash13", 64, 128, 0)
    g["thread_info"] = ("pthread", None, None)
    g["orig_argv"] = []
    g["copyright"] = "Copyright (c) Molt contributors."
    g["stdlib_module_names"] = frozenset()
    g["builtin_module_names"] = ()

_init_metadata()


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
        if not isinstance(entry, str):
            entry_type = type(entry).__name__
            raise RuntimeError(
                f"{intrinsic_name} returned invalid value (expected str entry, got {entry_type})"
            )
        out.append(entry)  # type: ignore[arg-type]
    return out


def _bootstrap_str(payload: dict[object, object], key: str, intrinsic_name: str) -> str:
    value = payload.get(key)
    if not isinstance(value, str):
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
    if not isinstance(value, str):
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
    # Fallback: provide minimal bootstrap payload for WASM environments
    _bootstrap_payload_value = {
        "path": [],
        "pythonpath_entries": [],
        "module_roots_entries": [],
        "venv_site_packages_entries": [],
        "pwd": "/",
        "include_cwd": False,
        "stdlib_root": None,
    }

# Prefer the new consolidated path intrinsic when available.
if callable(_MOLT_SYS_PATH_NEW):
    _sys_path_raw = _MOLT_SYS_PATH_NEW()
    if isinstance(_sys_path_raw, (list, tuple)):
        path = list(_sys_path_raw)
    else:
        path = _bootstrap_str_list(
            _bootstrap_payload_value, "path", "molt_sys_bootstrap_payload"
        )
else:
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
_molt_bootstrap_include_cwd_val = _bootstrap_payload_value.get("include_cwd")
_molt_bootstrap_include_cwd = bool(_molt_bootstrap_include_cwd_val) if _molt_bootstrap_include_cwd_val is not None else False
_molt_bootstrap_stdlib_root = _bootstrap_str_or_none(
    _bootstrap_payload_value, "stdlib_root", "molt_sys_bootstrap_payload"
)


def _resolve_stdio_handle(intrinsic: object, name: str) -> object:
    resolved = intrinsic
    if isinstance(resolved, str):
        resolved = _safe_intrinsic(resolved)
    if not callable(resolved):
        raise RuntimeError(f"sys {name} intrinsic unavailable")
    handle = resolved()
    if handle is None:
        raise RuntimeError(f"sys {name} intrinsic returned invalid value")
    return handle


# Use the safe intrinsics resolved earlier — avoids direct builtins imports
# which fail when sys is imported transitively at runtime.
stdin = _MOLT_SYS_STDIN()
stdout = _MOLT_SYS_STDOUT()
stderr = _MOLT_SYS_STDERR()
__stdin__ = stdin
__stdout__ = stdout
__stderr__ = stderr
_default_encoding = "utf-8"
_fs_encoding = "utf-8"
_fs_encode_errors_val = _MOLT_SYS_GETFILESYSTEMENCODEERRORS()
_fs_encode_errors = _fs_encode_errors_val if isinstance(_fs_encode_errors_val, str) else "surrogateescape"


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
    return int((_MOLT_GETRECURSIONLIMIT()))


def setrecursionlimit(limit: int) -> None:
    _MOLT_SETRECURSIONLIMIT(limit)
    return None


def exc_info() -> tuple[object, object, object]:
    # CPython 3.12: only return the exception currently being handled
    # in an active except block, never a stale exception from a previous block.
    exc = _MOLT_EXCEPTION_ACTIVE()
    if exc is None:
        return None, None, None
    return type(exc), exc, getattr(exc, "__traceback__", None)


def _getframe(depth: int = 0) -> object | None:
    return _MOLT_GETFRAME(depth + 2)


def getdefaultencoding() -> str:
    return (_MOLT_SYS_GETDEFAULTENCODING())


def getfilesystemencoding() -> str:
    return (_MOLT_SYS_GETFILESYSTEMENCODING())


def getfilesystemencodeerrors() -> str:
    return _fs_encode_errors


def get_asyncgen_hooks() -> object:
    hooks = _MOLT_ASYNCGEN_HOOKS_GET()
    if not isinstance(hooks, tuple) or len(hooks) != 2:
        raise RuntimeError("asyncgen hooks intrinsic returned invalid value")
    firstiter, finalizer = hooks
    return _asyncgen_hooks(firstiter, finalizer)


def set_asyncgen_hooks(
    *, firstiter: object | None = None, finalizer: object | None = None
) -> None:
    _MOLT_ASYNCGEN_HOOKS_SET(firstiter, finalizer)
    return None


def intern(s: object) -> str:
    if not isinstance(s, str):
        raise TypeError(f"intern() argument 1 must be str, not {type(s).__name__}")
    return (_MOLT_SYS_INTERN(s))


def getsizeof(obj: object, default: object = ...) -> int:
    if default is ...:
        return (_MOLT_SYS_GETSIZEOF(obj, None))
    return (_MOLT_SYS_GETSIZEOF(obj, default))


def displayhook(value: object) -> None:
    if value is None:
        return
    _builtins = modules.get("builtins")
    text = repr(value)
    _MOLT_SYS_DISPLAYHOOK_WRITE(text)
    _MOLT_SYS_DISPLAYHOOK_WRITE("\n")
    if _builtins is not None:
        _builtins._ = value  # type: ignore[attr-defined]


def excepthook(exc_type: object, exc_value: object, exc_tb: object) -> None:
    try:
        lines = _MOLT_TRACEBACK_FORMAT_EXCEPTION(
            exc_type, exc_value, exc_tb, None, True
        )
    except BaseException:  # noqa: BLE001
        lines = None
    if isinstance(lines, list) and all(isinstance(line, str) for line in lines):
        _MOLT_SYS_EXCEPTHOOK_WRITE("".join(lines))
        return

    type_name = getattr(exc_type, "__name__", None)
    if not isinstance(type_name, str):
        type_name = str(exc_type)
    detail = str(exc_value) if exc_value is not None else ""
    if detail:
        _MOLT_SYS_EXCEPTHOOK_WRITE(f"{type_name}: {detail}\n")
        return
    _MOLT_SYS_EXCEPTHOOK_WRITE(f"{type_name}\n")


def unraisablehook(unraisable: object) -> None:
    """Handle an unraisable exception (e.g. from __del__ or gc).

    CPython 3.8+ (PEP 578).  The *unraisable* argument should be an
    ``UnraisableHookArgs`` instance.
    """
    err_msg = getattr(unraisable, "err_msg", None)
    obj = getattr(unraisable, "object", None)
    exc_value = getattr(unraisable, "exc_value", None)
    exc_type = getattr(unraisable, "exc_type", None)
    exc_tb = getattr(unraisable, "exc_traceback", None)

    if err_msg is None:
        err_msg = "Exception ignored in"
    if obj is not None:
        _MOLT_SYS_EXCEPTHOOK_WRITE(f"{err_msg}: {obj!r}\n")
    else:
        _MOLT_SYS_EXCEPTHOOK_WRITE(f"{err_msg}\n")

    if exc_value is not None:
        try:
            lines = _MOLT_TRACEBACK_FORMAT_EXCEPTION(
                exc_type, exc_value, exc_tb, None, True
            )
        except BaseException:  # noqa: BLE001
            lines = None
        if isinstance(lines, list) and all(isinstance(line, str) for line in lines):
            _MOLT_SYS_EXCEPTHOOK_WRITE("".join(lines))
            return
        type_name = getattr(exc_type, "__name__", None)
        if not isinstance(type_name, str):
            type_name = str(exc_type)
        detail = str(exc_value) if exc_value is not None else ""
        if detail:
            _MOLT_SYS_EXCEPTHOOK_WRITE(f"{type_name}: {detail}\n")
        else:
            _MOLT_SYS_EXCEPTHOOK_WRITE(f"{type_name}\n")


# --- Save original hooks (CPython 3.12 parity) ---
__displayhook__ = displayhook
__excepthook__ = excepthook
__unraisablehook__ = unraisablehook


# --- Compile-time constants (no intrinsic needed) ---

# Molt never writes .pyc files; compiled binaries are self-contained.
dont_write_bytecode = True

# Python >= 3.1 always uses short repr for floats.
float_repr_style = "short"

# Molt has no bytecode cache; always None.
pycache_prefix = None

# Implementation detail of the warnings framework; always empty for Molt.
warnoptions: list[str] = []

# CPython -X options dict; Molt has none.
_xoptions: dict[str, object] = {}


# --- Integer string conversion length limitation (CPython 3.11+) ---

def get_int_max_str_digits() -> int:
    """Return the current integer string conversion length limit."""
    return int((_MOLT_SYS_GET_INT_MAX_STR_DIGITS()))


def set_int_max_str_digits(maxdigits: int) -> None:
    """Set the integer string conversion length limit.

    *maxdigits* must be 0 (unlimited) or >= str_digits_check_threshold.
    """
    _MOLT_SYS_SET_INT_MAX_STR_DIGITS(maxdigits)


# --- Interpreter state queries ---


def is_finalizing() -> bool:
    """Return True if the runtime is finalizing."""
    value = _MOLT_SYS_IS_FINALIZING()
    if not isinstance(value, bool):
        raise RuntimeError("molt_sys_is_finalizing returned invalid value")
    return value


def getrefcount(obj: object) -> int:
    """Return the runtime reference count of *obj* (best effort)."""
    value = _MOLT_SYS_GETREFCOUNT(obj)
    if not isinstance(value, int) or isinstance(value, bool):
        raise RuntimeError("molt_sys_getrefcount returned invalid value")
    return value


# --- Thread switch interval (CPython GIL timeslice) ---

def getswitchinterval() -> float:
    """Return the interpreter's thread switch interval in seconds."""
    return float((_MOLT_SYS_GETSWITCHINTERVAL()))


def setswitchinterval(interval: float) -> None:
    """Set the interpreter's thread switch interval (in seconds).

    Molt does not have a GIL; this is a compatibility stub that stores
    the value for callers that read it back.
    """
    _MOLT_SYS_SETSWITCHINTERVAL(interval)


def settrace(tracefunc: object) -> None:
    """Set the system trace function."""
    _MOLT_SYS_SETTRACE(tracefunc)


def gettrace() -> object:
    """Get the trace function as set by settrace()."""
    return _MOLT_SYS_GETTRACE()


def setprofile(profilefunc: object) -> None:
    """Set the system profile function."""
    _MOLT_SYS_SETPROFILE(profilefunc)


def getprofile() -> object:
    """Get the profiler function as set by setprofile()."""
    return _MOLT_SYS_GETPROFILE()


def call_tracing(func: object, args: object) -> object:
    """Call func(*args) while tracing is enabled.

    Molt does not support tracing; this simply calls func(*args).
    """
    _MOLT_SYS_CALL_TRACING_VALIDATE(func, args)
    return func(*args)  # type: ignore[operator]


# --- Active exception accessor (CPython 3.11+) ---


def exception() -> BaseException | None:
    """Return the active exception instance being handled, or None."""
    exc = _MOLT_EXCEPTION_ACTIVE()
    if exc is None:
        exc = _MOLT_EXCEPTION_LAST()
    if isinstance(exc, BaseException):
        return exc
    return None


# --- Audit hooks (CPython 3.8+, PEP 578) ---

def addaudithook(hook: object) -> None:
    """Append the callable *hook* to the list of active auditing hooks.

    Molt does not raise audit events internally; hooks are stored for
    compatibility with libraries that register them.
    """
    _MOLT_SYS_ADDAUDITHOOK(hook)


def audit(event: str, *args: object) -> None:
    """Raise an auditing event and trigger any active auditing hooks.

    Molt invokes registered hooks with (event, args) for compatibility,
    but the runtime does not raise its own internal audit events.
    """
    count = _MOLT_SYS_AUDIT_HOOK_COUNT()
    if not count:
        return
    hooks = _MOLT_SYS_AUDIT_GET_HOOKS()
    if isinstance(hooks, list):
        for hook in hooks:
            if callable(hook):
                hook(event, args)


# ---------------------------------------------------------------------------
# Namespace cleanup — remove names that are not part of CPython's sys public API.
# These are needed for type annotations, casting helpers, and intermediate
# variables but must not appear in the module __dict__ as non-underscore
# public names.
# ---------------------------------------------------------------------------
# Keep asyncgen_hooks reachable for get_asyncgen_hooks() via a private alias.
_asyncgen_hooks = asyncgen_hooks
for _name in (
    "TYPE_CHECKING",
    "cast",
    "Callable",
    "Iterable",
    "raw_argv",
    "version_obj",
    "abiflags_obj",
    "asyncgen_hooks",
):
    globals().pop(_name, None)
