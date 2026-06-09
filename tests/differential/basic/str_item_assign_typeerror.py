"""Purpose: differential coverage for item-assign / item-del TypeError on
immutable builtins (str / bytes / set / frozenset).

Regression for #52: `s = "hello"; s[0] = "H"` silently succeeded under molt
(no error, data unchanged) instead of raising the CPython
`TypeError: 'str' object does not support item assignment`. The same silent
no-op affected the store-index path for every immutable / non-subscript-
assignable builtin, and its deletion twin (`del s[0]`). CPython messages and
the index-vs-slice wording asymmetry on the delete path are version-stable
across 3.12 / 3.13 / 3.14.

Set / frozenset repr ordering is intentionally not exercised here (a separate
concern); only the TypeError surface is asserted, plus the str/bytes post-state
to prove the rejected mutation left the object unchanged.
"""


def rejects(action, label):
    try:
        action()
        print(f"{label}:NO_ERROR")
    except TypeError as exc:
        print(f"{label}:{exc}")


def assign_item(obj):
    obj[0] = 88


def assign_slice(obj):
    obj[0:1] = obj


def del_item(obj):
    del obj[0]


def del_slice(obj):
    del obj[0:1]


makers = [
    (lambda: "hello", "str"),
    (lambda: b"hello", "bytes"),
    (lambda: {1, 2, 3}, "set"),
    (lambda: frozenset({1, 2, 3}), "frozenset"),
]

for make, label in makers:
    rejects(lambda obj=make(): assign_item(obj), f"{label}_assign_item")
    rejects(lambda obj=make(): assign_slice(obj), f"{label}_assign_slice")
    rejects(lambda obj=make(): del_item(obj), f"{label}_del_item")
    rejects(lambda obj=make(): del_slice(obj), f"{label}_del_slice")

# The primary #52 repro, verbatim, including post-state (deterministic repr).
s = "hello"
try:
    s[0] = "H"
    print("repro_52:NO_ERROR")
except TypeError as exc:
    print(f"repro_52:{exc}")
print(f"repro_52_value:{s}")

b = b"hello"
try:
    b[0] = 72
    print("repro_bytes:NO_ERROR")
except TypeError as exc:
    print(f"repro_bytes:{exc}")
print(f"repro_bytes_value:{b!r}")
