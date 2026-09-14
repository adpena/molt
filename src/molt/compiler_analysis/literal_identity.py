"""Exact immutable literal value identity for compiler facts and cache keys.

This is not Python equality or object identity. Numeric types and floating-point
bits remain distinct; mutable objects and user-defined subclasses are not value
constants. Recursive keys therefore never invoke user equality/hash callbacks.
"""

from __future__ import annotations

import struct


def literal_identity_key(value: object) -> tuple[object, ...] | None:
    """Return a hashable exact value key, or None when no immutable fact exists."""
    kind = type(value)
    if value is None:
        return ("none",)
    if value is Ellipsis:
        return ("ellipsis",)
    if value is NotImplemented:
        return ("not_implemented",)
    if kind in (bool, int, str, bytes):
        return (kind.__name__, value)
    if kind is float:
        assert isinstance(value, float)
        return ("float", struct.pack("!d", value))
    if kind is complex:
        assert isinstance(value, complex)
        return ("complex", struct.pack("!dd", value.real, value.imag))
    if kind is tuple or kind is frozenset:
        assert isinstance(value, (tuple, frozenset))
        keys: list[tuple[object, ...]] = []
        for item in value:
            key = literal_identity_key(item)
            if key is None:
                return None
            keys.append(key)
        if kind is tuple:
            return ("tuple", tuple(keys))
        # A frozenset may contain multiple distinct NaNs with identical bits.
        # Preserve multiplicity rather than collapsing their equal value keys.
        counts: dict[tuple[object, ...], int] = {}
        for key in keys:
            counts[key] = counts.get(key, 0) + 1
        return ("frozenset", frozenset(counts.items()))
    if kind is range:
        assert isinstance(value, range)
        return ("range", value.start, value.stop, value.step)
    return None


def same_literal_value(left: object, right: object) -> bool:
    """Compare immutable value facts without Python's cross-type equality."""
    key = literal_identity_key(left)
    return key is not None and key == literal_identity_key(right)
