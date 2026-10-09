"""Minimal sys shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


# The native initializer publishes modules, process metadata, import state and
# stdio before executing this body. Python owns only shaped views and APIs.


# Frame depth counts Python frames only. Publish the runtime callable itself:
# a Python forwarding wrapper would change the visible stack and argument law.
_MOLT_GETFRAME = _require_intrinsic("molt_getframe")


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

_MOLT_GETRECURSIONLIMIT = _require_intrinsic("molt_getrecursionlimit")
_MOLT_SETRECURSIONLIMIT = _require_intrinsic("molt_setrecursionlimit")
_MOLT_EXCEPTION_ACTIVE = _require_intrinsic("molt_exception_active")
_MOLT_EXCEPTION_LAST = _require_intrinsic("molt_exception_last")
_MOLT_UNRAISABLE_HOOK_ARGS_IS_EXACT = _require_intrinsic("molt_unraisable_hook_args_is_exact")
_MOLT_ASYNCGEN_HOOKS_GET = _require_intrinsic("molt_asyncgen_hooks_get")
_MOLT_ASYNCGEN_HOOKS_SET = _require_intrinsic("molt_asyncgen_hooks_set")
_ASYNCGEN_HOOK_UNSET = object()
_MOLT_SYS_FLAGS_PAYLOAD = _require_intrinsic("molt_sys_flags_payload")
_MOLT_SYS_IS_FINALIZING = _require_intrinsic("molt_sys_is_finalizing")
_MOLT_SYS_GETREFCOUNT = _require_intrinsic("molt_sys_getrefcount")
_MOLT_SYS_SETTRACE = _require_intrinsic("molt_sys_settrace")
_MOLT_SYS_GETTRACE = _require_intrinsic("molt_sys_gettrace")
_MOLT_SYS_SETPROFILE = _require_intrinsic("molt_sys_setprofile")
_MOLT_SYS_GETPROFILE = _require_intrinsic("molt_sys_getprofile")
_MOLT_SYS_GETFILESYSTEMENCODEERRORS = _require_intrinsic("molt_sys_getfilesystemencodeerrors")
_MOLT_SYS_FLOAT_INFO = _require_intrinsic("molt_sys_float_info")
_MOLT_SYS_INT_INFO = _require_intrinsic("molt_sys_int_info")
_MOLT_SYS_HASH_INFO = _require_intrinsic("molt_sys_hash_info")
_MOLT_SYS_THREAD_INFO = _require_intrinsic("molt_sys_thread_info")
_MOLT_SYS_INTERN = _require_intrinsic("molt_sys_intern")
_MOLT_SYS_GETSIZEOF = _require_intrinsic("molt_sys_getsizeof")
_MOLT_TRACEBACK_FORMAT_EXCEPTION = _require_intrinsic("molt_traceback_format_exception")
_MOLT_SYS_GETDEFAULTENCODING = _require_intrinsic("molt_sys_getdefaultencoding")
_MOLT_SYS_GETFILESYSTEMENCODING = _require_intrinsic("molt_sys_getfilesystemencoding")
_MOLT_SYS_GETSWITCHINTERVAL = _require_intrinsic("molt_sys_getswitchinterval")
_MOLT_SYS_SETSWITCHINTERVAL = _require_intrinsic("molt_sys_setswitchinterval")
_MOLT_SYS_GET_INT_MAX_STR_DIGITS = _require_intrinsic("molt_sys_get_int_max_str_digits")
_MOLT_SYS_SET_INT_MAX_STR_DIGITS = _require_intrinsic("molt_sys_set_int_max_str_digits")
_MOLT_SYS_CALL_TRACING_VALIDATE = _require_intrinsic("molt_sys_call_tracing_validate")
_MOLT_SYS_ADDAUDITHOOK = _require_intrinsic("molt_sys_addaudithook")
_MOLT_SYS_AUDIT = _require_intrinsic("molt_sys_audit")
_MOLT_SYS_EXIT = _require_intrinsic("molt_sys_exit")
_MOLT_SYS_DISPLAYHOOK_WRITE = _require_intrinsic("molt_sys_displayhook_write")
_MOLT_SYS_EXCEPTHOOK_WRITE = _require_intrinsic("molt_sys_excepthook_write")

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
# CPython types these sequence fields as bool; the runtime payload carries ints.
_SYS_FLAGS_BOOL_FIELDS = frozenset(("dev_mode", "safe_path"))
# Named fields outside the sequence, with the first target minor exposing each.
_SYS_FLAGS_EXTRA_FIELDS = (
    ("gil", 13),
    ("context_aware_warnings", 14),
    ("thread_inherit_context", 14),
)


def _resolve_flags_payload(
    payload: object, target_minor: int
) -> tuple[tuple[int | bool, ...], dict[str, int]]:
    intrinsic_name = "molt_sys_flags_payload"
    if not isinstance(payload, dict):
        raise RuntimeError(f"{intrinsic_name} returned invalid value")
    values: list[int | bool] = []
    for field in _SYS_FLAGS_SEQUENCE_FIELDS:
        value = _expect_int(payload.get(field), intrinsic_name, field)
        values.append(bool(value) if field in _SYS_FLAGS_BOOL_FIELDS else value)
    extras: dict[str, int] = {}
    for field, minor in _SYS_FLAGS_EXTRA_FIELDS:
        if target_minor >= minor:
            extras[field] = _expect_int(payload.get(field), intrinsic_name, field)
    return tuple(values), extras


_VERSION_INFO_FIELDS = ("major", "minor", "micro", "releaselevel", "serial")


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


_INT_INFO_FIELDS = (
    "bits_per_digit",
    "sizeof_digit",
    "default_max_str_digits",
    "str_digits_check_threshold",
)


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


_THREAD_INFO_FIELDS = (
    "name",
    "lock",
    "version",
)


class _StructSequence(tuple):
    """CPython struct-sequence shape: a tuple with fixed, named fields.

    Each public type is a small subclass published by `_struct_sequence_type`;
    construction, field access and repr live here once.
    """

    __slots__ = ()
    _fields: tuple[str, ...] = ()
    _repr_prefix = ""
    # CPython refuses Python-level construction of some struct sequences
    # (sys.flags, sys.version_info); the bootstrap builds them with `_make`.
    _creatable = True
    n_fields = 0
    n_sequence_fields = 0
    n_unnamed_fields = 0

    def __new__(cls, values: object) -> "_StructSequence":
        if not cls._creatable:
            raise TypeError(f"cannot create '{cls._repr_prefix}' instances")
        return cls._make(values)

    @classmethod
    def _make(cls, values: object) -> "_StructSequence":
        items = tuple(values)
        if len(items) != len(cls._fields):
            raise TypeError(
                f"{cls._repr_prefix}() takes a {len(cls._fields)}-sequence "
                f"({len(items)}-sequence given)"
            )
        return tuple.__new__(cls, items)

    def __getattr__(self, name: str) -> object:
        fields = type(self)._fields
        if name in fields:
            return self[fields.index(name)]
        raise AttributeError(name)

    def __repr__(self) -> str:
        cls = type(self)
        items = ", ".join(
            f"{field}={value!r}" for field, value in zip(cls._fields, self)
        )
        return f"{cls._repr_prefix}({items})"


def _struct_sequence_type(
    cls: type, name: str, fields: tuple[str, ...], *, module: str = "sys"
) -> None:
    """Publish `cls` under CPython's type identity for a struct sequence."""
    cls._fields = fields
    cls._repr_prefix = name if module == "builtins" else f"{module}.{name}"
    cls.n_fields = cls.n_sequence_fields = len(fields)
    cls.__name__ = cls.__qualname__ = name
    cls.__module__ = module


class _Flags(_StructSequence):
    __slots__ = ()
    _creatable = False
    # Target-gated named fields outside the sequence, set once at bootstrap.
    _extra_values: dict[str, int] = {}

    def __getattr__(self, name: str) -> object:
        extra_values = type(self)._extra_values
        if name in extra_values:
            return extra_values[name]
        return _StructSequence.__getattr__(self, name)


class _VersionInfo(_StructSequence):
    __slots__ = ()
    _creatable = False


class _FloatInfo(_StructSequence):
    __slots__ = ()


class _IntInfo(_StructSequence):
    __slots__ = ()


class _HashInfo(_StructSequence):
    __slots__ = ()


class _ThreadInfo(_StructSequence):
    __slots__ = ()


class _AsyncgenHooks(_StructSequence):
    __slots__ = ()


_struct_sequence_type(_Flags, "flags", _SYS_FLAGS_SEQUENCE_FIELDS)
_struct_sequence_type(_VersionInfo, "version_info", _VERSION_INFO_FIELDS)
_struct_sequence_type(_FloatInfo, "float_info", _FLOAT_INFO_FIELDS)
_struct_sequence_type(_IntInfo, "int_info", _INT_INFO_FIELDS)
_struct_sequence_type(_HashInfo, "hash_info", _HASH_INFO_FIELDS)
_struct_sequence_type(_ThreadInfo, "thread_info", _THREAD_INFO_FIELDS)
_struct_sequence_type(
    _AsyncgenHooks, "asyncgen_hooks", ("firstiter", "finalizer"), module="builtins"
)


if "abiflags" in globals():
    __all__.insert(__all__.index("flags"), "abiflags")

def _validate_bootstrap_scalars() -> None:
    """Validate the native bootstrap's values without replacing their owners."""
    g = globals()
    maxsize_value = g["maxsize"]
    maxunicode_value = g["maxunicode"]
    byteorder_value = g["byteorder"]
    if not isinstance(maxsize_value, int) or isinstance(maxsize_value, bool) or maxsize_value <= 0:
        raise RuntimeError("canonical sys bootstrap returned invalid maxsize")
    if not isinstance(maxunicode_value, int) or isinstance(maxunicode_value, bool) or not 0 < maxunicode_value <= 0x10FFFF:
        raise RuntimeError("canonical sys bootstrap returned invalid maxunicode")
    if not isinstance(byteorder_value, str) or byteorder_value not in ("little", "big"):
        raise RuntimeError("canonical sys bootstrap returned invalid byteorder")


def _metadata_tuple(payload: object, name: str, count: int) -> tuple[object, ...]:
    if not isinstance(payload, (list, tuple)) or len(payload) != count:
        raise RuntimeError(f"{name} returned invalid value")
    return tuple(payload)


def _init_metadata_views() -> None:
    """Finalize public shapes once, during the canonical module initializer."""
    g = globals()
    _validate_bootstrap_scalars()
    raw_version = _expect_version_info_tuple(g["version_info"], "canonical sys bootstrap", "version_info")
    implementation_value = _resolve_implementation(g["implementation"])
    flags_values, flag_extras = _resolve_flags_payload(
        _MOLT_SYS_FLAGS_PAYLOAD(), raw_version[1]
    )
    float_values = _metadata_tuple(_MOLT_SYS_FLOAT_INFO(), "molt_sys_float_info", len(_FLOAT_INFO_FIELDS))
    int_values = _metadata_tuple(_MOLT_SYS_INT_INFO(), "molt_sys_int_info", len(_INT_INFO_FIELDS))
    hash_values = _metadata_tuple(_MOLT_SYS_HASH_INFO(), "molt_sys_hash_info", len(_HASH_INFO_FIELDS))
    thread_values = _metadata_tuple(_MOLT_SYS_THREAD_INFO(), "molt_sys_thread_info", len(_THREAD_INFO_FIELDS))
    g["version_info"] = _VersionInfo._make(raw_version)
    g["implementation"] = implementation_value
    _Flags._extra_values = flag_extras
    _Flags.n_fields = len(_SYS_FLAGS_SEQUENCE_FIELDS) + len(flag_extras)
    g["flags"] = _Flags._make(flags_values)
    g["float_info"] = _FloatInfo._make(float_values)
    g["int_info"] = _IntInfo._make(int_values)
    g["hash_info"] = _HashInfo._make(hash_values)
    g["thread_info"] = _ThreadInfo._make(thread_values)
    g["stdlib_module_names"] = frozenset(g["stdlib_module_names"])


_molt_bootstrap_pythonpath = ()
_molt_bootstrap_module_roots = ()
_molt_bootstrap_venv_site_packages = ()
_molt_bootstrap_pwd = "/"
_molt_bootstrap_include_cwd = False
_molt_bootstrap_stdlib_root = None


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


def getdefaultencoding() -> str:
    return _MOLT_SYS_GETDEFAULTENCODING()


def getfilesystemencoding() -> str:
    return _MOLT_SYS_GETFILESYSTEMENCODING()


def getfilesystemencodeerrors() -> str:
    return _MOLT_SYS_GETFILESYSTEMENCODEERRORS()


def get_asyncgen_hooks() -> object:
    hooks = _MOLT_ASYNCGEN_HOOKS_GET()
    if not isinstance(hooks, tuple) or len(hooks) != 2:
        raise RuntimeError("asyncgen hooks intrinsic returned invalid value")
    return _AsyncgenHooks._make(hooks)


def set_asyncgen_hooks(
    firstiter: object = _ASYNCGEN_HOOK_UNSET,
    finalizer: object = _ASYNCGEN_HOOK_UNSET,
) -> None:
    _MOLT_ASYNCGEN_HOOKS_SET(firstiter, finalizer, _ASYNCGEN_HOOK_UNSET)
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
    _MOLT_SYS_AUDIT(event, args)

# Direct aliases preserve callable identity without an export registry.
_getframe = _MOLT_GETFRAME
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


# Finalize shaped metadata before the initializer returns. Attribute reads
# never rerun producers or resurrect a deleted public key.
_init_metadata_views()

globals().pop("_require_intrinsic", None)
