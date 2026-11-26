"""Canonical Python value identities, independent of shape and storage custody.

Unknown shape does not erase a known runtime identity. Value producers own
identities; selectors transport them and never infer them from shape.
"""

from __future__ import annotations


from molt.compiler_analysis import python_identity_catalog_generated as _catalog
from molt.compiler_analysis.python_string_facts import PythonStringBound


SOURCE_ITERATION_CALLABLES = frozenset(
    {
        _catalog.PythonIdentity.BUILTIN_ENUMERATE,
        _catalog.PythonIdentity.BUILTIN_FILTER,
        _catalog.PythonIdentity.BUILTIN_ITER,
        _catalog.PythonIdentity.BUILTIN_MAP,
        _catalog.PythonIdentity.BUILTIN_NEXT,
        _catalog.PythonIdentity.BUILTIN_REVERSED,
        _catalog.PythonIdentity.BUILTIN_ZIP,
        *_catalog.NATIVE_ITERATOR_PAYLOADS,
    }
)
NATIVE_SELECTOR_CONSTRUCTORS = frozenset(
    {
        _catalog.PythonIdentity.OPERATOR_ITEMGETTER,
        _catalog.PythonIdentity.OPERATOR_ATTRGETTER,
        _catalog.PythonIdentity.OPERATOR_METHODCALLER,
    }
)

SOURCE_CALLABLE_IDENTITIES = (
    _catalog.ALL_INTRINSIC_CALLABLE_IDENTITIES
    | _catalog.PUBLIC_RUNTIME_IMPORT_IDENTITIES
    | int(_catalog.PythonIdentity.BUILTIN_DICT)
    | sum(SOURCE_ITERATION_CALLABLES | NATIVE_SELECTOR_CONSTRUCTORS)
)


def possible_runtime_import_call_identities(
    identities: _catalog.IdentityMask, argument_count: int | None
) -> _catalog.IdentityMask:
    """Project canonical callees that can reach the import protocol.

    Intrinsic binding rejects too few arguments before its body executes.
    Unknown expansions preserve all possibilities. Public Python callables
    retain their existing conservative admission; their mutable hooks and
    descriptors have a distinct binding contract.
    """
    intrinsic = _catalog.INTRINSIC_RUNTIME_IMPORT_IDENTITIES
    if argument_count is not None:
        if argument_count < 0:
            raise ValueError("call argument count must be nonnegative")
        masks = _catalog.INTRINSIC_RUNTIME_IMPORT_MIN_ARGUMENT_MASKS
        intrinsic = masks[min(argument_count, len(masks) - 1)]
    return identities & (_catalog.PUBLIC_RUNTIME_IMPORT_IDENTITIES | intrinsic)


def exact_identity(identity: _catalog.PythonIdentity) -> _catalog.IdentityMask:
    return int(identity)


def possible_identity(identity: _catalog.PythonIdentity) -> _catalog.IdentityMask:
    return int(identity) | _catalog.OTHER_IDENTITY


# Subscriptions whose ``__class_getitem__`` is fixed CPython code that calls
# nothing on its operands: ``dataclasses.InitVar[T]`` builds an ``InitVar``, and
# a builtin container class builds a ``types.GenericAlias``.
_INERT_ALIAS_OWNERS: frozenset[_catalog.IdentityMask] = frozenset(
    int(identity)
    for identity in (
        _catalog.PythonIdentity.DATACLASSES_INITVAR,
        _catalog.PythonIdentity.BUILTIN_TUPLE,
        _catalog.PythonIdentity.BUILTIN_LIST,
        _catalog.PythonIdentity.BUILTIN_SET,
        _catalog.PythonIdentity.BUILTIN_FROZENSET,
        _catalog.PythonIdentity.BUILTIN_DICT,
        _catalog.PythonIdentity.BUILTIN_TYPE,
    )
)
_BUILTIN_CLASSES: frozenset[_catalog.IdentityMask] = frozenset(
    int(identity) for identity in _catalog.BUILTIN_SHAPE_IDENTITIES.values()
)


def subscription_is_inert_alias(
    owner: _catalog.IdentityMask,
    index: _catalog.IdentityMask,
    static_index: object,
) -> bool:
    """Whether ``owner[index]`` runs no Python a program can supply.

    ``typing.ClassVar[X]`` runs ``typing._type_check(X)``, which compares X
    with ``==``, so it is inert only for an exact builtin class or a string.
    """
    if owner in _INERT_ALIAS_OWNERS:
        return True
    return owner == int(_catalog.PythonIdentity.TYPING_CLASSVAR) and (
        type(static_index) is str or index in _BUILTIN_CLASSES
    )


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


def canonical_import_binding_write_mask(
    key: str | PythonStringBound | None,
) -> _catalog.MemberMask:
    """A cache entry can redirect only acquisitions of that provider.

    Entry mutation does not replace the sys.modules object. An exact key
    invalidates canonical acquisition when it names a canonical provider, a
    bounded key when its bound admits one, an unbounded key always. This is
    the binding flow's flow-sensitive canonical fact; which rows a program can
    redirect is ``ImportRedirects`` (bedrock §13, M7).
    """
    if key is None:
        return int(_catalog.PythonMember.IMPORT_HOOKS)
    if isinstance(key, str):
        canonical = key in _catalog.CANONICAL_IMPORT_IDENTITIES
    else:
        canonical = any(
            key.admits(name) for name in _catalog.CANONICAL_IMPORT_IDENTITIES
        )
    return (
        int(_catalog.PythonMember.IMPORT_HOOKS)
        if canonical
        else _catalog.NO_INVALID_MEMBERS
    )
