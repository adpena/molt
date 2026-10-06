"""Canonical Python value identities, independent of shape and storage custody.

Unknown shape does not erase a known runtime identity. Value producers own
identities; selectors transport them and never infer them from shape.
"""

from __future__ import annotations

from enum import IntFlag
from typing import Final, TypeAlias


class PythonIdentity(IntFlag):
    """Compiler-relevant runtime identities.

    ``OTHER`` and ``UNBOUND`` are alternatives, not identities.  A fact is exact
    only when it contains one non-sentinel bit and neither sentinel.
    """

    IMPORTLIB_MODULE = 1 << 0
    IMPORTLIB_IMPORT_MODULE = 1 << 1
    IMPORTLIB_MACHINERY_MODULE = 1 << 2
    MODULE_SPEC_CLASS = 1 << 3
    MODULE_SPEC_INSTANCE = 1 << 4
    BUILTINS_MODULE = 1 << 5
    BUILTINS_IMPORT = 1 << 6
    SYS_MODULE = 1 << 7
    SYS_MODULES = 1 << 8
    INSPECT_MODULE = 1 << 9
    INSPECT_CURRENTFRAME = 1 << 10
    CURRENT_MODULE = 1 << 11
    CURRENT_GLOBALS = 1 << 12
    CURRENT_LOCALS = 1 << 13
    CURRENT_FRAME = 1 << 14
    BUILTIN_GLOBALS = 1 << 15
    BUILTIN_LOCALS = 1 << 16
    BUILTIN_VARS = 1 << 17
    BUILTIN_SETATTR = 1 << 18
    BUILTIN_EVAL = 1 << 19
    BUILTIN_EXEC = 1 << 20
    USER_FUNCTION = 1 << 21
    USER_CLASS = 1 << 22
    INERT_VALUE = 1 << 23
    IMPORTLIB_UTIL_MODULE = 1 << 24
    IMPORTLIB_FIND_SPEC = 1 << 25
    TYPING_MODULE = 1 << 26
    STATIC_FALSE = 1 << 27
    INTRINSICS_MODULE = 1 << 28
    INTRINSICS_REQUIRE = 1 << 29
    OTHER = 1 << 30
    UNBOUND = 1 << 31
    GLOBALS_SETITEM = 1 << 32
    GLOBALS_DELITEM = 1 << 33
    BUILTIN_BOOL = 1 << 34
    BUILTIN_INT = 1 << 35
    BUILTIN_FLOAT = 1 << 36
    BUILTIN_COMPLEX = 1 << 37
    BUILTIN_STR = 1 << 38
    BUILTIN_BYTES = 1 << 39
    BUILTIN_BYTEARRAY = 1 << 40
    BUILTIN_TUPLE = 1 << 41
    BUILTIN_LIST = 1 << 42
    BUILTIN_SET = 1 << 43
    BUILTIN_FROZENSET = 1 << 44
    BUILTIN_DICT = 1 << 45
    BUILTIN_RANGE = 1 << 46
    BUILTIN_LEN = 1 << 47
    BUILTIN_OPEN = 1 << 48


IdentityMask: TypeAlias = int
NO_IDENTITIES: Final[IdentityMask] = 0
OTHER_IDENTITY: Final[IdentityMask] = int(PythonIdentity.OTHER)
UNBOUND_IDENTITY: Final[IdentityMask] = int(PythonIdentity.UNBOUND)
UNKNOWN_IDENTITY: Final[IdentityMask] = OTHER_IDENTITY | UNBOUND_IDENTITY
_SENTINEL_IDENTITIES: Final[IdentityMask] = OTHER_IDENTITY | UNBOUND_IDENTITY


def exact_identity(identity: PythonIdentity) -> IdentityMask:
    return int(identity)


def possible_identity(identity: PythonIdentity) -> IdentityMask:
    return int(identity) | OTHER_IDENTITY


def identity_fact_is_exact(mask: IdentityMask, identity: PythonIdentity) -> bool:
    return mask == int(identity)


def identity_fact_may_be(mask: IdentityMask, identity: PythonIdentity) -> bool:
    return bool(mask & int(identity))


def identity_fact_is_proven(mask: IdentityMask) -> bool:
    known = mask & ~_SENTINEL_IDENTITIES
    return not (mask & _SENTINEL_IDENTITIES) and known.bit_count() == 1


def identity_fact_names(mask: IdentityMask) -> tuple[str, ...]:
    return tuple(
        identity.name.lower() for identity in PythonIdentity if mask & int(identity)
    )
