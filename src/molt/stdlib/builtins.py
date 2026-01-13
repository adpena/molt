"""Importable builtins for Molt.

Bind supported builtins to module globals so `import builtins` works in
compiled code without introducing dynamic indirection.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:

    def _molt_getrecursionlimit() -> int:
        return 0

    def _molt_setrecursionlimit(limit: int) -> None:
        return None


__all__ = [
    "object",
    "type",
    "isinstance",
    "issubclass",
    "len",
    "ord",
    "chr",
    "ascii",
    "bin",
    "oct",
    "hex",
    "abs",
    "divmod",
    "repr",
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
    "next",
    "aiter",
    "anext",
    "getattr",
    "setattr",
    "delattr",
    "hasattr",
    "super",
    "print",
]

object = object

type = type

isinstance = isinstance
issubclass = issubclass

len = len
ord = ord
chr = chr
ascii = ascii
bin = bin
oct = oct
hex = hex
abs = abs
divmod = divmod
repr = repr
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
next = next
aiter = aiter
anext = anext
getattr = getattr
setattr = setattr
delattr = delattr
hasattr = hasattr
super = super
print = print

try:
    _molt_getrecursionlimit = _molt_getrecursionlimit
    _molt_setrecursionlimit = _molt_setrecursionlimit
except NameError:
    pass
