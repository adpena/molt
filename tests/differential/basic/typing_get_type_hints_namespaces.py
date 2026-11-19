"""get_type_hints resolves string annotations through the right namespaces.

A function's names resolve through its globals, a class's through its
module, then builtins; a name nobody binds raises NameError. The lookups are
namespace reads (as evaluation reads globals and ``__builtins__``), never a
module ``__getattr__``.
"""

from __future__ import annotations

import typing
from typing import get_type_hints

Alias = int


def function(a: Alias, b: str, c: Later, d: typing.List[int]) -> bool:
    return True


class Holder:
    count: Alias
    name: str
    nested: Later

    def method(self, x: Alias) -> Holder:
        return self


class Later:
    pass


def broken(a: NowhereDefined) -> None:
    pass


def show(hints):
    return {k: getattr(v, "__name__", repr(v)) for k, v in hints.items()}


print("function", show(get_type_hints(function)))
print("class", show(get_type_hints(Holder)))
print("method", show(get_type_hints(Holder.method)))
print("bound method", show(get_type_hints(Holder().method)))
try:
    get_type_hints(broken)
except NameError as exc:
    print("broken", type(exc).__name__, exc)
