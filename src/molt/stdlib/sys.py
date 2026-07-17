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


def _return_empty_list() -> list[object]:
    return []


def _return_empty_tuple() -> tuple[object, ...]:
    return ()


def _return_empty_str() -> str:
    return ""


def _return_utf8() -> str:
    return "utf-8"


def _return_false() -> bool:
    return False


def _return_zero() -> int:
    return 0


def _return_zero_for_sizeof(_obj: object, _default: object = None) -> int:
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


def _return_version_default() -> str:
    return "3.12.0 (molt)"


def _return_hexversion_default() -> int:
    return 0x030C00F0


def _return_platform_unknown() -> str:
    return "unknown"


def _return_refcount_default(_obj: object) -> int:
    return 1


def _return_identity(value: object) -> object:
    return value


def _return_maxsize_default() -> int:
    return 2**63 - 1


def _return_maxunicode_default() -> int:
    return 0x10FFFF


def _return_little_endian() -> str:
    return "little"


def _return_empty_frozenset() -> frozenset[object]:
    return frozenset()


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
    return _noop


def _noop_getframe(_depth: int = 0) -> object:
    """Fallback for when molt_getframe intrinsic is unavailable (e.g. WASM/node)."""
    return None


def _noop_is_string_obj(val: object) -> bool:
    """Fallback for when molt_is_string_obj intrinsic is unavailable (e.g. WASM/node)."""
    return isinstance(val, str)


def _platform_default() -> str:
    try:
        value = _MOLT_SYS_PLATFORM()
        if isinstance(value, str):
            return value
    except Exception:
        pass
    return _return_platform_unknown()


def _platform_is_windows() -> bool:
    return _platform_default().startswith("win")


def _filesystem_encode_errors_default() -> str:
    if _platform_is_windows():
        return "surrogatepass"
    return "surrogateescape"


def _platlibdir_default() -> str:
    if _platform_is_windows():
        return "DLLs"
    return "lib"


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

_MOLT_GETRECURSIONLIMIT = _safe_intrinsic("molt_getrecursionlimit", _return_1000)
_MOLT_SETRECURSIONLIMIT = _safe_intrinsic("molt_setrecursionlimit", None)
_MOLT_EXCEPTION_ACTIVE = _safe_intrinsic("molt_exception_active", None)
_MOLT_EXCEPTION_LAST = _safe_intrinsic("molt_exception_last", None)
_MOLT_UNRAISABLE_HOOK_ARGS_IS_EXACT = _safe_intrinsic(
    "molt_unraisable_hook_args_is_exact", _return_false
)
_MOLT_ASYNCGEN_HOOKS_GET = _safe_intrinsic(
    "molt_asyncgen_hooks_get", _return_asyncgen_hooks_default
)
_MOLT_ASYNCGEN_HOOKS_SET = _safe_intrinsic("molt_asyncgen_hooks_set", None)
_MOLT_SYS_VERSION_INFO = _safe_intrinsic(
    "molt_sys_version_info", _return_version_info_default
)
_MOLT_SYS_VERSION = _safe_intrinsic("molt_sys_version", _return_version_default)
_MOLT_SYS_HEXVERSION = _safe_intrinsic("molt_sys_hexversion", _return_hexversion_default)
_MOLT_SYS_API_VERSION = _safe_intrinsic("molt_sys_api_version", _return_zero)
_MOLT_SYS_ABIFLAGS = _safe_intrinsic("molt_sys_abiflags", _return_empty_str)
_MOLT_SYS_IMPLEMENTATION_PAYLOAD = _safe_intrinsic(
    "molt_sys_implementation_payload", None
)
_MOLT_SYS_FLAGS_PAYLOAD = _safe_intrinsic("molt_sys_flags_payload", None)
_MOLT_SYS_PLATFORM = _safe_intrinsic("molt_sys_platform", _return_platform_unknown)
_MOLT_SYS_IS_FINALIZING = _safe_intrinsic("molt_sys_is_finalizing", _return_false)
_MOLT_SYS_GETREFCOUNT = _safe_intrinsic("molt_sys_getrefcount", _return_refcount_default)
_MOLT_SYS_SETTRACE = _safe_intrinsic("molt_sys_settrace", None)
_MOLT_SYS_GETTRACE = _safe_intrinsic("molt_sys_gettrace", None)
_MOLT_SYS_SETPROFILE = _safe_intrinsic("molt_sys_setprofile", None)
_MOLT_SYS_GETPROFILE = _safe_intrinsic("molt_sys_getprofile", None)
_MOLT_SYS_STDIN = _safe_intrinsic("molt_sys_stdin", None)
_MOLT_SYS_STDOUT = _safe_intrinsic("molt_sys_stdout", None)
_MOLT_SYS_STDERR = _safe_intrinsic("molt_sys_stderr", None)
_MOLT_SYS_GETFILESYSTEMENCODEERRORS = _safe_intrinsic(
    "molt_sys_getfilesystemencodeerrors", _filesystem_encode_errors_default
)
_MOLT_SYS_MAXSIZE = _safe_intrinsic("molt_sys_maxsize", _return_maxsize_default)
_MOLT_SYS_MAXUNICODE = _safe_intrinsic("molt_sys_maxunicode", _return_maxunicode_default)
_MOLT_SYS_BYTEORDER = _safe_intrinsic("molt_sys_byteorder", _return_little_endian)
_MOLT_SYS_PREFIX = _safe_intrinsic("molt_sys_prefix", _return_empty_str)
_MOLT_SYS_EXEC_PREFIX = _safe_intrinsic("molt_sys_exec_prefix", _return_empty_str)
_MOLT_SYS_BASE_PREFIX = _safe_intrinsic("molt_sys_base_prefix", _return_empty_str)
_MOLT_SYS_BASE_EXEC_PREFIX = _safe_intrinsic("molt_sys_base_exec_prefix", _return_empty_str)
_MOLT_SYS_PLATLIBDIR = _safe_intrinsic("molt_sys_platlibdir", _platlibdir_default)
_MOLT_SYS_FLOAT_INFO = _safe_intrinsic("molt_sys_float_info", None)
_MOLT_SYS_INT_INFO = _safe_intrinsic("molt_sys_int_info", None)
_MOLT_SYS_HASH_INFO = _safe_intrinsic("molt_sys_hash_info", None)
_MOLT_SYS_THREAD_INFO = _safe_intrinsic("molt_sys_thread_info", None)
_MOLT_SYS_INTERN = _safe_intrinsic("molt_sys_intern", _return_identity)
_MOLT_SYS_GETSIZEOF = _safe_intrinsic(
    "molt_sys_getsizeof", _return_zero_for_sizeof
)
_MOLT_SYS_STDLIB_MODULE_NAMES = _safe_intrinsic(
    "molt_sys_stdlib_module_names", _return_empty_frozenset
)
_MOLT_SYS_BUILTIN_MODULE_NAMES = _safe_intrinsic(
    "molt_sys_builtin_module_names", _return_empty_tuple
)
_MOLT_SYS_ORIG_ARGV = _safe_intrinsic("molt_sys_orig_argv", _return_empty_list)
_MOLT_SYS_COPYRIGHT = _safe_intrinsic("molt_sys_copyright", _return_empty_str)
_MOLT_TRACEBACK_FORMAT_EXCEPTION = _safe_intrinsic(
    "molt_traceback_format_exception", None
)
_MOLT_SYS_GETDEFAULTENCODING = _safe_intrinsic(
    "molt_sys_getdefaultencoding", _return_utf8
)
_MOLT_SYS_GETFILESYSTEMENCODING = _safe_intrinsic(
    "molt_sys_getfilesystemencoding", _return_utf8
)
_MOLT_SYS_GETSWITCHINTERVAL = _safe_intrinsic(
    "molt_sys_getswitchinterval", _return_switchinterval
)
_MOLT_SYS_SETSWITCHINTERVAL = _safe_intrinsic("molt_sys_setswitchinterval", None)
_MOLT_SYS_GET_INT_MAX_STR_DIGITS = _safe_intrinsic(
    "molt_sys_get_int_max_str_digits", _return_4300
)
_MOLT_SYS_SET_INT_MAX_STR_DIGITS = _safe_intrinsic(
    "molt_sys_set_int_max_str_digits", None
)
_MOLT_SYS_CALL_TRACING_VALIDATE = _safe_intrinsic(
    "molt_sys_call_tracing_validate", None
)
_MOLT_SYS_ADDAUDITHOOK = _safe_intrinsic("molt_sys_addaudithook", None)
_MOLT_SYS_AUDIT_HOOK_COUNT = _safe_intrinsic("molt_sys_audit_hook_count", _return_zero)
_MOLT_SYS_AUDIT_GET_HOOKS = _safe_intrinsic("molt_sys_audit_get_hooks", _return_empty_list)
_MOLT_SYS_EXIT = _safe_intrinsic("molt_sys_exit", None)
_MOLT_SYS_DISPLAYHOOK_WRITE = _safe_intrinsic("molt_sys_displayhook_write", None)
_MOLT_SYS_EXCEPTHOOK_WRITE = _safe_intrinsic("molt_sys_excepthook_write", None)
_MOLT_OS_WRITE_RESOLVED = False
try:
    _MOLT_OS_WRITE_FN = _require_intrinsic("molt_os_write")
    if callable(_MOLT_OS_WRITE_FN):
        _MOLT_OS_WRITE_RESOLVED = True
except (RuntimeError, TypeError):
    _MOLT_OS_WRITE_FN = None

# Runtime sys module publication is the sole argv/executable authority.  Python
# placeholders would execute after publication and could overwrite the process
# values, making behavior depend on module/link initialization order.


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


_ImplementationNamespaceType = None


def _implementation_namespace_type():
    global _ImplementationNamespaceType
    if _ImplementationNamespaceType is not None:
        return _ImplementationNamespaceType

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

    _ImplementationNamespaceType = _ImplementationNamespace
    return _ImplementationNamespaceType


def _resolve_implementation(payload: object) -> object:
    intrinsic_name = "molt_sys_implementation_payload"
    if isinstance(payload, dict):
        name_obj = payload.get("name")
        cache_tag_obj = payload.get("cache_tag")
        version_obj = payload.get("version")
        hexversion_obj = payload.get("hexversion")
    else:
        name_obj = getattr(payload, "name", None)
        cache_tag_obj = getattr(payload, "cache_tag", None)
        version_obj = getattr(payload, "version", None)
        hexversion_obj = getattr(payload, "hexversion", None)
    if not isinstance(name_obj, str):
        raise RuntimeError(f"{intrinsic_name} returned invalid value for name")
    if not isinstance(cache_tag_obj, str):
        raise RuntimeError(f"{intrinsic_name} returned invalid value for cache_tag")
    name = name_obj
    cache_tag = cache_tag_obj
    if not name:
        raise RuntimeError(f"{intrinsic_name} returned invalid value for name")
    if not cache_tag:
        raise RuntimeError(f"{intrinsic_name} returned invalid value for cache_tag")
    version = _expect_version_info_tuple(version_obj, intrinsic_name, "version")
    hexversion = _expect_int(hexversion_obj, intrinsic_name, "hexversion")
    return _implementation_namespace_type()(name, cache_tag, version, hexversion)


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
    _SYS_FLAGS_SEQUENCE_INDEX[
        _SYS_FLAGS_SEQUENCE_FIELDS[_i__SYS_FLAGS_SEQUENCE_INDEX]
    ] = _i__SYS_FLAGS_SEQUENCE_INDEX
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


_FlagsTuple = None


def _flags_tuple_type():
    global _FlagsTuple
    if _FlagsTuple is not None:
        return _FlagsTuple

    class _FlagsTupleType(tuple):
        __slots__ = ()
        n_fields = len(_SYS_FLAGS_SEQUENCE_FIELDS)
        n_sequence_fields = len(_SYS_FLAGS_SEQUENCE_FIELDS)
        n_unnamed_fields = 0

        def __new__(cls, values: tuple[int, ...]) -> "_FlagsTupleType":
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

    _FlagsTuple = _FlagsTupleType
    return _FlagsTuple


# --- version_info structured tuple ---

_VERSION_INFO_FIELDS = ("major", "minor", "micro", "releaselevel", "serial")
_VERSION_INFO_INDEX = {}
for _idx__VERSION_INFO_INDEX in range(len(_VERSION_INFO_FIELDS)):
    _VERSION_INFO_INDEX[_VERSION_INFO_FIELDS[_idx__VERSION_INFO_INDEX]] = (
        _idx__VERSION_INFO_INDEX
    )


_VersionInfoTuple = None


def _version_info_tuple_type():
    global _VersionInfoTuple
    if _VersionInfoTuple is not None:
        return _VersionInfoTuple

    class _VersionInfoTupleType(tuple):
        __slots__ = ()
        n_fields = len(_VERSION_INFO_FIELDS)
        n_sequence_fields = len(_VERSION_INFO_FIELDS)
        n_unnamed_fields = 0

        def __new__(cls, values: object) -> "_VersionInfoTupleType":
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

    _VersionInfoTuple = _VersionInfoTupleType
    return _VersionInfoTuple


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
    _FLOAT_INFO_INDEX[_FLOAT_INFO_FIELDS[_idx__FLOAT_INFO_INDEX]] = (
        _idx__FLOAT_INFO_INDEX
    )


_FloatInfoTuple = None


def _float_info_tuple_type():
    global _FloatInfoTuple
    if _FloatInfoTuple is not None:
        return _FloatInfoTuple

    class _FloatInfoTupleType(tuple):
        __slots__ = ()
        n_fields = len(_FLOAT_INFO_FIELDS)
        n_sequence_fields = len(_FLOAT_INFO_FIELDS)
        n_unnamed_fields = 0

        def __new__(cls, values: object) -> "_FloatInfoTupleType":
            return tuple.__new__(cls, values)

        def __getattr__(self, name: str) -> object:
            index = _FLOAT_INFO_INDEX.get(name)
            if index is not None:
                return self[index]
            raise AttributeError(name)

        def __repr__(self) -> str:
            items = ", ".join(
                f"{f}={self[i]!r}" for i, f in enumerate(_FLOAT_INFO_FIELDS)
            )
            return f"sys.float_info({items})"

    _FloatInfoTuple = _FloatInfoTupleType
    return _FloatInfoTuple


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


_IntInfoTuple = None


def _int_info_tuple_type():
    global _IntInfoTuple
    if _IntInfoTuple is not None:
        return _IntInfoTuple

    class _IntInfoTupleType(tuple):
        __slots__ = ()
        n_fields = len(_INT_INFO_FIELDS)
        n_sequence_fields = len(_INT_INFO_FIELDS)
        n_unnamed_fields = 0

        def __new__(cls, values: object) -> "_IntInfoTupleType":
            return tuple.__new__(cls, values)

        def __getattr__(self, name: str) -> object:
            index = _INT_INFO_INDEX.get(name)
            if index is not None:
                return self[index]
            raise AttributeError(name)

        def __repr__(self) -> str:
            items = ", ".join(
                f"{f}={self[i]!r}" for i, f in enumerate(_INT_INFO_FIELDS)
            )
            return f"sys.int_info({items})"

    _IntInfoTuple = _IntInfoTupleType
    return _IntInfoTuple


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


_HashInfoTuple = None


def _hash_info_tuple_type():
    global _HashInfoTuple
    if _HashInfoTuple is not None:
        return _HashInfoTuple

    class _HashInfoTupleType(tuple):
        __slots__ = ()
        n_fields = len(_HASH_INFO_FIELDS)
        n_sequence_fields = len(_HASH_INFO_FIELDS)
        n_unnamed_fields = 0

        def __new__(cls, values: object) -> "_HashInfoTupleType":
            return tuple.__new__(cls, values)

        def __getattr__(self, name: str) -> object:
            index = _HASH_INFO_INDEX.get(name)
            if index is not None:
                return self[index]
            raise AttributeError(name)

        def __repr__(self) -> str:
            items = ", ".join(
                f"{f}={self[i]!r}" for i, f in enumerate(_HASH_INFO_FIELDS)
            )
            return f"sys.hash_info({items})"

    _HashInfoTuple = _HashInfoTupleType
    return _HashInfoTuple


# --- thread_info structured tuple ---

_THREAD_INFO_FIELDS = (
    "name",
    "lock",
    "version",
)
_THREAD_INFO_INDEX = {}
for _idx__THREAD_INFO_INDEX in range(len(_THREAD_INFO_FIELDS)):
    _THREAD_INFO_INDEX[_THREAD_INFO_FIELDS[_idx__THREAD_INFO_INDEX]] = (
        _idx__THREAD_INFO_INDEX
    )


_ThreadInfoTuple = None


def _thread_info_tuple_type():
    global _ThreadInfoTuple
    if _ThreadInfoTuple is not None:
        return _ThreadInfoTuple

    class _ThreadInfoTupleType(tuple):
        __slots__ = ()
        n_fields = len(_THREAD_INFO_FIELDS)
        n_sequence_fields = len(_THREAD_INFO_FIELDS)
        n_unnamed_fields = 0

        def __new__(cls, values: object) -> "_ThreadInfoTupleType":
            return tuple.__new__(cls, values)

        def __getattr__(self, name: str) -> object:
            index = _THREAD_INFO_INDEX.get(name)
            if index is not None:
                return self[index]
            raise AttributeError(name)

        def __repr__(self) -> str:
            items = ", ".join(
                f"{f}={self[i]!r}" for i, f in enumerate(_THREAD_INFO_FIELDS)
            )
            return f"sys.thread_info({items})"

    _ThreadInfoTuple = _ThreadInfoTupleType
    return _ThreadInfoTuple


platform = _platform_default()


def _sys_abiflags_is_available(platform_name: str) -> bool:
    return not platform_name.startswith("win")


_SYS_ABIFLAGS_AVAILABLE = _sys_abiflags_is_available(platform)
if _SYS_ABIFLAGS_AVAILABLE:
    __all__.insert(__all__.index("flags"), "abiflags")


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
    version_text = _try_str_intrinsic(_MOLT_SYS_VERSION, "3.12.0 (molt)")
    raw_version_info = _try_tuple_intrinsic(
        _MOLT_SYS_VERSION_INFO, (3, 12, 0, "final", 0), expected_len=5
    )
    _rvi = tuple(raw_version_info)
    hexversion_value = _try_int_intrinsic(_MOLT_SYS_HEXVERSION, 0x030C00F0)
    api_version_value = _try_int_intrinsic(_MOLT_SYS_API_VERSION, 0)
    platform_value = _try_str_intrinsic(_MOLT_SYS_PLATFORM, _return_platform_unknown())
    abiflags_value = ""
    if _SYS_ABIFLAGS_AVAILABLE:
        abiflags_value = _try_str_intrinsic(_MOLT_SYS_ABIFLAGS, "")
    implementation_value = _implementation_namespace_type()(
        "molt", "molt-312", _rvi, hexversion_value
    )
    try:
        implementation_payload = _MOLT_SYS_IMPLEMENTATION_PAYLOAD()
    except Exception:
        implementation_payload = None
    if implementation_payload is not None:
        implementation_value = _resolve_implementation(implementation_payload)

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
    try:
        flags_payload = _MOLT_SYS_FLAGS_PAYLOAD()
    except Exception:
        flags_payload = None
    if flags_payload is not None:
        flags_values, _SYS_FLAGS_GIL = _resolve_flags_payload(flags_payload)

    float_info_values = tuple(
        _try_tuple_intrinsic(
            _MOLT_SYS_FLOAT_INFO,
            (
                1.7976931348623157e308,
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
            ),
            expected_len=len(_FLOAT_INFO_FIELDS),
        )
    )
    int_info_values = tuple(
        _try_tuple_intrinsic(
            _MOLT_SYS_INT_INFO,
            (30, 4, 4300, 640),
            expected_len=len(_INT_INFO_FIELDS),
        )
    )
    hash_info_values = tuple(
        _try_tuple_intrinsic(
            _MOLT_SYS_HASH_INFO,
            (64, 2305843009213693951, 314159, 0, 1000003, "siphash13", 64, 128, 0),
            expected_len=len(_HASH_INFO_FIELDS),
        )
    )
    thread_info_values = tuple(
        _try_tuple_intrinsic(
            _MOLT_SYS_THREAD_INFO,
            ("pthread", None, None),
            expected_len=len(_THREAD_INFO_FIELDS),
        )
    )
    platlibdir_value = _try_str_intrinsic(_MOLT_SYS_PLATLIBDIR, _platlibdir_default())

    g["version"] = version_text
    g["_raw_version_info"] = _rvi
    g["version_info"] = _version_info_tuple_type()(_rvi)
    g["hexversion"] = hexversion_value
    g["api_version"] = api_version_value
    g["platform"] = platform_value
    if _SYS_ABIFLAGS_AVAILABLE:
        g["abiflags"] = abiflags_value
    else:
        g.pop("abiflags", None)
    g["implementation"] = implementation_value
    g["flags"] = _flags_tuple_type()(flags_values)
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
    g["platlibdir"] = platlibdir_value
    g["float_info"] = _float_info_tuple_type()(float_info_values)
    g["int_info"] = _int_info_tuple_type()(int_info_values)
    g["hash_info"] = _hash_info_tuple_type()(hash_info_values)
    g["thread_info"] = _thread_info_tuple_type()(thread_info_values)
    g["orig_argv"] = []
    g["copyright"] = "Copyright (c) Molt contributors."
    g["stdlib_module_names"] = frozenset()
    g["builtin_module_names"] = ()


_metadata_names = [
        "version",
        "version_info",
        "hexversion",
        "api_version",
        "implementation",
        "flags",
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
        "orig_argv",
        "copyright",
        "stdlib_module_names",
        "builtin_module_names",
]
if _SYS_ABIFLAGS_AVAILABLE:
    _metadata_names.append("abiflags")
_METADATA_NAMES = frozenset(_metadata_names)
_metadata_initialized = False


def _ensure_metadata_initialized() -> None:
    global _metadata_initialized
    if _metadata_initialized:
        return
    _init_metadata()
    _metadata_initialized = True


path = []
_molt_bootstrap_pythonpath = ()
_molt_bootstrap_module_roots = ()
_molt_bootstrap_venv_site_packages = ()
_molt_bootstrap_pwd = "/"
_molt_bootstrap_include_cwd = False
_molt_bootstrap_stdlib_root = None
meta_path = []
path_hooks = []
path_importer_cache = {}


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


_STDIO_NAMES = frozenset(
    {"stdin", "stdout", "stderr", "__stdin__", "__stdout__", "__stderr__"}
)
_stdio_initialized = False


def _ensure_stdio_initialized() -> None:
    global _stdio_initialized
    if _stdio_initialized:
        return

    # Use the safe intrinsics resolved earlier — avoids direct builtins imports
    # which fail when sys is imported transitively at runtime.
    # If an intrinsic returns None (e.g. during early bootstrap before the runtime
    # registers stdio handles), fall back to a minimal file-like wrapper around
    # the raw C file descriptors so that print() and sys.stdout.write() still work.
    stdin = _MOLT_SYS_STDIN()
    stdout = _MOLT_SYS_STDOUT()
    stderr = _MOLT_SYS_STDERR()

    if stdout is None or stderr is None or stdin is None:

        class _StdioFallback:
            """Minimal file-like object wrapping a raw fd for bootstrap."""

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
                if _MOLT_OS_WRITE_RESOLVED and _MOLT_OS_WRITE_FN is not None:
                    _MOLT_OS_WRITE_FN(self._fd, data)
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

    g = globals()
    g["stdin"] = stdin
    g["stdout"] = stdout
    g["stderr"] = stderr
    g["__stdin__"] = stdin
    g["__stdout__"] = stdout
    g["__stderr__"] = stderr
    _stdio_initialized = True


def __getattr__(name: str) -> object:
    if name in _METADATA_NAMES:
        _ensure_metadata_initialized()
        return globals()[name]
    if name in _STDIO_NAMES:
        _ensure_stdio_initialized()
        return globals()[name]
    if name in _HEAVY_API_NAMES:
        _ensure_heavy_api_initialized()
        return globals()[name]
    raise AttributeError(name)


_default_encoding = "utf-8"
_fs_encoding = "utf-8"
_fs_encode_errors = None


def _filesystem_encode_errors() -> str:
    global _fs_encode_errors
    if isinstance(_fs_encode_errors, str):
        return _fs_encode_errors
    value = _MOLT_SYS_GETFILESYSTEMENCODEERRORS()
    _fs_encode_errors = (
        value if isinstance(value, str) else _filesystem_encode_errors_default()
    )
    return _fs_encode_errors


_AsyncgenHooksTuple = None


def _asyncgen_hooks_tuple_type():
    global _AsyncgenHooksTuple
    if _AsyncgenHooksTuple is not None:
        return _AsyncgenHooksTuple

    class _AsyncgenHooksTupleType(tuple):
        __slots__ = ()

        def __new__(
            cls, firstiter: object | None, finalizer: object | None
        ) -> "_AsyncgenHooksTupleType":
            return tuple.__new__(cls, (firstiter, finalizer))

        @property
        def firstiter(self) -> object | None:
            return self[0]

        @property
        def finalizer(self) -> object | None:
            return self[1]

    _AsyncgenHooksTuple = _AsyncgenHooksTupleType
    return _AsyncgenHooksTuple


_HEAVY_API_NAMES = frozenset(
    {
        "asyncgen_hooks",
        "getrecursionlimit",
        "setrecursionlimit",
        "exc_info",
        "_getframe",
        "getdefaultencoding",
        "getfilesystemencoding",
        "getfilesystemencodeerrors",
        "get_asyncgen_hooks",
        "set_asyncgen_hooks",
        "intern",
        "getsizeof",
        "displayhook",
        "__displayhook__",
        "excepthook",
        "__excepthook__",
        "unraisablehook",
        "__unraisablehook__",
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
    }
)
_heavy_api_initialized = False


def _ensure_heavy_api_initialized() -> None:
    global _heavy_api_initialized
    if _heavy_api_initialized:
        return

    AsyncgenHooksTuple = _asyncgen_hooks_tuple_type()

    def getrecursionlimit() -> int:
        return int((_MOLT_GETRECURSIONLIMIT()))

    def setrecursionlimit(limit: int) -> None:
        _MOLT_SETRECURSIONLIMIT(limit)
        return None

    def exc_info() -> tuple[object, object, object]:
        exc = _MOLT_EXCEPTION_ACTIVE()
        if exc is None:
            return None, None, None
        return type(exc), exc, getattr(exc, "__traceback__", None)

    def _getframe(depth: int = 0) -> object | None:
        return _MOLT_GETFRAME(depth + 2)

    def getdefaultencoding() -> str:
        return _MOLT_SYS_GETDEFAULTENCODING()

    def getfilesystemencoding() -> str:
        return _MOLT_SYS_GETFILESYSTEMENCODING()

    def getfilesystemencodeerrors() -> str:
        return _filesystem_encode_errors()

    def get_asyncgen_hooks() -> object:
        hooks = _MOLT_ASYNCGEN_HOOKS_GET()
        if not isinstance(hooks, tuple) or len(hooks) != 2:
            raise RuntimeError("asyncgen hooks intrinsic returned invalid value")
        firstiter, finalizer = hooks
        return AsyncgenHooksTuple(firstiter, finalizer)

    def set_asyncgen_hooks(
        *, firstiter: object | None = None, finalizer: object | None = None
    ) -> None:
        _MOLT_ASYNCGEN_HOOKS_SET(firstiter, finalizer)
        return None

    def intern(s: object) -> str:
        if not isinstance(s, str):
            raise TypeError(f"intern() argument 1 must be str, not {type(s).__name__}")
        return _MOLT_SYS_INTERN(s)

    def getsizeof(obj: object, default: object = ...) -> int:
        if default is ...:
            return _MOLT_SYS_GETSIZEOF(obj, None)
        return _MOLT_SYS_GETSIZEOF(obj, default)

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
        if not _MOLT_UNRAISABLE_HOOK_ARGS_IS_EXACT(unraisable):
            raise TypeError(
                "sys.unraisablehook argument type must be UnraisableHookArgs"
            )
        err_msg = getattr(unraisable, "err_msg", None)
        obj = getattr(unraisable, "object", None)
        exc_value = getattr(unraisable, "exc_value", None)
        exc_type = getattr(unraisable, "exc_type", None)
        exc_tb = getattr(unraisable, "exc_traceback", None)

        if err_msg is not None:
            if obj is not None:
                _MOLT_SYS_EXCEPTHOOK_WRITE(f"{err_msg}: {obj!r}\n")
            else:
                _MOLT_SYS_EXCEPTHOOK_WRITE(f"{err_msg}:\n")
        elif obj is not None:
            _MOLT_SYS_EXCEPTHOOK_WRITE(f"Exception ignored in: {obj!r}\n")

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

    def get_int_max_str_digits() -> int:
        return int((_MOLT_SYS_GET_INT_MAX_STR_DIGITS()))

    def set_int_max_str_digits(maxdigits: int) -> None:
        _MOLT_SYS_SET_INT_MAX_STR_DIGITS(maxdigits)

    def is_finalizing() -> bool:
        value = _MOLT_SYS_IS_FINALIZING()
        if not isinstance(value, bool):
            raise RuntimeError("molt_sys_is_finalizing returned invalid value")
        return value

    def getrefcount(obj: object) -> int:
        value = _MOLT_SYS_GETREFCOUNT(obj)
        if not isinstance(value, int) or isinstance(value, bool):
            raise RuntimeError("molt_sys_getrefcount returned invalid value")
        return value

    def getswitchinterval() -> float:
        return float((_MOLT_SYS_GETSWITCHINTERVAL()))

    def setswitchinterval(interval: float) -> None:
        _MOLT_SYS_SETSWITCHINTERVAL(interval)

    def settrace(tracefunc: object) -> None:
        _MOLT_SYS_SETTRACE(tracefunc)

    def gettrace() -> object:
        return _MOLT_SYS_GETTRACE()

    def setprofile(profilefunc: object) -> None:
        _MOLT_SYS_SETPROFILE(profilefunc)

    def getprofile() -> object:
        return _MOLT_SYS_GETPROFILE()

    def call_tracing(func: object, args: object) -> object:
        _MOLT_SYS_CALL_TRACING_VALIDATE(func, args)
        return func(*args)  # type: ignore[operator]

    def exception() -> BaseException | None:
        exc = _MOLT_EXCEPTION_ACTIVE()
        if exc is None:
            exc = _MOLT_EXCEPTION_LAST()
        if isinstance(exc, BaseException):
            return exc
        return None

    def addaudithook(hook: object) -> None:
        _MOLT_SYS_ADDAUDITHOOK(hook)

    def audit(event: str, *args: object) -> None:
        count = _MOLT_SYS_AUDIT_HOOK_COUNT()
        if not count:
            return
        hooks = _MOLT_SYS_AUDIT_GET_HOOKS()
        if isinstance(hooks, list):
            for hook in hooks:
                if callable(hook):
                    hook(event, args)

    g = globals()
    g.update(
        {
            "asyncgen_hooks": AsyncgenHooksTuple,
            "getrecursionlimit": getrecursionlimit,
            "setrecursionlimit": setrecursionlimit,
            "exc_info": exc_info,
            "_getframe": _getframe,
            "getdefaultencoding": getdefaultencoding,
            "getfilesystemencoding": getfilesystemencoding,
            "getfilesystemencodeerrors": getfilesystemencodeerrors,
            "get_asyncgen_hooks": get_asyncgen_hooks,
            "set_asyncgen_hooks": set_asyncgen_hooks,
            "intern": intern,
            "getsizeof": getsizeof,
            "displayhook": displayhook,
            "__displayhook__": displayhook,
            "excepthook": excepthook,
            "__excepthook__": excepthook,
            "unraisablehook": unraisablehook,
            "__unraisablehook__": unraisablehook,
            "get_int_max_str_digits": get_int_max_str_digits,
            "set_int_max_str_digits": set_int_max_str_digits,
            "is_finalizing": is_finalizing,
            "getrefcount": getrefcount,
            "getswitchinterval": getswitchinterval,
            "setswitchinterval": setswitchinterval,
            "settrace": settrace,
            "gettrace": gettrace,
            "setprofile": setprofile,
            "getprofile": getprofile,
            "call_tracing": call_tracing,
            "exception": exception,
            "addaudithook": addaudithook,
            "audit": audit,
        }
    )
    _heavy_api_initialized = True


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


# ---------------------------------------------------------------------------
# Namespace cleanup — remove names that are not part of CPython's sys public API.
# These are needed for type annotations, casting helpers, and intermediate
# variables but must not appear in the module __dict__ as non-underscore
# public names.
# ---------------------------------------------------------------------------
for _name in (
    "TYPE_CHECKING",
    "cast",
    "Callable",
    "Iterable",
    "version_obj",
    "abiflags_obj",
):
    globals().pop(_name, None)
