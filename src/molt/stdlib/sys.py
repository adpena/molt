"""Minimal sys shim for Molt."""

from __future__ import annotations

try:
    from _intrinsics import require_intrinsic as _require_intrinsic
except (ImportError, TypeError, RuntimeError):
    # _intrinsics may not be available during early bootstrap or WASM builds.
    def _require_intrinsic(name):  # type: ignore[misc]
        raise RuntimeError(f"intrinsic {name!r} not available (bootstrap)")


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


def _return_empty_list() -> list[object]:
    return []


def _return_empty_tuple() -> tuple[object, ...]:
    return ()


def _return_empty_str() -> str:
    return ""


def _return_utf8() -> str:
    return "utf-8"


def _return_surrogateescape() -> str:
    return "surrogateescape"


def _return_false() -> bool:
    return False


def _return_zero() -> int:
    return 0


def _return_1000() -> int:
    return 1000


def _return_4300() -> int:
    return 4300


def _return_switchinterval() -> float:
    return 0.005


def _return_asyncgen_hooks_default() -> tuple[None, None]:
    return (None, None)


def _return_version_info_default() -> tuple[int, int, int, str, int]:
    return (3, 12, 0, "final", 0)


def _return_hexversion_default() -> int:
    return 0x030C00F0


def _return_platform_unknown() -> str:
    return "unknown"


def _return_maxsize_default() -> int:
    return 2**63 - 1


def _return_maxunicode_default() -> int:
    return 0x10FFFF


def _return_little_endian() -> str:
    return "little"


def _return_libdir_default() -> str:
    return "lib"


def _return_empty_frozenset() -> frozenset[object]:
    return frozenset()


def _return_refcount_default(_obj: object) -> int:
    return 1


def _return_identity(value: object) -> object:
    return value


def _return_getsizeof_default(_obj: object, _default: object = None) -> int:
    return 0


def _safe_intrinsic(
    name: str,
    default: object = None,
    _ri: object = _require_intrinsic,
) -> Callable[..., object]:
    """Resolve an intrinsic, returning *default* (or _noop) on failure.

    This NEVER raises during import, making bootstrap infallible on all
    targets including WASM where the registry may be populated lazily.
    The _ri default captures the resolver at definition time, avoiding
    a module-global lookup that can fail in AOT-compiled stdlib modules.
    """
    try:
        fn = _ri(name)
        if callable(fn):
            return fn  # type: ignore[return-value]
    except (RuntimeError, TypeError):
        pass
    if default is not None:
        return default  # type: ignore[return-value]
    return lambda *_a, **_k: None  # inline fallback


class _LazyIntrinsic:
    __slots__ = ("_name", "_default", "_resolver", "_cached")

    def __init__(
        self,
        name: str,
        default: object = None,
        resolver: object = _require_intrinsic,
    ) -> None:
        self._name = name
        self._default = default
        self._resolver = resolver
        self._cached = None

    def __call__(self, *args: object, **kwargs: object) -> object:
        cached = self._cached
        if cached is None:
            cached = _safe_intrinsic(self._name, self._default, self._resolver)
            self._cached = cached
        return cached(*args, **kwargs)


def _lazy_intrinsic(
    name: str,
    default: object = None,
    _ri: object = _require_intrinsic,
) -> Callable[..., object]:
    """Return a compact thunk that resolves *name* on first invocation."""

    return _LazyIntrinsic(name, default, _ri)


def _noop_getframe(_depth: int = 0) -> object:
    """Fallback for when molt_getframe intrinsic is unavailable (e.g. WASM/node)."""
    return None


def _noop_is_string_obj(val: object) -> bool:
    """Fallback for when molt_is_string_obj intrinsic is unavailable (e.g. WASM/node)."""
    return isinstance(val, str)


# Define early to avoid circular-import NameError during stdlib bootstrap.
# _safe_intrinsic never raises — WASM builds won't crash when the lazy
# resolver hasn't wired these yet.
_MOLT_GETFRAME = _lazy_intrinsic("molt_getframe", _noop_getframe)
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

_MOLT_GETARGV = _lazy_intrinsic("molt_getargv", _return_empty_list)
_MOLT_SYS_EXECUTABLE = _lazy_intrinsic("molt_sys_executable", _return_empty_str)
_MOLT_GETRECURSIONLIMIT = _lazy_intrinsic("molt_getrecursionlimit", _return_1000)
_MOLT_SETRECURSIONLIMIT = _lazy_intrinsic("molt_setrecursionlimit", None)
_MOLT_EXCEPTION_ACTIVE = _lazy_intrinsic("molt_exception_active", None)
_MOLT_EXCEPTION_LAST = _lazy_intrinsic("molt_exception_last", None)
_MOLT_ASYNCGEN_HOOKS_GET = _lazy_intrinsic(
    "molt_asyncgen_hooks_get", _return_asyncgen_hooks_default
)
_MOLT_ASYNCGEN_HOOKS_SET = _lazy_intrinsic("molt_asyncgen_hooks_set", None)
_MOLT_SYS_VERSION_INFO = _lazy_intrinsic(
    "molt_sys_version_info", _return_version_info_default
)
_MOLT_SYS_VERSION = _lazy_intrinsic("molt_sys_version", _return_empty_str)
_MOLT_SYS_HEXVERSION = _lazy_intrinsic("molt_sys_hexversion", _return_hexversion_default)
_MOLT_SYS_API_VERSION = _lazy_intrinsic("molt_sys_api_version", _return_zero)
_MOLT_SYS_ABIFLAGS = _lazy_intrinsic("molt_sys_abiflags", _return_empty_str)
_MOLT_SYS_IMPLEMENTATION_PAYLOAD = _lazy_intrinsic("molt_sys_implementation_payload", None)
_MOLT_SYS_FLAGS_PAYLOAD = _lazy_intrinsic("molt_sys_flags_payload", None)
_MOLT_SYS_PLATFORM = _lazy_intrinsic("molt_sys_platform", _return_platform_unknown)
_MOLT_SYS_IS_FINALIZING = _lazy_intrinsic("molt_sys_is_finalizing", _return_false)
_MOLT_SYS_GETREFCOUNT = _lazy_intrinsic("molt_sys_getrefcount", _return_refcount_default)
_MOLT_SYS_SETTRACE = _lazy_intrinsic("molt_sys_settrace", None)
_MOLT_SYS_GETTRACE = _lazy_intrinsic("molt_sys_gettrace", None)
_MOLT_SYS_SETPROFILE = _lazy_intrinsic("molt_sys_setprofile", None)
_MOLT_SYS_GETPROFILE = _lazy_intrinsic("molt_sys_getprofile", None)
_MOLT_SYS_STDIN = _lazy_intrinsic("molt_sys_stdin", None)
_MOLT_SYS_STDOUT = _lazy_intrinsic("molt_sys_stdout", None)
_MOLT_SYS_STDERR = _lazy_intrinsic("molt_sys_stderr", None)
_MOLT_SYS_GETFILESYSTEMENCODEERRORS = _lazy_intrinsic(
    "molt_sys_getfilesystemencodeerrors", _return_surrogateescape
)
_MOLT_SYS_BOOTSTRAP_PAYLOAD = _lazy_intrinsic("molt_sys_bootstrap_payload", None)
_MOLT_SYS_MAXSIZE = _lazy_intrinsic("molt_sys_maxsize", _return_maxsize_default)
_MOLT_SYS_MAXUNICODE = _lazy_intrinsic("molt_sys_maxunicode", _return_maxunicode_default)
_MOLT_SYS_BYTEORDER = _lazy_intrinsic("molt_sys_byteorder", _return_little_endian)
_MOLT_SYS_PREFIX = _lazy_intrinsic("molt_sys_prefix", _return_empty_str)
_MOLT_SYS_EXEC_PREFIX = _lazy_intrinsic("molt_sys_exec_prefix", _return_empty_str)
_MOLT_SYS_BASE_PREFIX = _lazy_intrinsic("molt_sys_base_prefix", _return_empty_str)
_MOLT_SYS_BASE_EXEC_PREFIX = _lazy_intrinsic(
    "molt_sys_base_exec_prefix", _return_empty_str
)
_MOLT_SYS_PLATLIBDIR = _lazy_intrinsic("molt_sys_platlibdir", _return_libdir_default)
_MOLT_SYS_FLOAT_INFO = _lazy_intrinsic("molt_sys_float_info", None)
_MOLT_SYS_INT_INFO = _lazy_intrinsic("molt_sys_int_info", None)
_MOLT_SYS_HASH_INFO = _lazy_intrinsic("molt_sys_hash_info", None)
_MOLT_SYS_THREAD_INFO = _lazy_intrinsic("molt_sys_thread_info", None)
_MOLT_SYS_INTERN = _lazy_intrinsic("molt_sys_intern", _return_identity)
_MOLT_SYS_GETSIZEOF = _lazy_intrinsic(
    "molt_sys_getsizeof", _return_getsizeof_default
)
_MOLT_SYS_STDLIB_MODULE_NAMES = _lazy_intrinsic(
    "molt_sys_stdlib_module_names", _return_empty_frozenset
)
_MOLT_SYS_BUILTIN_MODULE_NAMES = _lazy_intrinsic(
    "molt_sys_builtin_module_names", _return_empty_tuple
)
_MOLT_SYS_ORIG_ARGV = _lazy_intrinsic("molt_sys_orig_argv", _return_empty_list)
_MOLT_SYS_COPYRIGHT = _lazy_intrinsic("molt_sys_copyright", _return_empty_str)
_MOLT_TRACEBACK_FORMAT_EXCEPTION = _lazy_intrinsic("molt_traceback_format_exception", None)
_MOLT_SYS_GETDEFAULTENCODING = _lazy_intrinsic(
    "molt_sys_getdefaultencoding", _return_utf8
)
_MOLT_SYS_GETFILESYSTEMENCODING = _lazy_intrinsic(
    "molt_sys_getfilesystemencoding", _return_utf8
)
_MOLT_SYS_GETSWITCHINTERVAL = _lazy_intrinsic(
    "molt_sys_getswitchinterval", _return_switchinterval
)
_MOLT_SYS_SETSWITCHINTERVAL = _lazy_intrinsic("molt_sys_setswitchinterval", None)
_MOLT_SYS_GET_INT_MAX_STR_DIGITS = _lazy_intrinsic(
    "molt_sys_get_int_max_str_digits", _return_4300
)
_MOLT_SYS_SET_INT_MAX_STR_DIGITS = _lazy_intrinsic("molt_sys_set_int_max_str_digits", None)
_MOLT_SYS_CALL_TRACING_VALIDATE = _lazy_intrinsic("molt_sys_call_tracing_validate", None)
_MOLT_SYS_ADDAUDITHOOK = _lazy_intrinsic("molt_sys_addaudithook", None)
_MOLT_SYS_AUDIT_HOOK_COUNT = _lazy_intrinsic("molt_sys_audit_hook_count", _return_zero)
_MOLT_SYS_AUDIT_GET_HOOKS = _lazy_intrinsic("molt_sys_audit_get_hooks", _return_empty_list)
_MOLT_SYS_EXIT = _lazy_intrinsic("molt_sys_exit", None)
_MOLT_SYS_DISPLAYHOOK_WRITE = _lazy_intrinsic("molt_sys_displayhook_write", None)
_MOLT_SYS_EXCEPTHOOK_WRITE = _lazy_intrinsic("molt_sys_excepthook_write", None)
_MOLT_SYS_ARGV_NEW = _lazy_intrinsic("molt_sys_argv", None)
_MOLT_SYS_MODULES_NEW = _lazy_intrinsic("molt_sys_modules", None)
_MOLT_SYS_PATH_NEW = _lazy_intrinsic("molt_sys_path", None)
_MOLT_OS_WRITE_FN = _lazy_intrinsic("molt_os_write", _noop)

# Import-time bootstrap should avoid routing through the generic lazy wrapper.
# Resolve the small set of startup intrinsics once up front and call those
# directly so `import sys` stays on the shortest possible path.
_BOOT_GETARGV = _safe_intrinsic("molt_getargv", _return_empty_list)
_BOOT_SYS_PLATFORM = _safe_intrinsic("molt_sys_platform", _return_platform_unknown)
_BOOT_SYS_BOOTSTRAP_PAYLOAD = _safe_intrinsic("molt_sys_bootstrap_payload", _noop)
_BOOT_SYS_STDIN = _safe_intrinsic("molt_sys_stdin", _noop)
_BOOT_SYS_STDOUT = _safe_intrinsic("molt_sys_stdout", _noop)
_BOOT_SYS_STDERR = _safe_intrinsic("molt_sys_stderr", _noop)

# Use the safe intrinsic resolved above — _safe_intrinsic never raises,
# so this works on both native and WASM without needing a direct builtins
# import (which fails when sys is imported transitively at runtime).
raw_argv = _BOOT_GETARGV()
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
_SYS_FLAGS_SEQUENCE_INDEX = {}
for _i__SYS_FLAGS_SEQUENCE_INDEX in range(len(_SYS_FLAGS_SEQUENCE_FIELDS)):
    _SYS_FLAGS_SEQUENCE_INDEX[_SYS_FLAGS_SEQUENCE_FIELDS[_i__SYS_FLAGS_SEQUENCE_INDEX]] = _i__SYS_FLAGS_SEQUENCE_INDEX
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
_VERSION_INFO_INDEX = {}
for _idx__VERSION_INFO_INDEX in range(len(_VERSION_INFO_FIELDS)):
    _VERSION_INFO_INDEX[_VERSION_INFO_FIELDS[_idx__VERSION_INFO_INDEX]] = _idx__VERSION_INFO_INDEX


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
_FLOAT_INFO_INDEX = {}
for _idx__FLOAT_INFO_INDEX in range(len(_FLOAT_INFO_FIELDS)):
    _FLOAT_INFO_INDEX[_FLOAT_INFO_FIELDS[_idx__FLOAT_INFO_INDEX]] = _idx__FLOAT_INFO_INDEX


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
_INT_INFO_INDEX = {}
for _idx__INT_INFO_INDEX in range(len(_INT_INFO_FIELDS)):
    _INT_INFO_INDEX[_INT_INFO_FIELDS[_idx__INT_INFO_INDEX]] = _idx__INT_INFO_INDEX


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
_HASH_INFO_INDEX = {}
for _idx__HASH_INFO_INDEX in range(len(_HASH_INFO_FIELDS)):
    _HASH_INFO_INDEX[_HASH_INFO_FIELDS[_idx__HASH_INFO_INDEX]] = _idx__HASH_INFO_INDEX


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
_THREAD_INFO_INDEX = {}
for _idx__THREAD_INFO_INDEX in range(len(_THREAD_INFO_FIELDS)):
    _THREAD_INFO_INDEX[_THREAD_INFO_FIELDS[_idx__THREAD_INFO_INDEX]] = _idx__THREAD_INFO_INDEX


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


_FlagsTuple = flags
_VersionInfoTuple = version_info
_FloatInfoTuple = float_info
_IntInfoTuple = int_info
_HashInfoTuple = hash_info
_ThreadInfoTuple = thread_info


_platform_val = _BOOT_SYS_PLATFORM()
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
    global _SYS_FLAGS_GIL
    g = globals()
    # Use runtime intrinsics when available; fall back to compile-time defaults.
    version_text = _MOLT_SYS_VERSION() or "3.12.0 (molt)"
    version_values = _MOLT_SYS_VERSION_INFO() or (3, 12, 0, "final", 0)
    hexversion_value = _MOLT_SYS_HEXVERSION() or 0x030C00F0
    api_version_value = 0
    abiflags_value = ""
    implementation_value = _ImplementationNamespace(
        "molt", "molt-312", version_values, hexversion_value
    )
    _SYS_FLAGS_GIL = 1
    flags_values = (
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        1,
        0,
        0,
        0,
        0,
        0,
        4300,
    )
    float_info_values = tuple(
        (
            1.7976931348623157e+308,
            1024,
            308,
            2.2250738585072014e-308,
            -1021,
            -307,
            15,
            53,
            2.220446049250313e-16,
            2,
            1,
        )
    )
    int_info_values = tuple(
        (30, 4, 4300, 640)
    )
    hash_info_values = tuple(
        (
            64,
            2305843009213693951,
            314159,
            0,
            1000003,
            "siphash13",
            64,
            128,
            0,
        )
    )
    thread_info_values = tuple(
        ("pthread", None, None)
    )

    g["version"] = version_text
    g["_raw_version_info"] = version_values
    g["version_info"] = _VersionInfoTuple(version_values)
    g["hexversion"] = hexversion_value
    g["api_version"] = api_version_value
    g["abiflags"] = abiflags_value
    g["implementation"] = implementation_value
    g["flags"] = _FlagsTuple(flags_values)
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
    g["float_info"] = _FloatInfoTuple(float_info_values)
    g["int_info"] = _IntInfoTuple(int_info_values)
    g["hash_info"] = _HashInfoTuple(hash_info_values)
    g["thread_info"] = _ThreadInfoTuple(thread_info_values)
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
_bootstrap_payload_value = _BOOT_SYS_BOOTSTRAP_PAYLOAD(_BOOTSTRAP_MODULE_FILE)
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

# Extract all payload fields immediately in a local function scope.
# This avoids a native-backend use-after-free where the dict reference
# held in the module dict can get corrupted across chunk boundaries
# when many function calls intervene between dict creation and use.
def _extract_bootstrap_payload(payload: dict) -> tuple:
    """Extract all bootstrap payload fields in one scope."""
    _path_val = payload.get("path") if isinstance(payload, dict) else []
    _pp_val = payload.get("pythonpath_entries") if isinstance(payload, dict) else []
    _mr_val = payload.get("module_roots_entries") if isinstance(payload, dict) else []
    _vsp_val = payload.get("venv_site_packages_entries") if isinstance(payload, dict) else []
    _pwd_val = payload.get("pwd") if isinstance(payload, dict) else "/"
    _icwd_val = payload.get("include_cwd") if isinstance(payload, dict) else False
    _sr_val = payload.get("stdlib_root") if isinstance(payload, dict) else None

    def _to_str_list(val: object) -> list:
        if not isinstance(val, (list, tuple)):
            return []
        out: list = []
        for entry in val:
            if isinstance(entry, str):
                out.append(entry)
        return out

    path_list = _to_str_list(_path_val)
    pp_list = _to_str_list(_pp_val)
    mr_list = _to_str_list(_mr_val)
    vsp_list = _to_str_list(_vsp_val)
    pwd_str = str(_pwd_val) if isinstance(_pwd_val, str) else "/"
    icwd_bool = bool(_icwd_val) if _icwd_val is not None else False
    sr_str = str(_sr_val) if isinstance(_sr_val, str) else None
    return (path_list, pp_list, mr_list, vsp_list, pwd_str, icwd_bool, sr_str)

_bp_result = _extract_bootstrap_payload(_bootstrap_payload_value)
_bp_path_list = _bp_result[0]
_bp_pythonpath_list = _bp_result[1]
_bp_module_roots_list = _bp_result[2]
_bp_vsp_list = _bp_result[3]
_bp_pwd_str = _bp_result[4]
_bp_include_cwd_bool = _bp_result[5]
_bp_stdlib_root_str = _bp_result[6]

path = list(_bp_path_list)
_molt_bootstrap_pythonpath = tuple(_bp_pythonpath_list)
_molt_bootstrap_module_roots = tuple(_bp_module_roots_list)
_molt_bootstrap_venv_site_packages = tuple(_bp_vsp_list)
_molt_bootstrap_pwd = _bp_pwd_str
_molt_bootstrap_include_cwd_val = _bp_include_cwd_bool
_molt_bootstrap_include_cwd = bool(_molt_bootstrap_include_cwd_val) if _molt_bootstrap_include_cwd_val is not None else False
_molt_bootstrap_stdlib_root = _bp_stdlib_root_str


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
# If an intrinsic returns None (e.g. during early bootstrap before the runtime
# registers stdio handles), fall back to a minimal file-like wrapper around
# the raw C file descriptors so that print() and sys.stdout.write() still work.
stdin = _BOOT_SYS_STDIN()
stdout = _BOOT_SYS_STDOUT()
stderr = _BOOT_SYS_STDERR()

if stdout is None or stderr is None or stdin is None:
    class _StdioFallback:
        """Minimal file-like object wrapping a raw fd for bootstrap.

        Used only when the molt_sys_std{in,out,err} intrinsics return None
        during early bootstrap (before the runtime registers stdio handles).
        The runtime will normally overwrite these with proper TextIOWrapper
        objects in sys_populate_stdio, so this is a last-resort fallback.
        """
        def __init__(self, _name: str, _fd: int, _writable: bool = True) -> None:
            self.name = _name
            self._fd = _fd
            self.mode = "w" if _writable else "r"
            self.encoding = "utf-8"
            self.errors = "surrogateescape"
            self.closed = False
            self._writable = _writable
        def write(self, s: object) -> int:
            text = str(s) if not isinstance(s, str) else s
            if not text:
                return 0
            data = text.encode(self.encoding, self.errors)
            try:
                _MOLT_OS_WRITE_FN(self._fd, data)
            except Exception:
                pass
            return len(text)
        def read(self, n: int = -1) -> str:
            return ""
        def readline(self, limit: int = -1) -> str:
            return ""
        def flush(self) -> None:
            pass
        def fileno(self) -> int:
            return self._fd
        def isatty(self) -> bool:
            return False
        def readable(self) -> bool:
            return not self._writable
        def writable(self) -> bool:
            return self._writable
        def seekable(self) -> bool:
            return False
        def close(self) -> None:
            self.closed = True
        def __enter__(self) -> "_StdioFallback":
            return self
        def __exit__(self, *_args: object) -> None:
            pass

    if stdin is None:
        stdin = _StdioFallback("<stdin>", 0, False)
    if stdout is None:
        stdout = _StdioFallback("<stdout>", 1, True)
    if stderr is None:
        stderr = _StdioFallback("<stderr>", 2, True)

__stdin__ = stdin
__stdout__ = stdout
__stderr__ = stderr
_default_encoding = "utf-8"
_fs_encoding = "utf-8"
_fs_encode_errors = "surrogateescape"


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
