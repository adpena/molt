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
_MOLT_UNRAISABLE_HOOK_ARGS_IS_EXACT = _require_intrinsic(
    "molt_unraisable_hook_args_is_exact"
)
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
_MOLT_SYS_GETFILESYSTEMENCODEERRORS = _require_intrinsic(
    "molt_sys_getfilesystemencodeerrors"
)
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

# Runtime sys module publication is the sole argv/executable authority.  Python
# placeholders would execute after publication and could overwrite the process
# values, making behavior depend on module/link initialization order.


def exit(status: object = None, /) -> None:
    # CPython raises through _PyErr_SetObject: a SystemExit instance is raised
    # as is, None means no arguments and a tuple supplies the arguments.
    if issubclass(type(status), SystemExit):
        raise status
    if status is None:
        raise SystemExit
    if isinstance(status, tuple):
        raise SystemExit(*status)
    raise SystemExit(status)


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
    ("thread_inherit_context", 14),
    ("context_aware_warnings", 14),
    ("lazy_imports", 15),
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


_MOLT_STRUCTSEQ_TYPE = _require_intrinsic("molt_structseq_type")
_struct_sequence_new = _require_intrinsic("molt_structseq_new")


def _struct_sequence_type(
    name: str,
    fields: tuple[str | None, ...],
    *,
    module: str = "sys",
    visible: int | None = None,
    constructor: int = 0,
    static_origin: bool = True,
    docs: tuple[str | None, ...] | None = None,
    doc: str | None = None,
) -> type:
    """Publish one native direct-tuple class with an immutable field layout."""
    if visible is None:
        visible = len(fields)
    if docs is None:
        docs = (None,) * len(fields)
    fullname = name if module == "builtins" else module + "." + name
    return _MOLT_STRUCTSEQ_TYPE(
        fullname, module, fields, docs, visible, constructor, static_origin, doc
    )


_VersionInfo = _struct_sequence_type(
    "version_info",
    _VERSION_INFO_FIELDS,
    constructor=1,
    docs=(
        "Major release number",
        "Minor release number",
        "Patch release number",
        "'alpha', 'beta', 'candidate', or 'final'",
        "Serial release number",
    ),
    doc="sys.version_info\n\nVersion information as a named tuple.",
)
_FloatInfo = _struct_sequence_type(
    "float_info",
    _FLOAT_INFO_FIELDS,
    docs=(
        "DBL_MAX -- maximum representable finite float",
        "DBL_MAX_EXP -- maximum int e such that radix**(e-1) is representable",
        "DBL_MAX_10_EXP -- maximum int e such that 10**e is representable",
        "DBL_MIN -- Minimum positive normalized float",
        "DBL_MIN_EXP -- minimum int e such that radix**(e-1) is a normalized float",
        "DBL_MIN_10_EXP -- minimum int e such that 10**e is a normalized float",
        "DBL_DIG -- maximum number of decimal digits that can be faithfully represented in a float",
        "DBL_MANT_DIG -- mantissa digits",
        "DBL_EPSILON -- Difference between 1 and the next representable float",
        "FLT_RADIX -- radix of exponent",
        "FLT_ROUNDS -- rounding mode used for arithmetic operations",
    ),
    doc="sys.float_info\n\nA named tuple holding information about the float type. It contains low level\ninformation about the precision and internal representation. Please study\nyour system's :file:`float.h` for more information.",
)
_IntInfo = _struct_sequence_type(
    "int_info",
    _INT_INFO_FIELDS,
    docs=(
        "size of a digit in bits",
        "size in bytes of the C type used to represent a digit",
        "maximum string conversion digits limitation",
        "minimum positive value for int_max_str_digits",
    ),
    doc="sys.int_info\n\nA named tuple that holds information about Python's\ninternal representation of integers.  The attributes are read only.",
)
_HashInfo = _struct_sequence_type(
    "hash_info",
    _HASH_INFO_FIELDS,
    docs=(
        "width of the type used for hashing, in bits",
        "prime number giving the modulus on which the hash function is based",
        "value to be used for hash of a positive infinity",
        "value to be used for hash of a nan",
        "multiplier used for the imaginary part of a complex number",
        "name of the algorithm for hashing of str, bytes and memoryviews",
        "internal output size of hash algorithm",
        "seed size of hash algorithm",
        "small string optimization cutoff",
    ),
    doc="hash_info\n\nA named tuple providing parameters used for computing\nhashes. The attributes are read only.",
)
_ThreadInfo = _struct_sequence_type(
    "thread_info",
    _THREAD_INFO_FIELDS,
    docs=(
        "name of the thread implementation",
        "name of the lock implementation",
        "name and version of the thread library",
    ),
    doc="sys.thread_info\n\nA named tuple holding information about the thread implementation.",
)
_AsyncgenHooks = _struct_sequence_type(
    "asyncgen_hooks",
    ("firstiter", "finalizer"),
    module="builtins",
    docs=("Hook to intercept first iteration", "Hook to intercept finalization"),
    doc="asyncgen_hooks\n\nA named tuple providing information about asynchronous\ngenerators hooks.  The attributes are read only.",
)


if "abiflags" in globals():
    __all__.insert(__all__.index("flags"), "abiflags")


def _validate_bootstrap_scalars() -> None:
    """Validate the native bootstrap's values without replacing their owners."""
    g = globals()
    maxsize_value = g["maxsize"]
    maxunicode_value = g["maxunicode"]
    byteorder_value = g["byteorder"]
    if (
        not isinstance(maxsize_value, int)
        or isinstance(maxsize_value, bool)
        or maxsize_value <= 0
    ):
        raise RuntimeError("canonical sys bootstrap returned invalid maxsize")
    if (
        not isinstance(maxunicode_value, int)
        or isinstance(maxunicode_value, bool)
        or not 0 < maxunicode_value <= 0x10FFFF
    ):
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
    raw_version = _expect_version_info_tuple(
        g["version_info"], "canonical sys bootstrap", "version_info"
    )
    implementation_value = _resolve_implementation(g["implementation"])
    flags_values, flag_extras = _resolve_flags_payload(
        _MOLT_SYS_FLAGS_PAYLOAD(), raw_version[1]
    )
    float_values = _metadata_tuple(
        _MOLT_SYS_FLOAT_INFO(), "molt_sys_float_info", len(_FLOAT_INFO_FIELDS)
    )
    int_values = _metadata_tuple(
        _MOLT_SYS_INT_INFO(), "molt_sys_int_info", len(_INT_INFO_FIELDS)
    )
    hash_values = _metadata_tuple(
        _MOLT_SYS_HASH_INFO(), "molt_sys_hash_info", len(_HASH_INFO_FIELDS)
    )
    thread_values = _metadata_tuple(
        _MOLT_SYS_THREAD_INFO(), "molt_sys_thread_info", len(_THREAD_INFO_FIELDS)
    )
    g["version_info"] = _struct_sequence_new(_VersionInfo, raw_version)
    g["implementation"] = implementation_value
    flags_type = _struct_sequence_type(
        "flags",
        _SYS_FLAGS_SEQUENCE_FIELDS + tuple(flag_extras),
        visible=len(_SYS_FLAGS_SEQUENCE_FIELDS),
        constructor=1,
        docs=(
            "-d",
            "-i",
            "-i",
            "-O or -OO",
            "-B",
            "-s",
            "-S",
            "-E",
            "-v",
            "-b",
            "-q",
            "-R",
            "-I",
            "-X dev",
            "-X utf8",
            "-X warn_default_encoding",
            "-P",
            "-X int_max_str_digits",
        )
        + tuple("-X " + name for name in flag_extras),
        doc="sys.flags\n\nFlags provided through command line arguments or environment vars.",
    )
    g["flags"] = _struct_sequence_new(
        flags_type, flags_values + tuple(flag_extras.values())
    )
    g["float_info"] = _struct_sequence_new(_FloatInfo, float_values)
    g["int_info"] = _struct_sequence_new(_IntInfo, int_values)
    g["hash_info"] = _struct_sequence_new(_HashInfo, hash_values)
    g["thread_info"] = _struct_sequence_new(_ThreadInfo, thread_values)
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
    return _struct_sequence_new(_AsyncgenHooks, hooks)


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


# The default hooks write to the interpreter's current streams, as CPython's
# do: `stdout` and `stderr` are this module's live attributes, so a reassigned
# or redirected stream receives the text.
_MISSING_STREAM = object()


def _current_stream(name: str) -> object:
    try:
        return stdout if name == "stdout" else stderr  # noqa: F821
    except NameError:
        return _MISSING_STREAM


def _flush_ignoring_errors(stream: object) -> None:
    try:
        stream.flush()  # type: ignore[attr-defined]
    except Exception:  # noqa: BLE001
        pass


def _exception_text(exc_type: object, exc_value: object, exc_tb: object) -> str:
    try:
        lines = _MOLT_TRACEBACK_FORMAT_EXCEPTION(
            exc_type, exc_value, exc_tb, None, True
        )
    except BaseException:  # noqa: BLE001
        lines = None
    if isinstance(lines, list) and all(isinstance(line, str) for line in lines):
        return "".join(lines)
    type_name = getattr(exc_type, "__name__", None)
    if not isinstance(type_name, str):
        type_name = str(exc_type)
    detail = str(exc_value) if exc_value is not None else ""
    if detail:
        return f"{type_name}: {detail}\n"
    return f"{type_name}\n"


def displayhook(value: object) -> None:
    if value is None:
        return
    out = _current_stream("stdout")
    if out is _MISSING_STREAM or out is None:
        raise RuntimeError("lost sys.stdout")
    _builtins = modules.get("builtins")
    if _builtins is not None:
        _builtins._ = None  # type: ignore[attr-defined]
    out.write(repr(value))  # type: ignore[attr-defined]
    out.write("\n")  # type: ignore[attr-defined]
    if _builtins is not None:
        _builtins._ = value  # type: ignore[attr-defined]


def excepthook(exc_type: object, exc_value: object, exc_tb: object) -> None:
    err = _current_stream("stderr")
    if err is None:
        return
    text = _exception_text(exc_type, exc_value, exc_tb)
    if err is _MISSING_STREAM:
        original = globals().get("__stderr__")
        if original is not None:
            original.write(text)
            original.write("lost sys.stderr\n")
        return
    err.write(text)  # type: ignore[attr-defined]
    _flush_ignoring_errors(err)


def unraisablehook(unraisable: object) -> None:
    if not _MOLT_UNRAISABLE_HOOK_ARGS_IS_EXACT(unraisable):
        raise TypeError("sys.unraisablehook argument type must be UnraisableHookArgs")
    err = _current_stream("stderr")
    if err is None or err is _MISSING_STREAM:
        return
    err_msg = getattr(unraisable, "err_msg", None)
    obj = getattr(unraisable, "object", None)
    exc_value = getattr(unraisable, "exc_value", None)
    exc_type = getattr(unraisable, "exc_type", None)
    exc_tb = getattr(unraisable, "exc_traceback", None)

    if err_msg is not None:
        if obj is not None:
            err.write(f"{err_msg}: {obj!r}\n")  # type: ignore[attr-defined]
        else:
            err.write(f"{err_msg}:\n")  # type: ignore[attr-defined]
    elif obj is not None:
        err.write(f"Exception ignored in: {obj!r}\n")  # type: ignore[attr-defined]
    if exc_value is not None:
        err.write(_exception_text(exc_type, exc_value, exc_tb))  # type: ignore[attr-defined]
    _flush_ignoring_errors(err)


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

# CPython publishes its macOS framework name on darwin, "" for a build that is
# not a framework. A Molt binary is never a framework build.
if globals().get("platform") == "darwin":
    _framework = ""


# Finalize shaped metadata before the initializer returns. Attribute reads
# never rerun producers or resurrect a deleted public key.
_init_metadata_views()

globals().pop("_require_intrinsic", None)
