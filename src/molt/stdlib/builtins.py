"""Importable builtins for Molt.

Bind supported builtins to module globals so `import builtins` works in
compiled code without introducing dynamic indirection.
"""

from __future__ import annotations

import builtins as _py_builtins
from typing import TYPE_CHECKING, Any, Callable, Optional, cast


def _load_intrinsic(name: str) -> Any | None:
    direct = globals().get(name)
    if direct is not None:
        return direct
    return getattr(_py_builtins, name, None)


if TYPE_CHECKING:
    _molt_getargv: Callable[[], list[str]]
    _molt_getrecursionlimit: Callable[[], int]
    _molt_setrecursionlimit: Callable[[int], None]
    _molt_sys_version_info: Callable[[], tuple[int, int, int, str, int]]
    _molt_sys_version: Callable[[], str]
    _molt_exception_last: Callable[[], Optional[BaseException]]
    _molt_exception_active: Callable[[], Optional[BaseException]]
    _molt_asyncgen_hooks_get: Callable[[], object]
    _molt_asyncgen_hooks_set: Callable[[object, object], object]
    _molt_asyncgen_locals: Callable[[object], object]
    _molt_module_new: Callable[[object], object]
    _molt_function_set_builtin: Callable[[object], object]
    _molt_class_new: Callable[[object], object]
    _molt_class_set_base: Callable[[object, object], object]
    _molt_class_apply_set_name: Callable[[object], object]
    _molt_getpid: Callable[[], int]
    _molt_env_get_raw: Callable[..., object]
    _molt_errno_constants: Callable[[], tuple[dict[str, int], dict[int, str]]]
    _molt_path_exists: Callable[[object], bool]
    _molt_path_unlink: Callable[[object], None]


# TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:missing): implement eval/exec/compile builtins with sandboxing rules.
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
Ellipsis = Ellipsis
NotImplemented = NotImplemented
BaseException = BaseException
BaseExceptionGroup = BaseExceptionGroup
Exception = Exception
ExceptionGroup = ExceptionGroup
try:
    _cancelled_error = getattr(_py_builtins, "CancelledError")
except AttributeError:
    _cancelled_error = None

if _cancelled_error is None:

    class CancelledError(_py_builtins.BaseException):
        pass

else:
    CancelledError = _cancelled_error


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

WindowsError = getattr(_py_builtins, "WindowsError", OSError)

_molt_getargv = cast(Callable[[], list[str]], _load_intrinsic("_molt_getargv"))
_molt_getrecursionlimit = cast(
    Callable[[], int],
    _load_intrinsic("_molt_getrecursionlimit"),
)
_molt_setrecursionlimit = cast(
    Callable[[int], None],
    _load_intrinsic("_molt_setrecursionlimit"),
)
_molt_sys_version_info = cast(
    Callable[[], tuple[int, int, int, str, int]],
    _load_intrinsic("_molt_sys_version_info"),
)
_molt_sys_version = cast(Callable[[], str], _load_intrinsic("_molt_sys_version"))
_molt_exception_last = cast(
    Callable[[], Optional[BaseException]],
    _load_intrinsic("_molt_exception_last"),
)
_molt_exception_active = cast(
    Callable[[], Optional[BaseException]],
    _load_intrinsic("_molt_exception_active"),
)
_molt_asyncgen_hooks_get = cast(
    Callable[[], object],
    _load_intrinsic("_molt_asyncgen_hooks_get"),
)
_molt_asyncgen_hooks_set = cast(
    Callable[[object, object], object],
    _load_intrinsic("_molt_asyncgen_hooks_set"),
)
_molt_asyncgen_locals = cast(
    Callable[[object], object],
    _load_intrinsic("_molt_asyncgen_locals"),
)
_molt_module_new = cast(Callable[[object], object], _load_intrinsic("_molt_module_new"))
_molt_function_set_builtin = cast(
    Callable[[object], object],
    _load_intrinsic("_molt_function_set_builtin"),
)
_molt_class_new = cast(Callable[[object], object], _load_intrinsic("_molt_class_new"))
_molt_class_set_base = cast(
    Callable[[object, object], object],
    _load_intrinsic("_molt_class_set_base"),
)
_molt_class_apply_set_name = cast(
    Callable[[object], object],
    _load_intrinsic("_molt_class_apply_set_name"),
)
_molt_getpid = cast(Callable[[], int], _load_intrinsic("_molt_getpid"))
_molt_env_get_raw = cast(Callable[..., object], _load_intrinsic("_molt_env_get_raw"))
_molt_errno_constants = cast(
    Callable[[], tuple[dict[str, int], dict[int, str]]],
    _load_intrinsic("_molt_errno_constants"),
)
_molt_path_exists = cast(
    Callable[[object], bool],
    _load_intrinsic("_molt_path_exists"),
)
_molt_path_unlink = cast(
    Callable[[object], None],
    _load_intrinsic("_molt_path_unlink"),
)
