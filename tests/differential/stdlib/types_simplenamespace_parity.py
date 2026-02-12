"""Purpose: differential coverage for SimpleNamespace parity."""

import types


ns = types.SimpleNamespace(a=2)
print("value", ns.a)

print("repr", repr(types.SimpleNamespace(b=2, a=1)))

class C:
    def __init__(self) -> None:
        self.a = 1

print("eq_other", types.SimpleNamespace(a=1) == C())

try:
    types.SimpleNamespace({"a": 1})
except Exception as exc:
    print("positional", type(exc).__name__)
