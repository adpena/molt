"""Purpose: differential coverage for inspect signature bound."""

import inspect


class C:
    def method(self, a, b=1):
        return a + b


c = C()
print(str(inspect.signature(C.method)))
print(str(inspect.signature(c.method)))
