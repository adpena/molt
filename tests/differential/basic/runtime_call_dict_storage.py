"""STORE_GLOBAL uses dict storage, bypassing subclass __setitem__."""

import builtins
from types import FunctionType


class Namespace(dict):
    def __setitem__(self, key, value):
        raise AssertionError("subclass assignment hook")


def store_global():
    global marker
    marker = "stored"
    return marker


namespace = Namespace(__builtins__=builtins.__dict__)
try:
    namespace["control"] = 1
except AssertionError as error:
    print(str(error))
else:
    raise AssertionError("the control must invoke __setitem__")

dict.__setitem__(namespace, "direct", "storage")
print(namespace["direct"])
writer = FunctionType(store_global.__code__, namespace)
print(writer())
print(namespace.get("marker"))
assert "control" not in namespace
