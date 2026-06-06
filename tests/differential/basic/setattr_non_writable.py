"""Purpose: setting/deleting an attribute on a target that cannot hold one must
raise a catchable AttributeError/TypeError (never crash), with CPython-exact text.

Covers the SETATTR/DELATTR-failure path for every target kind that has no
``__dict__`` and no matching slot: tagged scalars (int/str/float/bool/None),
tuple, a ``__slots__`` instance, and a builtin/immutable type. CPython 3.13+
appends ``and no __dict__ for setting new attributes`` to the SET/DEL message on
these targets; 3.12 omits it (the test is version-gated through the differential
harness's ``MOLT_PYTHON_VERSION``).

Regression for: ``typing.final(42)`` SIGSEGV (a tagged-int receiver flowing into
the direct ``set_attr_generic_ptr`` path, which unboxed the tag as a pointer and
dereferenced the object header), and the broader class of SETATTR-on-non-writable
panics.
"""


def set_attr(obj: object, label: str) -> None:
    try:
        obj.injected = 1  # type: ignore[attr-defined]
        print(label, "set: NO ERROR")
    except (AttributeError, TypeError) as exc:
        print(label, "set:", type(exc).__name__, exc)


def del_attr(obj: object, label: str) -> None:
    try:
        del obj.injected  # type: ignore[attr-defined]
        print(label, "del: NO ERROR")
    except (AttributeError, TypeError) as exc:
        print(label, "del:", type(exc).__name__, exc)


for value, name in (
    (42, "int"),
    ("s", "str"),
    (3.14, "float"),
    (True, "bool"),
    (None, "None"),
    ((1, 2), "tuple"),
    (b"bytes", "bytes"),
    (frozenset({1}), "frozenset"),
):
    set_attr(value, name)
    del_attr(value, name)


class Slotted:
    __slots__ = ("a",)


s = Slotted()
s.a = 5
print("slot write/read:", s.a)
set_attr(s, "slotted")
del_attr(s, "slotted")


class TwoSlots:
    __slots__ = ("a", "b")


t = TwoSlots()
t.a = 1
t.b = 2
print("two-slot read:", t.a, t.b)
set_attr(t, "two_slots")


# setting an attribute on an immutable builtin type raises TypeError.
try:
    int.injected = 1  # type: ignore[attr-defined]
    print("type int set: NO ERROR")
except (AttributeError, TypeError) as exc:
    print("type int set:", type(exc).__name__, exc)
