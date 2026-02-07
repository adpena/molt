"""Purpose: code object metadata parity for core inspect/types fields."""

import types


def add(a, b=1):
    return a + b


co = add.__code__
print(isinstance(co, types.CodeType))
print(co.co_argcount, co.co_posonlyargcount, co.co_kwonlyargcount)
print(co.co_freevars, co.co_cellvars)
print(isinstance(co.co_flags, int))
print((co.co_flags & 0x03) == 0x03)
