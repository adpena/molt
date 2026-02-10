"""Importable builtins for Molt.

Bind supported builtins to module globals so `import builtins` works in
compiled code without introducing dynamic indirection.
"""

from __future__ import annotations

import sys as _sys
import types as _types  # noqa: F401

from _intrinsics import require_intrinsic as _require_intrinsic

TYPE_CHECKING = False


def cast(_type, value):
    return value


if TYPE_CHECKING:
    from typing import Any, Callable, Optional
else:

    class _TypingAlias:
        __slots__ = ()

        def __getitem__(self, _item):
            return self


Any = object()
Callable = _TypingAlias()
Optional = _TypingAlias()

_MOLT_CLASSMETHOD_NEW = _require_intrinsic("molt_classmethod_new", globals())
_MOLT_STATICMETHOD_NEW = _require_intrinsic("molt_staticmethod_new", globals())
_MOLT_PROPERTY_NEW = _require_intrinsic("molt_property_new", globals())
_MOLT_MODULE_IMPORT = _require_intrinsic("molt_module_import", globals())


def _molt_descriptor_types():
    def _probe():
        return None

    classmethod_type = type(_MOLT_CLASSMETHOD_NEW(_probe))
    staticmethod_type = type(_MOLT_STATICMETHOD_NEW(_probe))
    property_type = type(_MOLT_PROPERTY_NEW(None, None, None))
    return classmethod_type, staticmethod_type, property_type


classmethod, staticmethod, property = _molt_descriptor_types()


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
    return (
        isinstance(module_dict, dict)
        and len(module_dict) == 1
        and "__name__" in module_dict
    )


def _recover_placeholder_module(resolved: str, placeholder: object):
    modules = getattr(_sys, "modules", {})
    previous = modules.pop(resolved, None)
    try:
        import importlib.util as _importlib_util

        spec = _importlib_util.find_spec(resolved, None)
        if spec is None:
            raise ImportError(f"No module named '{resolved}'")
        module = _importlib_util.module_from_spec(spec)
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


def _intrinsic_import(name, globals=None, locals=None, fromlist=(), level=0):
    if not name:
        raise ImportError("Empty module name")
    resolved = _resolve_import_name(name, globals, level) if level else name
    modules = getattr(_sys, "modules", {})
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
    mod = _MOLT_MODULE_IMPORT(resolved)
    if mod is None:
        raise ImportError(f"No module named '{resolved}'")
    if _is_placeholder_module(mod):
        mod = _recover_placeholder_module(resolved, mod)
    if fromlist:
        return mod
    top = resolved.split(".", 1)[0]
    return modules.get(top, mod)


_molt_import_impl = None
_molt_importer = _sys.modules.get("_molt_importer")
if _molt_importer is not None:
    _molt_import_impl = getattr(_molt_importer, "_molt_import", None)
if _molt_import_impl is None:
    __import__ = _intrinsic_import
else:
    __import__ = _molt_import_impl

if TYPE_CHECKING:
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
    _molt_os_name: Callable[[], str]
    _molt_sys_platform: Callable[[], str]
    _molt_time_monotonic: Callable[[], float]
    _molt_time_monotonic_ns: Callable[[], int]
    _molt_time_time: Callable[[], float]
    _molt_time_time_ns: Callable[[], int]
    _molt_getpid: Callable[[], int]
    _molt_getcwd: Callable[[], str]
    _molt_os_close: Callable[[object], object]
    _molt_os_dup: Callable[[object], object]
    _molt_os_get_inheritable: Callable[[object], object]
    _molt_os_set_inheritable: Callable[[object, object], object]
    _molt_os_urandom: Callable[[object], object]
    _molt_math_log: Callable[[object], object]
    _molt_math_log2: Callable[[object], object]
    _molt_math_exp: Callable[[object], object]
    _molt_math_sin: Callable[[object], object]
    _molt_math_cos: Callable[[object], object]
    _molt_math_acos: Callable[[object], object]
    _molt_math_lgamma: Callable[[object], object]
    _molt_struct_pack: Callable[[object, object], object]
    _molt_struct_unpack: Callable[[object, object], object]
    _molt_struct_calcsize: Callable[[object], object]
    _molt_codecs_decode: Callable[[object, object, object], object]
    _molt_codecs_encode: Callable[[object, object, object], object]
    _molt_deflate_raw: Callable[[object, object], object]
    _molt_inflate_raw: Callable[[object], object]
    _molt_env_get: Callable[..., object]
    _molt_env_snapshot: Callable[[], object]
    _molt_errno_constants: Callable[[], tuple[dict[str, int], dict[int, str]]]
    _molt_path_exists: Callable[[object], bool]
    _molt_path_listdir: Callable[[object], object]
    _molt_path_mkdir: Callable[[object], object]
    _molt_path_unlink: Callable[[object], None]
    _molt_path_rmdir: Callable[[object], None]
    _molt_path_chmod: Callable[[object, object], None]
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


def _require_builtin_intrinsic(name: str) -> Any:
    return _require_intrinsic(name, globals())


def compile(
    source: object,
    filename: object,
    mode: object,
    flags: int = 0,
    dont_inherit: bool = False,
    optimize: int = -1,
):
    intrinsic = _require_builtin_intrinsic("molt_compile_builtin")
    return intrinsic(source, filename, mode, flags, dont_inherit, optimize)


# TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:missing): implement eval/exec builtins with sandboxing rules.
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
    "CancelledError",
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
try:
    complex = complex
except NameError:
    try:
        complex = type(0j)
    except Exception:
        pass
int = int
bool = bool
round = round
set = set
frozenset = frozenset
bytes = bytes
bytearray = bytearray
memoryview = memoryview
iter = iter
map = map
filter = filter
zip = zip
reversed = reversed
next = next
aiter = aiter
anext = anext
getattr = getattr
setattr = setattr
delattr = delattr
hasattr = hasattr
super = super
print = print
vars = vars
Ellipsis = ...
# Avoid bootstrap-time global lookup of NotImplemented in runtimes where builtins
# are still being initialized; rich-compare returns the singleton directly.
NotImplemented = object.__eq__(object(), object())
BaseException = BaseException
BaseExceptionGroup = BaseExceptionGroup
Exception = Exception
ExceptionGroup = ExceptionGroup


class CancelledError(BaseException):
    pass


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

WindowsError = OSError

_molt_getargv = cast(
    Callable[[], list[str]], _require_builtin_intrinsic("molt_getargv")
)
_molt_getframe = cast(
    Callable[[object], object], _require_builtin_intrinsic("molt_getframe")
)
_molt_trace_enter_slot = cast(
    Callable[[int], object], _require_builtin_intrinsic("molt_trace_enter_slot")
)
_molt_trace_exit = cast(
    Callable[[], object], _require_builtin_intrinsic("molt_trace_exit")
)
_molt_getrecursionlimit = cast(
    Callable[[], int],
    _require_builtin_intrinsic("molt_getrecursionlimit"),
)
_molt_setrecursionlimit = cast(
    Callable[[int], None],
    _require_builtin_intrinsic("molt_setrecursionlimit"),
)
_molt_sys_version_info = cast(
    Callable[[], tuple[int, int, int, str, int]],
    _require_builtin_intrinsic("molt_sys_version_info"),
)
_molt_sys_version = cast(
    Callable[[], str], _require_builtin_intrinsic("molt_sys_version")
)
_molt_sys_stdin = cast(
    Callable[[], object], _require_builtin_intrinsic("molt_sys_stdin")
)
_molt_sys_stdout = cast(
    Callable[[], object], _require_builtin_intrinsic("molt_sys_stdout")
)
_molt_sys_stderr = cast(
    Callable[[], object], _require_builtin_intrinsic("molt_sys_stderr")
)
_molt_exception_last = cast(
    Callable[[], Optional[BaseException]],
    _require_builtin_intrinsic("molt_exception_last"),
)
_molt_exception_active = cast(
    Callable[[], Optional[BaseException]],
    _require_builtin_intrinsic("molt_exception_active"),
)
_molt_asyncgen_hooks_get = cast(
    Callable[[], object],
    _require_builtin_intrinsic("molt_asyncgen_hooks_get"),
)
_molt_asyncgen_hooks_set = cast(
    Callable[[object, object], object],
    _require_builtin_intrinsic("molt_asyncgen_hooks_set"),
)
_molt_asyncgen_locals = cast(
    Callable[[object], object],
    _require_builtin_intrinsic("molt_asyncgen_locals"),
)
_molt_module_new = cast(
    Callable[[object], object], _require_builtin_intrinsic("molt_module_new")
)
_molt_function_set_builtin = cast(
    Callable[[object], object],
    _require_builtin_intrinsic("molt_function_set_builtin"),
)
_molt_class_new = cast(
    Callable[[object], object], _require_builtin_intrinsic("molt_class_new")
)
_molt_class_set_base = cast(
    Callable[[object, object], object],
    _require_builtin_intrinsic("molt_class_set_base"),
)
_molt_class_apply_set_name = cast(
    Callable[[object], object],
    _require_builtin_intrinsic("molt_class_apply_set_name"),
)
_molt_os_name = cast(Callable[[], str], _require_builtin_intrinsic("molt_os_name"))
_molt_sys_platform = cast(
    Callable[[], str], _require_builtin_intrinsic("molt_sys_platform")
)
_molt_time_monotonic = cast(
    Callable[[], float], _require_builtin_intrinsic("molt_time_monotonic")
)
_molt_time_monotonic_ns = cast(
    Callable[[], int], _require_builtin_intrinsic("molt_time_monotonic_ns")
)
_molt_time_time = cast(
    Callable[[], float], _require_builtin_intrinsic("molt_time_time")
)
_molt_time_time_ns = cast(
    Callable[[], int], _require_builtin_intrinsic("molt_time_time_ns")
)
_molt_getpid = cast(Callable[[], int], _require_builtin_intrinsic("molt_getpid"))
_molt_getcwd = cast(Callable[[], str], _require_builtin_intrinsic("molt_getcwd"))
_molt_env_get = cast(Callable[..., object], _require_builtin_intrinsic("molt_env_get"))
_molt_env_snapshot = cast(
    Callable[[], object], _require_builtin_intrinsic("molt_env_snapshot")
)
_molt_errno_constants = cast(
    Callable[[], tuple[dict[str, int], dict[int, str]]],
    _require_builtin_intrinsic("molt_errno_constants"),
)
_molt_path_exists = cast(
    Callable[[object], bool],
    _require_builtin_intrinsic("molt_path_exists"),
)
_molt_path_listdir = cast(
    Callable[[object], object],
    _require_builtin_intrinsic("molt_path_listdir"),
)
_molt_path_mkdir = cast(
    Callable[[object], object],
    _require_builtin_intrinsic("molt_path_mkdir"),
)
_molt_path_unlink = cast(
    Callable[[object], None],
    _require_builtin_intrinsic("molt_path_unlink"),
)
_molt_path_rmdir = cast(
    Callable[[object], None],
    _require_builtin_intrinsic("molt_path_rmdir"),
)
_molt_path_chmod = cast(
    Callable[[object, object], None],
    _require_builtin_intrinsic("molt_path_chmod"),
)
_molt_os_close = cast(
    Callable[[object], object], _require_builtin_intrinsic("molt_os_close")
)
_molt_os_dup = cast(
    Callable[[object], object], _require_builtin_intrinsic("molt_os_dup")
)
_molt_os_get_inheritable = cast(
    Callable[[object], object],
    _require_builtin_intrinsic("molt_os_get_inheritable"),
)
_molt_os_set_inheritable = cast(
    Callable[[object, object], object],
    _require_builtin_intrinsic("molt_os_set_inheritable"),
)
_molt_os_urandom = cast(
    Callable[[object], object], _require_builtin_intrinsic("molt_os_urandom")
)
_molt_math_log = cast(
    Callable[[object], object], _require_builtin_intrinsic("molt_math_log")
)
_molt_math_log2 = cast(
    Callable[[object], object], _require_builtin_intrinsic("molt_math_log2")
)
_molt_math_exp = cast(
    Callable[[object], object], _require_builtin_intrinsic("molt_math_exp")
)
_molt_math_sin = cast(
    Callable[[object], object], _require_builtin_intrinsic("molt_math_sin")
)
_molt_math_cos = cast(
    Callable[[object], object], _require_builtin_intrinsic("molt_math_cos")
)
_molt_math_acos = cast(
    Callable[[object], object], _require_builtin_intrinsic("molt_math_acos")
)
_molt_math_lgamma = cast(
    Callable[[object], object], _require_builtin_intrinsic("molt_math_lgamma")
)
_molt_struct_pack = cast(
    Callable[[object, object], object], _require_builtin_intrinsic("molt_struct_pack")
)
_molt_struct_unpack = cast(
    Callable[[object, object], object], _require_builtin_intrinsic("molt_struct_unpack")
)
_molt_struct_calcsize = cast(
    Callable[[object], object], _require_builtin_intrinsic("molt_struct_calcsize")
)
_molt_codecs_decode = cast(
    Callable[[object, object, object], object],
    _require_builtin_intrinsic("molt_codecs_decode"),
)
_molt_codecs_encode = cast(
    Callable[[object, object, object], object],
    _require_builtin_intrinsic("molt_codecs_encode"),
)
_molt_deflate_raw = cast(
    Callable[[object, object], object], _require_builtin_intrinsic("molt_deflate_raw")
)
_molt_inflate_raw = cast(
    Callable[[object], object], _require_builtin_intrinsic("molt_inflate_raw")
)
