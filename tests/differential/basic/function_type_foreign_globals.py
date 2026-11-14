"""FunctionType runs existing code over another globals mapping.

CPython oracle: Objects/funcobject.c func_new_impl stores the supplied globals,
and LOAD_GLOBAL reads that mapping and its __builtins__. A program that reads a
code object can rebind any function this way, so its module globals cannot be
compile-time cells (call_anatomy.md 4.5a, the code_rebinding premise). The
differential runner supplies matching CPython target expectations.
"""

import types

scale = 1


def read_global():
    return scale + len("ab")


class Counter:
    step = 3

    def advance(self, value):
        return value * scale + Counter.step


foreign = {"scale": 10, "Counter": Counter, "__builtins__": __builtins__}
rebound = types.FunctionType(read_global.__code__, foreign)
print("own globals", read_global())
print("foreign globals", rebound())

foreign["len"] = lambda value: 100
print("foreign builtin shadow", rebound())

scale = 2
print("own update", read_global(), "foreign unchanged", rebound())

method = types.FunctionType(Counter.advance.__code__, foreign)
print("method code over foreign globals", method(Counter(), 5), Counter().advance(5))

del foreign["scale"]
try:
    rebound()
except NameError as error:
    print("foreign NameError", error)

print("code identity kept", rebound.__code__ is read_global.__code__)
print("globals identity", rebound.__globals__ is foreign)
