"""Helper module for cross_module_inline_global.

Defines classes/functions whose trivially-inlinable bodies reference THIS
module's globals (a module-level constant, a constant dict, an __all__-adjacent
helper function, and a module-level intrinsic-style binding). When such a method
is devirtualized + inlined at a call site compiled in a DIFFERENT module, the
inlined body's bare-name reference to one of these globals must NOT mis-resolve
against the caller's module (the regression this fixture pins).
"""

__all__ = ["Widget", "Lookup", "Greeter", "scale"]

_SCALE = 100
_TABLE = {"a": 1, "b": 2, "c": 3}
_PREFIX = "Hello, "


def _decorate(name: str) -> str:
    # An __all__-adjacent module helper read from an inlinable method body.
    return _PREFIX + name


def scale(value: int) -> int:
    # A free function whose inlinable-style body reads a module global.
    return value * _SCALE


class Widget:
    def __init__(self, base: int) -> None:
        self.base = base

    def scaled(self) -> int:
        # Single-return inlinable body reading the module-level constant _SCALE.
        return self.base * _SCALE


class Lookup:
    def __init__(self, key: str) -> None:
        # __init__ body reading a module-global constant dict (the inline-init
        # path's cross-module gate).
        self.value = _TABLE[key]

    def get(self) -> int:
        return self.value


class Greeter:
    def __init__(self, name: str) -> None:
        self.name = name

    def greet(self) -> str:
        # Inlinable body that calls an __all__-adjacent module helper, which in
        # turn reads another module global (_PREFIX).
        return _decorate(self.name)
