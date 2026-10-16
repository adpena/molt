"""Canonical Python value identities, independent of shape and storage custody.

Unknown shape does not erase a known runtime identity. Value producers own
identities; selectors transport them and never infer them from shape.
"""

from __future__ import annotations

from molt.compiler_analysis import python_identity_catalog_generated as _catalog


def exact_identity(identity: _catalog.PythonIdentity) -> _catalog.IdentityMask:
    return int(identity)


def possible_identity(identity: _catalog.PythonIdentity) -> _catalog.IdentityMask:
    return int(identity) | _catalog.OTHER_IDENTITY


def identity_fact_is_exact(
    mask: _catalog.IdentityMask, identity: _catalog.PythonIdentity
) -> bool:
    return mask == int(identity)


def identity_fact_may_be(
    mask: _catalog.IdentityMask, identity: _catalog.PythonIdentity
) -> bool:
    return bool(mask & int(identity))


def identity_fact_is_proven(mask: _catalog.IdentityMask) -> bool:
    known = mask & ~_catalog.SENTINEL_IDENTITIES
    return not (mask & _catalog.SENTINEL_IDENTITIES) and known.bit_count() == 1


def identity_fact_names(mask: _catalog.IdentityMask) -> tuple[str, ...]:
    return tuple(
        identity.name.lower()
        for identity in _catalog.PythonIdentity
        if mask & int(identity)
    )
