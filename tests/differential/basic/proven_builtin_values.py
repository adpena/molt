"""Proved builtin reads keep CPython identity, protocols and errors (HF-226).

No code in this program writes a builtin name, the builtins module or
``__builtins__``, so the closed-world binding flow proves every builtin read
exact and the compiler lowers it without a namespace load or guard.
"""

import builtins


class Meta(type):
    def __instancecheck__(cls, obj):
        return obj == "meta"

    def __subclasscheck__(cls, sub):
        return sub is int


class Probe(metaclass=Meta):
    pass


def identities():
    return (
        str is builtins.str,
        int is builtins.int,
        range is builtins.range,
        len is builtins.len,
        isinstance is builtins.isinstance,
        issubclass is builtins.issubclass,
    )


def count_strings(items):
    count = 0
    for i in range(len(items)):
        if isinstance(items[i], str):
            count += 1
    return count


def protocols():
    return (
        isinstance("meta", Probe),
        isinstance(1, Probe),
        issubclass(int, Probe),
        issubclass(str, Probe),
        isinstance(3, (str, int)),
        issubclass(bool, (str, int)),
        isinstance(True, int),
    )


def aliases():
    kind = str
    check = isinstance
    size = len
    return kind(5), check(1, int), size("abc"), type(kind).__name__, kind.__name__


def shapes():
    out = []
    for call in (
        lambda: len(),
        lambda: len(1, 2),
        lambda: isinstance(1),
        lambda: issubclass(1, int),
        lambda: range(),
        lambda: int("x"),
        lambda: str(b"x", "no-such-codec"),
    ):
        try:
            out.append(call())
        except (TypeError, ValueError, LookupError) as error:
            out.append(type(error).__name__)
    return out


def zero_trip(n):
    total = 0
    for i in range(n):
        total += isinstance(i, int)
    return total


def nested(values):
    return [isinstance(value, str) for value in values], sum(
        1 for value in values if isinstance(value, int)
    )


print("identities", identities())
print("count", count_strings(["a", 1, "b", 2.0, "c"]))
print("protocols", protocols())
print("aliases", aliases())
print("shapes", shapes())
print("zero-trip", zero_trip(0), zero_trip(5))
print("nested", nested(["a", 1, "b", 2]))
print("module-scope", isinstance("x", str), len("four"), str is builtins.str)
