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

# Complete construction metadata before sealing the public builtin role.
# Native callable metadata is readonly after finalization; initialization must
# propagate its original failure instead of exposing a partially initialized API.
if _sys.version_info >= (3, 13):
    eval.__text_signature__ = "($module, source, /, globals=None, locals=None)"  # type: ignore[attr-defined]
    exec.__text_signature__ = (  # type: ignore[attr-defined]
        "($module, source, /, globals=None, locals=None, *, closure=None)"
    )
    breakpoint.__text_signature__ = "($module, /, *args, **kws)"  # type: ignore[attr-defined]
else:
    eval.__text_signature__ = "($module, source, globals=None, locals=None, /)"  # type: ignore[attr-defined]
    exec.__text_signature__ = (  # type: ignore[attr-defined]
        "($module, source, globals=None, locals=None, /, *, closure=None)"
    )
    breakpoint.__text_signature__ = None  # type: ignore[attr-defined]
compile.__text_signature__ = (  # type: ignore[attr-defined]
    "($module, /, source, filename, mode, flags=0,\n"
    "        dont_inherit=False, optimize=-1, *, _feature_version=-1)"
)
input.__text_signature__ = "($module, prompt='', /)"  # type: ignore[attr-defined]

_molt_function_set_builtin = _require_intrinsic("molt_function_set_builtin", _NS)
_molt_function_set_builtin(compile)
_molt_function_set_builtin(input)
_molt_function_set_builtin(breakpoint)
_molt_function_set_builtin(eval)
_molt_function_set_builtin(exec)
