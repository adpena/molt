"""Canonical Python value identities, independent of shape and storage custody.

Unknown shape does not erase a known runtime identity. Value producers own
identities; selectors transport them and never infer them from shape.
"""

from __future__ import annotations

import ast

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


def canonical_import_binding_write_mask(name: str | None) -> _catalog.MemberMask:
    """A cache entry can redirect only acquisitions of that provider.

    Entry mutation does not replace the sys.modules object. Unknown keys still
    invalidate every canonical acquisition; hook/object replacement has its
    separate, broader member-write contract.
    """
    return (
        int(_catalog.PythonMember.IMPORT_HOOKS)
        if name is None or name in _catalog.CANONICAL_IMPORT_IDENTITIES
        else _catalog.NO_INVALID_MEMBERS
    )


def canonical_import_binding_candidates(
    node: ast.expr | None,
) -> tuple[str | None, ...]:
    """Bound possible canonical keys without executing a key expression.

    JoinedStr preserves its literal fragments regardless of formatting or name
    rebinding. Prefix/suffix/length bounds may reject a provider but never prove
    a full value or callback freedom. Other computed expressions stay unknown.
    """
    if isinstance(node, ast.Constant) and type(node.value) is str:
        return (node.value,)
    if not isinstance(node, ast.JoinedStr):
        return (None,)
    if any(
        not isinstance(part, ast.FormattedValue)
        and not (isinstance(part, ast.Constant) and type(part.value) is str)
        for part in node.values
    ):
        return (None,)
    literals = [
        part.value
        for part in node.values
        if isinstance(part, ast.Constant) and isinstance(part.value, str)
    ]
    if len(literals) == len(node.values):
        return ("".join(literals),)
    first, last = node.values[0], node.values[-1]
    prefix = (
        first.value
        if isinstance(first, ast.Constant) and isinstance(first.value, str)
        else ""
    )
    suffix = (
        last.value
        if isinstance(last, ast.Constant) and isinstance(last.value, str)
        else ""
    )
    minimum = sum(map(len, literals))
    return tuple(
        name
        for name in _catalog.CANONICAL_IMPORT_IDENTITIES
        if len(name) >= minimum and name.startswith(prefix) and name.endswith(suffix)
    )
