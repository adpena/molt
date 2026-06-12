"""Purpose: differential coverage for Enum value-alias semantics.

Regression for #51: a second member bound to an already-used value must become
an ALIAS of the first (canonical) member, not a distinct member. Under molt
this previously produced `Color.CRIMSON is Color.RED == False`, name
'CRIMSON', and CRIMSON appearing in iteration / len — diverging from CPython,
where CRIMSON aliases RED (same object, name 'RED', excluded from iteration,
present in __members__ mapping to the canonical member, and `Color(1)` /
`Color['CRIMSON']` both return RED).

Covers: identity, name, value, iteration order, len, __members__ (ordered,
includes alias, read-only), value lookup, name lookup, membership, and
multiple interleaved aliases.
"""

from enum import Enum


class Color(Enum):
    RED = 1
    CRIMSON = 1
    GREEN = 2
    EMERALD = 2
    BLUE = 3


print("CRIMSON is RED:", Color.CRIMSON is Color.RED)
print("EMERALD is GREEN:", Color.EMERALD is Color.GREEN)
print("CRIMSON.name:", Color.CRIMSON.name)
print("EMERALD.name:", Color.EMERALD.name)
print("CRIMSON.value:", Color.CRIMSON.value)

print("iter names:", [m.name for m in Color])
print("iter values:", [m.value for m in Color])
print("len:", len(Color))
print("_member_names_:", Color._member_names_)

print("members keys:", list(Color.__members__.keys()))
print("members CRIMSON is RED:", Color.__members__["CRIMSON"] is Color.RED)
print("members EMERALD is GREEN:", Color.__members__["EMERALD"] is Color.GREEN)
print("members len:", len(Color.__members__))

# __members__ is a read-only proxy.
try:
    Color.__members__["X"] = Color.RED
    print("members mutable: NO_ERROR")
except TypeError as exc:
    print("members readonly:", exc)

print("Color(1).name:", Color(1).name)
print("Color(2).name:", Color(2).name)
print("Color(3).name:", Color(3).name)
print("Color['CRIMSON'].name:", Color["CRIMSON"].name)
print("Color['EMERALD'] is GREEN:", Color["EMERALD"] is Color.GREEN)

print("RED in Color:", Color.RED in Color)
print("CRIMSON in Color:", Color.CRIMSON in Color)
print("1 in Color:", 1 in Color)
print("99 in Color:", 99 in Color)

try:
    Color(99)
    print("Color(99): NO_ERROR")
except ValueError as exc:
    print("Color(99):", exc)

# str() of an alias resolves to the canonical name (repr formatting of Enum is
# a separate, pre-existing divergence tracked apart from #51's alias contract,
# so it is intentionally not asserted here).
print("str RED:", str(Color.RED))
print("str CRIMSON:", str(Color.CRIMSON))
print("equal:", Color.CRIMSON == Color.RED)
print("hash equal:", hash(Color.CRIMSON) == hash(Color.RED))


# Alias resolution must also hold when the first definition is an alias target
# referenced before later canonical members (definition order matters).
class Status(Enum):
    OK = 200
    FINE = 200
    OKAY = 200
    ERROR = 500


print("Status names:", [m.name for m in Status])
print("FINE is OK:", Status.FINE is Status.OK)
print("OKAY is OK:", Status.OKAY is Status.OK)
print("Status len:", len(Status))
print("Status members:", list(Status.__members__.keys()))
print("Status(200).name:", Status(200).name)

# `@unique` must detect aliases via __members__ (name != member.name), now that
# aliases are no longer distinct members. We assert the exception type and the
# alias-detail portion of the message; the leading `repr(cls)` is intentionally
# stripped because metaclass __repr__ dispatch for class objects is a separate,
# pre-existing molt limitation (general to any custom metaclass, not specific
# to enum or to #51's alias contract).
from enum import unique


@unique
class Distinct(Enum):
    A = 1
    B = 2


print("unique distinct:", [m.name for m in Distinct])

try:

    @unique
    class HasAlias(Enum):
        A = 1
        B = 1
        C = 1

    print("unique alias: NO_ERROR")
except ValueError as exc:
    detail = str(exc).split(": ", 1)[1]
    print("unique alias type:", type(exc).__name__)
    print("unique alias detail:", detail)
