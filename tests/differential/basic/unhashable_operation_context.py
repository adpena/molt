"""Purpose: from 3.14, a set or dict names its operation when hashing a key
raises an exact TypeError ("cannot use 'X' as a set element (<message>)"),
while a TypeError subclass, any other exception, the probe-only set
operations (intersection, issubset) and hash() keep the error as it is.
Each row separates the correct message from a plausible defect: a builtin
unhashable key wrapped but a user __hash__ error left bare, a subclass or a
ValueError wrapped, or a probe-only operation wrapped."""

class Custom:
    def __hash__(self):
        raise TypeError("custom")


class Sub(TypeError):
    pass


class RaisesSub:
    def __hash__(self):
        raise Sub("sub")


class RaisesValue:
    def __hash__(self):
        raise ValueError("value")


class Unhash:
    __hash__ = None


def operations(key):
    return {
        "add": lambda: set().add(key),
        "contains": lambda: key in {1},
        "discard": lambda: {1}.discard(key),
        "remove": lambda: {1}.remove(key),
        "set": lambda: set([key]),
        "update": lambda: set().update([key]),
        "isdisjoint": lambda: {1}.isdisjoint([key]),
        "issuperset": lambda: {1}.issuperset([key]),
        "issubset": lambda: {1}.issubset([key]),
        "intersection": lambda: {1}.intersection([key]),
        "difference": lambda: {1}.difference([key]),
        "union": lambda: {1}.union([key]),
        "frozenset_contains": lambda: key in frozenset({1}),
        "dict_set": lambda: {}.__setitem__(key, 1),
        "dict_get": lambda: {1: 2}.get(key),
        "dict_contains": lambda: key in {1: 2},
        "dict_literal": lambda: {key: 1},
        "fromkeys": lambda: dict.fromkeys([key]),
        "setdefault": lambda: {}.setdefault(key),
        "dict_getitem": lambda: {1: 2}[key],
        "hash": lambda: hash(key),
    }


for label, key in [
    ("custom", Custom()),
    ("list", []),
    ("unhash", Unhash()),
    ("subclass", RaisesSub()),
    ("value", RaisesValue()),
]:
    for name, operation in operations(key).items():
        try:
            operation()
            print(label, name, "ok")
        except Exception as exc:  # noqa: BLE001
            print(label, name, type(exc).__name__, exc)
