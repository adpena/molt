"""Importable builtins for Molt.

Bind supported builtins to module globals so `import builtins` works in
compiled code without introducing dynamic indirection.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
import sys as _sys

_MOLT_SYS_MODULES = _require_intrinsic("molt_sys_modules")


def _modules_dict() -> dict[str, object]:
    modules = _MOLT_SYS_MODULES()
    if not isinstance(modules, dict):
        raise RuntimeError("molt_sys_modules returned invalid value")
    return modules


def _module_namespace(name: str) -> dict[str, object]:
    module = _modules_dict().get(name)
    namespace = getattr(module, "__dict__", None)
    if isinstance(namespace, dict):
        return namespace
    return globals()


# During builtins bootstrap we cannot rely on `globals()` / `locals()` being present
# yet (we are defining them). Use the module object dict via the runtime modules map.
_NS = _module_namespace(__name__)

# `builtins` must match CPython's public API surface; keep typing helpers out of the
# runtime module namespace.
if False:  # TYPE_CHECKING
    from typing import Any  # noqa: F401

_MOLT_BOOTSTRAP_DESCRIPTOR_TYPES = _require_intrinsic(
    "molt_bootstrap_descriptor_types", _NS
)
_MOLT_MODULE_IMPORT = _require_intrinsic("molt_module_import", _NS)
_MOLT_EXCEPTION_CLEAR = _require_intrinsic("molt_exception_clear", _NS)

# Provide `builtins.globals` / `builtins.locals` early during bootstrap. Other stdlib
# modules may import `builtins` during their own initialization and expect these to
# exist (CPython parity).
globals = _require_intrinsic("molt_globals_builtin", _NS)
locals = _require_intrinsic("molt_locals_builtin", _NS)
try:
    globals.__text_signature__ = "()"  # type: ignore[attr-defined]
    locals.__text_signature__ = "()"  # type: ignore[attr-defined]
except Exception:  # noqa: BLE001
    pass  # Non-fatal: cosmetic for inspect.signature parity


try:
    classmethod, staticmethod, property = _MOLT_BOOTSTRAP_DESCRIPTOR_TYPES()
except Exception as _exc:  # noqa: BLE001
    raise RuntimeError(
        "descriptor bootstrap unresolved: expected (classmethod, staticmethod, property)"
    ) from _exc


def _resolve_import_name(name: str, globals_obj, level: int) -> str:
    if level <= 0:
        return name
    package = None
    if isinstance(globals_obj, dict):
        package = globals_obj.get("__package__")
        if not package and globals_obj.get("__path__") and globals_obj.get("__name__"):
            package = globals_obj.get("__name__")
    if not package:
        raise ImportError("relative import requires package")
    parts = package.split(".")
    if level > len(parts):
        raise ImportError("attempted relative import beyond top-level package")
    cut = len(parts) - level + 1
    base = ".".join(parts[:cut])
    return f"{base}.{name}" if name and base else (name or base)


def _is_placeholder_module(mod: object) -> bool:
    module_dict = getattr(mod, "__dict__", None)
    if not isinstance(module_dict, dict):
        return False
    if len(module_dict) == 1 and "__name__" in module_dict:
        return True
    if not module_dict or any(not key.startswith("_") for key in module_dict):
        return False
    return any(
        key in module_dict
        for key in (
            "_molt_intrinsic_lookup",
            "_molt_intrinsics",
            "_molt_runtime",
        )
    )


def _require_importlib_util_module() -> object:
    modules = _modules_dict()
    mod = modules.get("importlib.util")
    if mod is None:
        mod = _MOLT_MODULE_IMPORT("importlib.util")
    if mod is None:
        raise ImportError("No module named 'importlib.util'")
    return mod


def _recover_placeholder_module(resolved: str, placeholder: object):
    modules = _modules_dict()
    previous = modules.pop(resolved, None)
    try:
        importlib_util = _require_importlib_util_module()
        find_spec = getattr(importlib_util, "find_spec", None)
        module_from_spec = getattr(importlib_util, "module_from_spec", None)
        if not callable(find_spec) or not callable(module_from_spec):
            raise RuntimeError("importlib.util missing required loader helpers")
        spec = find_spec(resolved, None)
        if spec is None:
            raise ImportError(f"No module named '{resolved}'")
        module = module_from_spec(spec)
        modules[resolved] = module
        loader = getattr(spec, "loader", None)
        if loader is not None:
            if hasattr(loader, "exec_module"):
                loader.exec_module(module)
            elif hasattr(loader, "load_module"):
                loaded = loader.load_module(resolved)
                if loaded is not None:
                    module = loaded
        recovered = modules.get(resolved, module)
        if _is_placeholder_module(recovered):
            raise ImportError(f"import of {resolved} produced placeholder module")
        return recovered
    except Exception:
        modules.pop(resolved, None)
        if previous is not None and previous is not placeholder:
            modules[resolved] = previous
        raise


def _load_via_spec(resolved: str):
    modules = _modules_dict()
    existing = modules.get(resolved)
    if existing is not None and not _is_placeholder_module(existing):
        return existing
    importlib_util = _require_importlib_util_module()
    find_spec = getattr(importlib_util, "find_spec", None)
    module_from_spec = getattr(importlib_util, "module_from_spec", None)
    if not callable(find_spec) or not callable(module_from_spec):
        raise RuntimeError("importlib.util missing required loader helpers")
    spec = find_spec(resolved, None)
    if spec is None:
        raise ImportError(f"No module named '{resolved}'")
    module = module_from_spec(spec)
    modules[resolved] = module
    try:
        loader = getattr(spec, "loader", None)
        if loader is not None:
            if hasattr(loader, "exec_module"):
                loader.exec_module(module)
            elif hasattr(loader, "load_module"):
                loaded = loader.load_module(resolved)
                if loaded is not None:
                    module = loaded
        loaded_module = modules.get(resolved, module)
        if _is_placeholder_module(loaded_module):
            raise ImportError(f"import of {resolved} produced placeholder module")
        return loaded_module
    except Exception:
        modules.pop(resolved, None)
        raise


def _intrinsic_import(name, globals=None, locals=None, fromlist=(), level=0):
    if not name:
        raise ImportError("Empty module name")
    resolved = _resolve_import_name(name, globals, level) if level else name
    modules = _modules_dict()
    if resolved in modules:
        mod = modules[resolved]
        if mod is None:
            raise ImportError(f"import of {resolved} halted; None in sys.modules")
        if _is_placeholder_module(mod):
            mod = _recover_placeholder_module(resolved, mod)
        if fromlist:
            return mod
        top = resolved.split(".", 1)[0]
        return modules.get(top, mod)
    try:
        mod = _MOLT_MODULE_IMPORT(resolved)
    except (ImportError, TypeError):
        _MOLT_EXCEPTION_CLEAR()
        mod = _load_via_spec(resolved)
    if mod is None:
        _MOLT_EXCEPTION_CLEAR()
        mod = _load_via_spec(resolved)
    if _is_placeholder_module(mod):
        mod = _recover_placeholder_module(resolved, mod)
    if fromlist:
        return mod
    top = resolved.split(".", 1)[0]
    return modules.get(top, mod)


__import__ = _intrinsic_import

if False:  # TYPE_CHECKING
    from typing import Callable, Optional  # noqa: F401

    _molt_getargv: Callable[[], list[str]]
    _molt_getframe: Callable[[object], object]
    _molt_trace_enter_slot: Callable[[int], object]
    _molt_trace_exit: Callable[[], object]
    _molt_getrecursionlimit: Callable[[], int]
    _molt_setrecursionlimit: Callable[[int], None]
    _molt_sys_version_info: Callable[[], tuple[int, int, int, str, int]]
    _molt_sys_version: Callable[[], str]
    _molt_sys_stdin: Callable[[], object]
    _molt_sys_stdout: Callable[[], object]
    _molt_sys_stderr: Callable[[], object]
    _molt_sys_executable: Callable[[], str]
    _molt_exception_last: Callable[[], Optional[BaseException]]
    _molt_exception_active: Callable[[], Optional[BaseException]]
    _molt_asyncgen_hooks_get: Callable[[], object]
    _molt_asyncgen_hooks_set: Callable[[object, object], object]
    _molt_asyncgen_locals: Callable[[object], object]
    _molt_gen_locals: Callable[[object], object]
    _molt_code_new: Callable[
        [object, object, object, object, object, object, object, object], object
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
    molt_weakref_register: Callable[[object, object, object], object]
    molt_weakref_get: Callable[[object], object]
    molt_weakref_drop: Callable[[object], object]
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


def _require_builtin_intrinsic(name: str) -> object:
    return _require_intrinsic(name, _NS)


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
    intrinsic = _require_builtin_intrinsic("molt_compile_builtin")
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


_MOLT_POW = _require_builtin_intrinsic("molt_pow")
_MOLT_POW_MOD = _require_builtin_intrinsic("molt_pow_mod")


def pow(base, exp, mod=None):
    if mod is None:
        return _MOLT_POW(base, exp)
    return _MOLT_POW_MOD(base, exp, mod)


def input(prompt: object = "", /) -> str:
    intrinsic = _require_builtin_intrinsic("molt_input_builtin")
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

object = object

type = type

isinstance = isinstance
issubclass = issubclass

len = len
hash = hash
ord = ord
chr = chr
ascii = ascii
bin = bin
oct = oct
hex = hex
abs = abs
divmod = divmod
open = open
try:
    open.__module__ = "_io"
except Exception:
    pass
repr = repr
format = format
dir = dir
callable = callable
any = any
all = all
sum = sum
sorted = sorted
min = min
max = max
id = id
str = str
range = range
enumerate = enumerate
slice = slice
list = list
tuple = tuple
dict = dict
float = float
_complex_type = _NS.get("complex")
if not isinstance(_complex_type, type):
    try:
        _complex_type = type(0j)
    except Exception as _exc:
        raise RuntimeError(
            "builtins.complex requires runtime complex-type support"
        ) from _exc
complex = _complex_type
int = int
bool = bool
round = round
set = set
frozenset = frozenset
bytes = bytes
bytearray = bytearray
memoryview = memoryview
iter = iter
_molt_builtin_class_lookup = _require_builtin_intrinsic("molt_builtin_class_lookup")
enumerate = _molt_builtin_class_lookup("enumerate")
reversed = _molt_builtin_class_lookup("reversed")
zip = _molt_builtin_class_lookup("zip")
map = _molt_builtin_class_lookup("map")
filter = _molt_builtin_class_lookup("filter")
next = next
aiter = aiter
anext = anext
getattr = getattr
setattr = setattr
delattr = delattr
hasattr = hasattr
super = super
print = print

# CPython exposes these via `site`, but compiled Molt binaries should have them
# available without importing host Python. This is a Molt-native `_sitebuiltins`.
import _sitebuiltins as _sitebuiltins  # noqa: PLC0415,E402

help = _sitebuiltins.help
credits = _sitebuiltins.credits
copyright = _sitebuiltins.copyright
license = _sitebuiltins.license
quit = _sitebuiltins.quit
exit = _sitebuiltins.exit
vars = vars
Ellipsis = ...
# Avoid bootstrap-time global lookup of NotImplemented in runtimes where builtins
# are still being initialized; rich-compare returns the singleton directly.
NotImplemented = object.__eq__(object(), object())
_NS["True"] = True
_NS["False"] = False
_NS["None"] = None
BaseException = BaseException
BaseExceptionGroup = BaseExceptionGroup
Exception = Exception
ExceptionGroup = ExceptionGroup


ArithmeticError = ArithmeticError
AssertionError = AssertionError
AttributeError = AttributeError
BufferError = BufferError
EOFError = EOFError
FloatingPointError = FloatingPointError
GeneratorExit = GeneratorExit
ImportError = ImportError
ModuleNotFoundError = ModuleNotFoundError
IndexError = IndexError
KeyError = KeyError
KeyboardInterrupt = KeyboardInterrupt
LookupError = LookupError
MemoryError = MemoryError
NameError = NameError
UnboundLocalError = UnboundLocalError
NotImplementedError = NotImplementedError
OSError = OSError
EnvironmentError = EnvironmentError
IOError = IOError
BlockingIOError = BlockingIOError
ChildProcessError = ChildProcessError
ConnectionError = ConnectionError
BrokenPipeError = BrokenPipeError
ConnectionAbortedError = ConnectionAbortedError
ConnectionRefusedError = ConnectionRefusedError
ConnectionResetError = ConnectionResetError
FileExistsError = FileExistsError
OverflowError = OverflowError
PermissionError = PermissionError
FileNotFoundError = FileNotFoundError
InterruptedError = InterruptedError
IsADirectoryError = IsADirectoryError
NotADirectoryError = NotADirectoryError
RecursionError = RecursionError
ReferenceError = ReferenceError
RuntimeError = RuntimeError
StopIteration = StopIteration
StopAsyncIteration = StopAsyncIteration
SyntaxError = SyntaxError
IndentationError = IndentationError
TabError = TabError
SystemError = SystemError
SystemExit = SystemExit
TimeoutError = TimeoutError
ProcessLookupError = ProcessLookupError
TypeError = TypeError
UnicodeError = UnicodeError
UnicodeDecodeError = UnicodeDecodeError
UnicodeEncodeError = UnicodeEncodeError
UnicodeTranslateError = UnicodeTranslateError
ValueError = ValueError
ZeroDivisionError = ZeroDivisionError
Warning = Warning
DeprecationWarning = DeprecationWarning
PendingDeprecationWarning = PendingDeprecationWarning
RuntimeWarning = RuntimeWarning
SyntaxWarning = SyntaxWarning
UserWarning = UserWarning
FutureWarning = FutureWarning
ImportWarning = ImportWarning
UnicodeWarning = UnicodeWarning
BytesWarning = BytesWarning
ResourceWarning = ResourceWarning
EncodingWarning = EncodingWarning

_molt_getargv = _require_builtin_intrinsic("molt_getargv")
_molt_getframe = _require_builtin_intrinsic("molt_getframe")
_molt_trace_enter_slot = _require_builtin_intrinsic("molt_trace_enter_slot")
_molt_trace_exit = _require_builtin_intrinsic("molt_trace_exit")
_molt_getrecursionlimit = _require_builtin_intrinsic("molt_getrecursionlimit")
_molt_setrecursionlimit = _require_builtin_intrinsic("molt_setrecursionlimit")
_molt_sys_version_info = _require_builtin_intrinsic("molt_sys_version_info")
_molt_sys_version = _require_builtin_intrinsic("molt_sys_version")
_molt_sys_stdin = _require_builtin_intrinsic("molt_sys_stdin")
_molt_sys_stdout = _require_builtin_intrinsic("molt_sys_stdout")
_molt_sys_stderr = _require_builtin_intrinsic("molt_sys_stderr")
_molt_exception_last = _require_builtin_intrinsic("molt_exception_last")
_molt_exception_active = _require_builtin_intrinsic("molt_exception_active")
_molt_asyncgen_hooks_get = _require_builtin_intrinsic("molt_asyncgen_hooks_get")
_molt_asyncgen_hooks_set = _require_builtin_intrinsic("molt_asyncgen_hooks_set")
_molt_asyncgen_locals = _require_builtin_intrinsic("molt_asyncgen_locals")
_molt_module_new = _require_builtin_intrinsic("molt_module_new")
_molt_function_set_builtin = _require_builtin_intrinsic("molt_function_set_builtin")
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
_molt_class_new = _require_builtin_intrinsic("molt_class_new")
_molt_class_set_base = _require_builtin_intrinsic("molt_class_set_base")
_molt_class_apply_set_name = _require_builtin_intrinsic("molt_class_apply_set_name")
_molt_sys_platform = _require_builtin_intrinsic("molt_sys_platform")
if _molt_sys_platform() == "win32":
    WindowsError = OSError
_molt_getpid = _require_builtin_intrinsic("molt_getpid")
_molt_getcwd = _require_builtin_intrinsic("molt_getcwd")
