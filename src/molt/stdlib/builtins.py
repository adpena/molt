"""Importable builtins for Molt.

Runtime publication owns primitive bindings; this facade supplies Python
wrappers and public metadata without a second bootstrap binding path.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# globals is already published and the executing module frame owns this dict.
_NS = globals()

# `builtins` must match CPython's public API surface; keep typing helpers out of the
# runtime module namespace.
if False:  # TYPE_CHECKING
    from typing import Any  # noqa: F401

    # The runtime publishes this name only for Python >= 3.13.
    PythonFinalizationError: type[RuntimeError]

# The canonical module initializer publishes runtime-backed builtins before
# module metadata or this Python body can recursively import another module.
# This facade only supplies Python-defined wrappers and public API metadata.
import sys as _sys

if False:  # TYPE_CHECKING
    from typing import Callable, Optional  # noqa: F401

    _molt_getargv: Callable[[], list[str]]
    _molt_getframe: Callable[[object], object]
    _molt_trace_enter_slot: Callable[[int], object]
    _molt_trace_exit: Callable[[], object]
    _molt_getrecursionlimit: Callable[[], int]
    _molt_setrecursionlimit: Callable[[int], None]
    _molt_sys_version: Callable[[], str]
    _molt_sys_stdin: Callable[[], object]
    _molt_sys_stdout: Callable[[], object]
    _molt_sys_stderr: Callable[[], object]
    _molt_sys_executable: Callable[[], str]
    _molt_exception_last: Callable[[], Optional[BaseException]]
    _molt_exception_last_pending: Callable[[], Optional[BaseException]]
    _molt_exception_active: Callable[[], Optional[BaseException]]
    _molt_asyncgen_hooks_get: Callable[[], object]
    _molt_asyncgen_hooks_set: Callable[[object, object], object]
    _molt_asyncgen_locals: Callable[[object], object]
    _molt_gen_locals: Callable[[object], object]
    _molt_code_new: Callable[
        [object, object, object, object, object, object, object, object, object], object
    ]
    molt_compile_builtin: Callable[[object, object, object, int, bool, int], object]
    _molt_module_new: Callable[[object], object]
    _molt_function_set_builtin: Callable[[object], object]
    _molt_class_new: Callable[[object], object]
    _molt_class_set_base: Callable[[object, object], object]
    _molt_class_apply_set_name: Callable[[object], object]
    _molt_sys_platform: Callable[[], str]
    _molt_getpid: Callable[[], int]
    _molt_getcwd: Callable[[], str]
    _molt_io_wait_new: Callable[[object, int, object], object]
    _molt_ws_wait_new: Callable[[object, int, object], object]
    molt_block_on: Callable[[object], object]
    molt_asyncgen_shutdown: Callable[[], object]
    molt_db_query_obj: Callable[[object, object], object]
    molt_db_exec_obj: Callable[[object, object], object]
    molt_msgpack_parse_scalar_obj: Callable[[object], object]
    molt_weakref_register: Callable[[object, object, object], None]
    molt_weakref_get: Callable[[object], object]
    molt_thread_spawn: Callable[[object], object]
    molt_thread_join: Callable[[object, object], object]
    molt_thread_is_alive: Callable[[object], object]
    molt_thread_ident: Callable[[object], object]
    molt_thread_native_id: Callable[[object], object]
    molt_thread_current_ident: Callable[[], object]
    molt_thread_current_native_id: Callable[[], object]
    molt_thread_drop: Callable[[object], object]
    molt_chan_new: Callable[[object], object]
    molt_chan_send: Callable[[object, object], object]
    molt_chan_recv: Callable[[object], object]
    molt_chan_try_send: Callable[[object, object], object]
    molt_chan_try_recv: Callable[[object], object]
    molt_chan_send_blocking: Callable[[object, object], object]
    molt_chan_recv_blocking: Callable[[object], object]
    molt_chan_drop: Callable[[object], object]
    molt_lock_new: Callable[[], object]
    molt_lock_acquire: Callable[[object, object, object], object]
    molt_lock_release: Callable[[object], object]
    molt_lock_locked: Callable[[object], object]
    molt_lock_drop: Callable[[object], object]
    molt_rlock_new: Callable[[], object]
    molt_rlock_acquire: Callable[[object, object, object], object]
    molt_rlock_release: Callable[[object], object]
    molt_rlock_locked: Callable[[object], object]
    molt_rlock_drop: Callable[[object], object]


def compile(
    source: object,
    filename: object,
    mode: object,
    flags: int = 0,
    dont_inherit: bool = False,
    optimize: int = -1,
    *,
    _feature_version: int = -1,
):
    del _feature_version
    intrinsic = _require_intrinsic("molt_compile_builtin", _NS)
    return intrinsic(source, filename, mode, flags, dont_inherit, optimize)


def _dynamic_execution_unavailable(name: str) -> RuntimeError:
    return RuntimeError(
        "MOLT_COMPAT_ERROR: "
        f"{name}() is unsupported in compiled Molt binaries; "
        "dynamic code execution is outside the verified subset. "
        "Use static modules or pre-generated code paths instead."
    )


def eval(source, globals=None, locals=None):
    raise _dynamic_execution_unavailable("eval")


def exec(source, globals=None, locals=None, *, closure=None):
    raise _dynamic_execution_unavailable("exec")


_MOLT_POW = _require_intrinsic("molt_pow", _NS)
_MOLT_POW_MOD = _require_intrinsic("molt_pow_mod", _NS)


def pow(base, exp, mod=None):
    if mod is None:
        return _MOLT_POW(base, exp)
    return _MOLT_POW_MOD(base, exp, mod)


def input(prompt: object = "", /) -> str:
    intrinsic = _require_intrinsic("molt_input_builtin", _NS)
    return intrinsic(prompt)


def breakpoint(*args: object, **kws: object) -> object:
    hook = getattr(_sys, "breakpointhook", None)
    if hook is None:
        raise RuntimeError("sys.breakpointhook unavailable")
    return hook(*args, **kws)


# Policy-deferred: dynamic execution (`eval`/`exec`/`compile`) remains intentionally unsupported for compiled binaries; `compile` currently provides parser-backed validation only and any broader execution support requires explicit capability-gated approval with utility/performance evidence.
__all__ = [
    "object",
    "type",
    "isinstance",
    "issubclass",
    "len",
    "hash",
    "ord",
    "chr",
    "ascii",
    "bin",
    "oct",
    "hex",
    "abs",
    "divmod",
    "pow",
    "compile",
    "open",
    "input",
    "breakpoint",
    "eval",
    "exec",
    "__import__",
    "globals",
    "locals",
    "repr",
    "format",
    "dir",
    "callable",
    "any",
    "all",
    "sum",
    "sorted",
    "min",
    "max",
    "id",
    "str",
    "range",
    "enumerate",
    "slice",
    "list",
    "tuple",
    "dict",
    "float",
    "complex",
    "int",
    "bool",
    "round",
    "set",
    "frozenset",
    "bytes",
    "bytearray",
    "memoryview",
    "iter",
    "map",
    "filter",
    "zip",
    "reversed",
    "next",
    "aiter",
    "anext",
    "getattr",
    "setattr",
    "delattr",
    "hasattr",
    "super",
    "property",
    "classmethod",
    "staticmethod",
    "print",
    # Tooling/interactive builtins (site-like conveniences).
    "help",
    "credits",
    "copyright",
    "license",
    "quit",
    "exit",
    "vars",
    "Ellipsis",
    "NotImplemented",
    "BaseException",
    "BaseExceptionGroup",
    "Exception",
    "ExceptionGroup",
    "ArithmeticError",
    "AssertionError",
    "AttributeError",
    "BufferError",
    "EOFError",
    "FloatingPointError",
    "GeneratorExit",
    "ImportError",
    "ModuleNotFoundError",
    "IndexError",
    "KeyError",
    "KeyboardInterrupt",
    "LookupError",
    "MemoryError",
    "NameError",
    "NotImplementedError",
    "PythonFinalizationError",
    "OSError",
    "EnvironmentError",
    "IOError",
    "WindowsError",
    "BlockingIOError",
    "ChildProcessError",
    "ConnectionError",
    "BrokenPipeError",
    "ConnectionAbortedError",
    "ConnectionRefusedError",
    "ConnectionResetError",
    "FileExistsError",
    "OverflowError",
    "PermissionError",
    "FileNotFoundError",
    "InterruptedError",
    "IsADirectoryError",
    "NotADirectoryError",
    "RecursionError",
    "ReferenceError",
    "RuntimeError",
    "StopIteration",
    "StopAsyncIteration",
    "SyntaxError",
    "IndentationError",
    "TabError",
    "SystemError",
    "SystemExit",
    "TimeoutError",
    "ProcessLookupError",
    "TypeError",
    "UnboundLocalError",
    "UnicodeError",
    "UnicodeDecodeError",
    "UnicodeEncodeError",
    "UnicodeTranslateError",
    "ValueError",
    "ZeroDivisionError",
    "Warning",
    "DeprecationWarning",
    "PendingDeprecationWarning",
    "RuntimeWarning",
    "SyntaxWarning",
    "UserWarning",
    "FutureWarning",
    "ImportWarning",
    "UnicodeWarning",
    "BytesWarning",
    "ResourceWarning",
    "EncodingWarning",
]

try:
    open.__module__ = "_io"
except Exception:
    pass

# CPython exposes these through site; Molt supplies its native site helpers.
import _sitebuiltins as _sitebuiltins  # noqa: PLC0415,E402

help = _sitebuiltins.help
credits = _sitebuiltins.credits
copyright = _sitebuiltins.copyright
license = _sitebuiltins.license
quit = _sitebuiltins.quit
exit = _sitebuiltins.exit

# Runtime publication owns version, platform, and executable/profile admission.
# Project the public list from that namespace instead of repeating its gates.
__all__ = [name for name in __all__ if name in _NS]

_molt_getargv = _require_intrinsic("molt_getargv", _NS)
_molt_getframe = _require_intrinsic("molt_getframe", _NS)
_molt_trace_enter_slot = _require_intrinsic("molt_trace_enter_slot", _NS)
_molt_trace_exit = _require_intrinsic("molt_trace_exit", _NS)
_molt_getrecursionlimit = _require_intrinsic("molt_getrecursionlimit", _NS)
_molt_setrecursionlimit = _require_intrinsic("molt_setrecursionlimit", _NS)
_molt_sys_version = _require_intrinsic("molt_sys_version", _NS)
_molt_sys_stdin = _require_intrinsic("molt_sys_stdin", _NS)
_molt_sys_stdout = _require_intrinsic("molt_sys_stdout", _NS)
_molt_sys_stderr = _require_intrinsic("molt_sys_stderr", _NS)
_molt_exception_last = _require_intrinsic("molt_exception_last", _NS)
_molt_exception_last_pending = _require_intrinsic("molt_exception_last_pending", _NS)
_molt_exception_active = _require_intrinsic("molt_exception_active", _NS)
_molt_asyncgen_hooks_get = _require_intrinsic("molt_asyncgen_hooks_get", _NS)
_molt_asyncgen_hooks_set = _require_intrinsic("molt_asyncgen_hooks_set", _NS)
_molt_asyncgen_locals = _require_intrinsic("molt_asyncgen_locals", _NS)
_molt_module_new = _require_intrinsic("molt_module_new", _NS)
_molt_function_set_builtin = _require_intrinsic("molt_function_set_builtin", _NS)
_molt_function_set_builtin(compile)
_molt_function_set_builtin(input)
_molt_function_set_builtin(breakpoint)
_molt_function_set_builtin(eval)
_molt_function_set_builtin(exec)
_molt_function_set_builtin(pow)
try:
    # CPython 3.12+ `inspect.signature` uses `__text_signature__` for these builtins.
    eval.__text_signature__ = "(source, globals=None, locals=None, /)"  # type: ignore[attr-defined]
    exec.__text_signature__ = (  # type: ignore[attr-defined]
        "(source, globals=None, locals=None, /, *, closure=None)"
    )
except Exception as _exc:  # noqa: BLE001
    raise RuntimeError(
        "builtins.eval/exec missing __text_signature__ support for inspect.signature parity"
    ) from _exc

try:
    # CPython 3.12+ builtin-function signatures (Python-defined builtins in this module).
    compile.__text_signature__ = (  # type: ignore[attr-defined]
        "(source, filename, mode, flags=0, dont_inherit=False, optimize=-1, *, _feature_version=-1)"
    )
    input.__text_signature__ = "(prompt='', /)"  # type: ignore[attr-defined]
    pow.__text_signature__ = "(base, exp, mod=None)"  # type: ignore[attr-defined]
except Exception as _exc:  # noqa: BLE001
    raise RuntimeError(
        "builtins.compile/input/pow missing __text_signature__ support for inspect.signature parity"
    ) from _exc
_molt_class_new = _require_intrinsic("molt_class_new", _NS)
_molt_class_set_base = _require_intrinsic("molt_class_set_base", _NS)
_molt_class_apply_set_name = _require_intrinsic("molt_class_apply_set_name", _NS)
_molt_sys_platform = _require_intrinsic("molt_sys_platform", _NS)
_molt_getpid = _require_intrinsic("molt_getpid", _NS)
_molt_getcwd = _require_intrinsic("molt_getcwd", _NS)
