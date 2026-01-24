"""Purpose: differential coverage for builtins basic."""

import builtins

print(isinstance(len, object))
print(callable(len))
print(hasattr(builtins, "len"))

vals = [None, True, 0, 1, "", "x", [], [1]]
print([bool(v) for v in vals])
