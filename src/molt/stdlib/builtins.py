"""Importable builtins for Molt.

Bind supported builtins to module globals so `import builtins` works in
compiled code without introducing dynamic indirection.
"""

from __future__ import annotations

__all__ = [
    "object",
    "type",
    "isinstance",
    "issubclass",
    "len",
    "str",
    "range",
    "slice",
    "list",
    "tuple",
    "float",
    "int",
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
str = str
range = range
slice = slice
list = list
tuple = tuple
float = float
int = int
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
