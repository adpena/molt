"""Dense immutable facts for source-ordered Python binding analysis.

The representation is intentionally raw-mask based, matching
``python_effects_generated``: joins are integer ORs and facts are cheap to copy,
hash, and share across compiler consumers.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass
from enum import IntFlag
from types import MappingProxyType
from typing import TYPE_CHECKING, Final, Mapping, TypeAlias

from molt.compiler_analysis.python_effects_generated import EffectMask

if TYPE_CHECKING:
    from molt.compiler_analysis.python_imports import ModuleImportFlow


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
    OTHER = 1 << 30
    UNBOUND = 1 << 31


IdentityMask: TypeAlias = int
NO_IDENTITIES: Final[IdentityMask] = 0
OTHER_IDENTITY: Final[IdentityMask] = int(PythonIdentity.OTHER)
UNBOUND_IDENTITY: Final[IdentityMask] = int(PythonIdentity.UNBOUND)
UNKNOWN_IDENTITY: Final[IdentityMask] = OTHER_IDENTITY | UNBOUND_IDENTITY
_SENTINEL_IDENTITIES: Final[IdentityMask] = OTHER_IDENTITY | UNBOUND_IDENTITY


@dataclass(frozen=True, slots=True)
class PythonParameterRef:
    name: str


PythonStaticValue: TypeAlias = str | int | tuple[str, ...] | PythonParameterRef | None


class PythonMember(IntFlag):
    """Canonical object members whose identity can be invalidated by mutation."""

    IMPORTLIB_IMPORT_MODULE = 1 << 0
    IMPORTLIB_MACHINERY = 1 << 1
    MACHINERY_MODULE_SPEC = 1 << 2
    BUILTINS_IMPORT = 1 << 3
    SYS_MODULES = 1 << 4
    INSPECT_CURRENTFRAME = 1 << 5
    MODULE_SPEC_CLASS = 1 << 6
    IMPORT_HOOKS = 1 << 7
    IMPORTLIB_UTIL = 1 << 8
    UTIL_FIND_SPEC = 1 << 9
    TYPING_TYPE_CHECKING = 1 << 10


MemberMask: TypeAlias = int
NO_INVALID_MEMBERS: Final[MemberMask] = 0
ALL_INVALID_MEMBERS: Final[MemberMask] = sum(int(member) for member in PythonMember)


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
        identity.name.lower()
        for identity in PythonIdentity
        if mask & int(identity)
    )


@dataclass(frozen=True, slots=True, order=True)
class PythonNodeKey:
    """Stable source key; unlike ``id(ast_node)`` it survives reparsing/cache hits."""

    lineno: int
    col_offset: int
    end_lineno: int
    end_col_offset: int
    kind: str

    @classmethod
    def from_node(cls, node: ast.AST) -> PythonNodeKey:
        return cls(
            int(getattr(node, "lineno", 0)),
            int(getattr(node, "col_offset", 0)),
            int(getattr(node, "end_lineno", getattr(node, "lineno", 0))),
            int(getattr(node, "end_col_offset", getattr(node, "col_offset", 0))),
            type(node).__name__,
        )


@dataclass(frozen=True, slots=True)
class PythonExpressionFact:
    node: PythonNodeKey
    scope_id: int
    state_id: int
    identities: IdentityMask
    effects: EffectMask
    static_value: PythonStaticValue = None


@dataclass(frozen=True, slots=True)
class PythonCallSiteFact:
    node: PythonNodeKey
    scope_id: int
    state_id: int
    callee_identities: IdentityMask
    result_identities: IdentityMask
    effects: EffectMask
    maybe_invalidated_members_after: MemberMask
    definitely_invalidated_members_after: MemberMask

    def callee_is(self, identity: PythonIdentity) -> bool:
        return identity_fact_is_exact(self.callee_identities, identity)

    def callee_may_be(self, identity: PythonIdentity) -> bool:
        return identity_fact_may_be(self.callee_identities, identity)


@dataclass(frozen=True, slots=True)
class PythonScopeFact:
    scope_id: int
    parent_scope_id: int | None
    kind: str
    name: str
    local_names: tuple[str, ...]
    global_names: tuple[str, ...]
    nonlocal_names: tuple[str, ...]
    binding_slots: tuple[tuple[str, int], ...]
    entry_state_id: int
    exit_state_id: int


@dataclass(frozen=True, slots=True)
class PythonBindingState:
    """One interned persistent environment node.

    A single-parent node is an O(1) strong update.  A multi-parent node is a
    control-flow join.  This keeps memory linear in program size instead of
    copying every live binding at every source position.
    """

    parents: tuple[int, ...] = ()
    updated_slot: int = -1
    updated_value: IdentityMask = UNBOUND_IDENTITY
    updated_static_value: PythonStaticValue = None
    clean_slots: int = 0
    maybe_invalidated_members: MemberMask = 0
    definitely_invalidated_members: MemberMask = 0


@dataclass(frozen=True, slots=True)
class PythonBindingIndex:
    """Immutable query surface shared by import and frontend consumers."""

    source_digest: str
    target_python: tuple[int, int]
    target_sys_platform: str | None
    module_name: str | None
    module_spec_name: str | None
    module_is_package: bool
    module_execution_kind: str
    module_import_flow: ModuleImportFlow
    expressions: tuple[PythonExpressionFact, ...]
    calls: tuple[PythonCallSiteFact, ...]
    scopes: tuple[PythonScopeFact, ...]
    states: tuple[PythonBindingState, ...]
    slot_names: tuple[str, ...]
    _expression_lookup: Mapping[PythonNodeKey, PythonExpressionFact]
    _call_lookup: Mapping[PythonNodeKey, PythonCallSiteFact]

    @classmethod
    def create(
        cls,
        *,
        source_digest: str,
        target_python: tuple[int, int],
        target_sys_platform: str | None,
        module_name: str | None,
        module_spec_name: str | None,
        module_is_package: bool,
        module_execution_kind: str,
        module_import_flow: ModuleImportFlow,
        expressions: tuple[PythonExpressionFact, ...],
        calls: tuple[PythonCallSiteFact, ...],
        scopes: tuple[PythonScopeFact, ...],
        states: tuple[PythonBindingState, ...],
        slot_names: tuple[str, ...],
    ) -> PythonBindingIndex:
        return cls(
            source_digest=source_digest,
            target_python=target_python,
            target_sys_platform=target_sys_platform,
            module_name=module_name,
            module_spec_name=module_spec_name,
            module_is_package=module_is_package,
            module_execution_kind=module_execution_kind,
            module_import_flow=module_import_flow,
            expressions=expressions,
            calls=calls,
            scopes=scopes,
            states=states,
            slot_names=slot_names,
            _expression_lookup=MappingProxyType({fact.node: fact for fact in expressions}),
            _call_lookup=MappingProxyType({fact.node: fact for fact in calls}),
        )

    def expression_fact(self, node: ast.AST) -> PythonExpressionFact | None:
        return self._expression_lookup.get(PythonNodeKey.from_node(node))

    def call_fact(self, node: ast.Call) -> PythonCallSiteFact | None:
        return self._call_lookup.get(PythonNodeKey.from_node(node))

    def static_truth(self, node: ast.expr) -> bool | None:
        """Return truth known by the source-ordered identity analysis in O(1)."""

        fact = self._expression_lookup.get(PythonNodeKey.from_node(node))
        if fact is not None and fact.identities == int(PythonIdentity.STATIC_FALSE):
            return False
        return None

    def static_value(self, node: ast.expr) -> PythonStaticValue:
        fact = self._expression_lookup.get(PythonNodeKey.from_node(node))
        return None if fact is None else fact.static_value

__all__ = [
    "ALL_INVALID_MEMBERS",
    "IdentityMask",
    "MemberMask",
    "NO_IDENTITIES",
    "OTHER_IDENTITY",
    "PythonBindingIndex",
    "PythonBindingState",
    "PythonCallSiteFact",
    "PythonExpressionFact",
    "PythonIdentity",
    "PythonMember",
    "PythonNodeKey",
    "PythonParameterRef",
    "PythonStaticValue",
    "PythonScopeFact",
    "UNKNOWN_IDENTITY",
    "UNBOUND_IDENTITY",
    "exact_identity",
    "identity_fact_is_exact",
    "identity_fact_is_proven",
    "identity_fact_may_be",
    "identity_fact_names",
    "possible_identity",
]
