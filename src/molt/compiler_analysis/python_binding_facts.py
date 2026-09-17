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
from collections.abc import Callable, Sequence
from typing import TYPE_CHECKING, Final, Literal, Mapping, TypeAlias

from molt.compiler_analysis.python_builtin_shapes import BUILTIN_SHAPE_NAMES
from molt.compiler_analysis.python_effects_generated import EffectMask
from molt.compiler_analysis.python_source_keys import python_node_source_key
from molt.compiler_analysis.static_truth import (
    StaticExpressionResult,
    UNKNOWN_EXPRESSION_RESULT,
    iterable_element_result,
    join_static_expression_results,
)

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
PythonImportCallKind: TypeAlias = Literal["import_module", "dunder_import"]
NO_IDENTITIES: Final[IdentityMask] = 0
OTHER_IDENTITY: Final[IdentityMask] = int(PythonIdentity.OTHER)
UNBOUND_IDENTITY: Final[IdentityMask] = int(PythonIdentity.UNBOUND)
UNKNOWN_IDENTITY: Final[IdentityMask] = OTHER_IDENTITY | UNBOUND_IDENTITY
_SENTINEL_IDENTITIES: Final[IdentityMask] = OTHER_IDENTITY | UNBOUND_IDENTITY


@dataclass(frozen=True, slots=True)
class PythonParameterRef:
    name: str


@dataclass(frozen=True, slots=True)
class PythonStringAlternatives:
    """Finite alternatives for one string value, not a sequence value."""

    values: frozenset[str]


PythonStaticValue: TypeAlias = (
    str | int | tuple[str, ...] | PythonParameterRef | PythonStringAlternatives | None
)


@dataclass(frozen=True, slots=True)
class PythonIterationFact:
    """Executed iterator boundary and conservative yielded-value provenance."""

    effects: EffectMask
    element_strings: PythonStringAlternatives | None = None
    element_result: StaticExpressionResult = UNKNOWN_EXPRESSION_RESULT
    empty: bool = False
    mutable: bool = True
    release_effects: EffectMask = 0

    @classmethod
    def from_result(
        cls,
        result: StaticExpressionResult,
        effects: EffectMask,
        release_effects: EffectMask = 0,
    ) -> PythonIterationFact:
        # Effects are supplied by the existing iterable protocol authority.
        # Contents of an aliased mutable container are not stable value facts.
        eligible = result.kind in {"tuple", "frozenset"} or result.fresh_container
        pending = list(result.items or ())
        expanded_seen: set[int] = set()
        strings: set[str] = set()
        contents_known = eligible and result.items is not None
        strings_known = contents_known
        while pending and contents_known:
            item = pending.pop()
            if item.expanded:
                identity = id(item.result)
                if identity in expanded_seen:
                    continue
                expanded_seen.add(identity)
                if item.result.items is None:
                    contents_known = False
                else:
                    pending.extend(item.result.items)
            else:
                if item.result.kind == "str" and item.result.value_known:
                    strings.add(str(item.result.value))
                else:
                    strings_known = False
        element_result = iterable_element_result(result) or UNKNOWN_EXPRESSION_RESULT
        return cls(
            effects=effects,
            element_strings=PythonStringAlternatives(frozenset(strings))
            if contents_known and strings_known
            else None,
            element_result=element_result,
            empty=effects == 0 and result.truth is False,
            mutable=result.kind
            not in {
                "str",
                "bytes",
                "bytearray",
                "tuple",
                "frozenset",
                "range",
                "file_text",
                "file_bytes",
            },
            release_effects=release_effects,
        )

    def merge(self, other: PythonIterationFact) -> PythonIterationFact:
        left, right = self.element_strings, other.element_strings
        return PythonIterationFact(
            effects=self.effects | other.effects,
            element_strings=(
                PythonStringAlternatives(left.values | right.values)
                if left is not None and right is not None
                else None
            ),
            element_result=join_static_expression_results(
                (self.element_result, other.element_result)
            ),
            empty=self.empty and other.empty,
            mutable=self.mutable or other.mutable,
            release_effects=self.release_effects | other.release_effects,
        )


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
    INTRINSICS_REQUIRE = 1 << 11
    SYS_PLATFORM = 1 << 12
    BUILTINS_BOOL = 1 << 13
    BUILTINS_INT = 1 << 14
    BUILTINS_FLOAT = 1 << 15
    BUILTINS_COMPLEX = 1 << 16
    BUILTINS_STR = 1 << 17
    BUILTINS_BYTES = 1 << 18
    BUILTINS_BYTEARRAY = 1 << 19
    BUILTINS_TUPLE = 1 << 20
    BUILTINS_LIST = 1 << 21
    BUILTINS_SET = 1 << 22
    BUILTINS_FROZENSET = 1 << 23
    BUILTINS_DICT = 1 << 24
    BUILTINS_RANGE = 1 << 25
    BUILTINS_LEN = 1 << 26
    BUILTINS_OPEN = 1 << 27


MemberMask: TypeAlias = int
NO_INVALID_MEMBERS: Final[MemberMask] = 0
ALL_INVALID_MEMBERS: Final[MemberMask] = sum(int(member) for member in PythonMember)

BUILTIN_SHAPE_IDENTITIES: Final[Mapping[str, PythonIdentity]] = MappingProxyType(
    {
        name: PythonIdentity[f"BUILTIN_{name.upper()}"]
        for name in sorted(BUILTIN_SHAPE_NAMES)
    }
)
BUILTIN_SHAPE_MEMBERS: Final[Mapping[str, PythonMember]] = MappingProxyType(
    {
        name: PythonMember[f"BUILTINS_{name.upper()}"]
        for name in sorted(BUILTIN_SHAPE_NAMES)
    }
)
_BUILTIN_SHAPE_NAMES_BY_IDENTITY: Final[Mapping[IdentityMask, str]] = MappingProxyType(
    {int(identity): name for name, identity in BUILTIN_SHAPE_IDENTITIES.items()}
)


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
        return cls(*python_node_source_key(node))


PythonNameLookup: TypeAlias = Literal[
    "none", "lexical", "global", "class_lexical", "class_global"
]


@dataclass(frozen=True, slots=True)
class PythonExpressionFact:
    """Expression identity plus source-point name storage/specialization facts.

    Invalidation removes value/specialization facts, not storage ownership;
    class_namespace_lookup retains the source scope's mapping precedence. Bound
    lexical names shadow builtins even when their current value is unbound.
    Neither property may be inferred from OTHER, which also describes pristine
    builtin names not enumerated in the capability identity vocabulary.
    """

    node: PythonNodeKey
    scope_id: int
    identities: IdentityMask
    effects: EffectMask
    static_value: PythonStaticValue = None
    binding_invalidated: bool = False
    binding_is_bound: bool = False
    result: StaticExpressionResult = UNKNOWN_EXPRESSION_RESULT
    module_namespace_observable: bool = False
    truth_effects: EffectMask = 0
    name_lookup: PythonNameLookup = "none"

    @property
    def exposes_module_globals(self) -> bool:
        """A mapping result or an observed container can publish module custody."""
        return bool(self.identities & int(PythonIdentity.CURRENT_GLOBALS)) or (
            self.result.kind in {"tuple", "list", "set", "dict"}
            and self.module_namespace_observable
        )

    @property
    def class_namespace_lookup(self) -> bool:
        return self.name_lookup in {"class_lexical", "class_global"}

    @property
    def binding_global_lookup(self) -> bool:
        return self.name_lookup == "global"


_LOOP_FIXPOINT_STEPS: Final = 8


class PythonCompletion(IntFlag):
    """Mutually routed statement successors; NONE is analyzed divergence."""

    NONE = 0
    NORMAL = 1 << 0
    RETURN = 1 << 1
    RAISE = 1 << 2
    BREAK = 1 << 3
    CONTINUE = 1 << 4


@dataclass(frozen=True, slots=True)
class PythonCompletionFlow[T]:
    """One abstract state per completion, shared by binding and import flow.

    Missing states are unreachable successors, not unknown values. Sequence
    visits only normal completion; finally may replace each incoming completion.
    The payload join belongs to the consumer, never to an AST classifier.
    """

    normal: T | None = None
    returned: T | None = None
    raised: T | None = None
    broken: T | None = None
    continued: T | None = None
    effects: EffectMask = 0

    def successors(self) -> tuple[tuple[PythonCompletion, T], ...]:
        return tuple(
            (kind, state)
            for kind, state in (
                (PythonCompletion.NORMAL, self.normal),
                (PythonCompletion.RETURN, self.returned),
                (PythonCompletion.RAISE, self.raised),
                (PythonCompletion.BREAK, self.broken),
                (PythonCompletion.CONTINUE, self.continued),
            )
            if state is not None
        )

    @property
    def completions(self) -> PythonCompletion:
        result = PythonCompletion.NONE
        for kind, _state in self.successors():
            result |= kind
        return result

    @classmethod
    def single(
        cls, kind: PythonCompletion, state: T, *, effects: EffectMask = 0
    ) -> PythonCompletionFlow[T]:
        if kind not in (
            PythonCompletion.NORMAL,
            PythonCompletion.RETURN,
            PythonCompletion.RAISE,
            PythonCompletion.BREAK,
            PythonCompletion.CONTINUE,
        ):
            raise ValueError("a flow successor requires one completion kind")
        return cls(
            normal=state if kind == PythonCompletion.NORMAL else None,
            returned=state if kind == PythonCompletion.RETURN else None,
            raised=state if kind == PythonCompletion.RAISE else None,
            broken=state if kind == PythonCompletion.BREAK else None,
            continued=state if kind == PythonCompletion.CONTINUE else None,
            effects=effects,
        )

    def merge(
        self, other: PythonCompletionFlow[T], *, join_states: Callable[[T, T], T]
    ) -> PythonCompletionFlow[T]:
        def join(left: T | None, right: T | None) -> T | None:
            if left is None:
                return right
            if right is None:
                return left
            return join_states(left, right)

        return PythonCompletionFlow(
            normal=join(self.normal, other.normal),
            returned=join(self.returned, other.returned),
            raised=join(self.raised, other.raised),
            broken=join(self.broken, other.broken),
            continued=join(self.continued, other.continued),
            effects=self.effects | other.effects,
        )

    def without_normal(self) -> PythonCompletionFlow[T]:
        return PythonCompletionFlow(
            returned=self.returned,
            raised=self.raised,
            broken=self.broken,
            continued=self.continued,
            effects=self.effects,
        )

    def sequence(
        self,
        execute: Callable[[T], PythonCompletionFlow[T]],
        *,
        join_states: Callable[[T, T], T],
    ) -> PythonCompletionFlow[T]:
        if self.normal is None:
            return self
        return self.without_normal().merge(
            execute(self.normal), join_states=join_states
        )

    def map_states[U](self, transform: Callable[[T], U]) -> PythonCompletionFlow[U]:
        return PythonCompletionFlow(
            normal=None if self.normal is None else transform(self.normal),
            returned=None if self.returned is None else transform(self.returned),
            raised=None if self.raised is None else transform(self.raised),
            broken=None if self.broken is None else transform(self.broken),
            continued=None if self.continued is None else transform(self.continued),
            effects=self.effects,
        )

    def _unwind(
        self,
        execute: Callable[[T], PythonCompletionFlow[T]],
        *,
        join_states: Callable[[T, T], T],
        suppress_exceptions: bool,
    ) -> PythonCompletionFlow[T]:
        result: PythonCompletionFlow[T] = PythonCompletionFlow(effects=self.effects)
        for incoming, state in self.successors():
            final = execute(state)
            result = result.merge(final.without_normal(), join_states=join_states)
            if final.normal is not None:
                result = result.merge(
                    PythonCompletionFlow.single(incoming, final.normal),
                    join_states=join_states,
                )
                if suppress_exceptions and incoming == PythonCompletion.RAISE:
                    result = result.merge(
                        PythonCompletionFlow(normal=final.normal),
                        join_states=join_states,
                    )
        return result

    def apply_finally(
        self,
        execute: Callable[[T], PythonCompletionFlow[T]],
        *,
        join_states: Callable[[T, T], T],
    ) -> PythonCompletionFlow[T]:
        return self._unwind(execute, join_states=join_states, suppress_exceptions=False)

    def unwind_context(
        self,
        execute_exit: Callable[[T], PythonCompletionFlow[T]],
        *,
        join_states: Callable[[T, T], T],
    ) -> PythonCompletionFlow[T]:
        """Only a pending exception can become normal through __exit__."""
        return self._unwind(
            execute_exit, join_states=join_states, suppress_exceptions=True
        )

    @classmethod
    def loop(
        cls,
        entry: T,
        advance: Callable[[T], tuple[T | None, PythonCompletionFlow[T]]],
        execute_else: Callable[[T], PythonCompletionFlow[T]],
        *,
        join_states: Callable[[T, T], T],
        equivalent_states: Callable[[T, T], bool],
        widen_state: Callable[[T], T],
        finalize: Callable[[T], PythonCompletionFlow[T]] | None = None,
    ) -> PythonCompletionFlow[T]:
        """Own backedges and finalize iteration before else or terminal exits."""
        header = entry
        exhausted: T | None = None
        exits: PythonCompletionFlow[T] = cls()
        # The final visit must evaluate the widened state, not merely publish it.
        for step in range(_LOOP_FIXPOINT_STEPS + 1):
            if step == _LOOP_FIXPOINT_STEPS:
                header = widen_state(header)
            iteration_exhausted, body = advance(header)
            if iteration_exhausted is not None:
                exhausted = (
                    iteration_exhausted
                    if exhausted is None
                    else join_states(exhausted, iteration_exhausted)
                )
            exits = exits.merge(
                cls(
                    normal=body.broken,
                    returned=body.returned,
                    raised=body.raised,
                    effects=body.effects,
                ),
                join_states=join_states,
            )
            backedge = body.normal
            if body.continued is not None:
                backedge = (
                    body.continued
                    if backedge is None
                    else join_states(backedge, body.continued)
                )
            if backedge is None or step == _LOOP_FIXPOINT_STEPS:
                break
            next_header = join_states(entry, backedge)
            if equivalent_states(next_header, header):
                break
            header = next_header
        if finalize is not None:
            exits = exits.apply_finally(finalize, join_states=join_states)
        if exhausted is not None:
            exhaustion = cls(normal=exhausted)
            if finalize is not None:
                exhaustion = exhaustion.sequence(finalize, join_states=join_states)
            exits = exits.merge(
                exhaustion.sequence(execute_else, join_states=join_states),
                join_states=join_states,
            )
        return exits

    @classmethod
    def exception_group_handlers[H](
        cls,
        state: T,
        handlers: Sequence[H],
        *,
        evaluate_type: Callable[[H, T], PythonCompletionFlow[T]],
        split_group: Callable[[T], PythonCompletionFlow[T]],
        execute_handler: Callable[[H, T], PythonCompletionFlow[T]],
        merge_group: Callable[[T], PythonCompletionFlow[T]],
        join_states: Callable[[T, T], T],
    ) -> PythonCompletionFlow[T]:
        """Route subgroups while deferring handler-raised exceptions.

        A handler can match some, all, or none of the remaining subgroup.
        Exceptions raised by a handler do not prevent later handlers from
        observing the remaining original subgroups.
        Subgroup splitting and final exception reconstruction can call Python
        through ExceptionGroup subclasses; their consumers supply the state
        transfer, while this scheduler owns the protocol's execution order.
        """
        pending: dict[tuple[bool, bool], T] = {(False, False): state}
        result: PythonCompletionFlow[T] = cls()

        def append(
            states: dict[tuple[bool, bool], T], key: tuple[bool, bool], incoming: T
        ) -> None:
            previous = states.get(key)
            states[key] = (
                incoming if previous is None else join_states(previous, incoming)
            )

        for handler in handlers:
            following: dict[tuple[bool, bool], T] = {}
            for (matched, deferred_raise), incoming in pending.items():
                evaluated = evaluate_type(handler, incoming).sequence(
                    split_group, join_states=join_states
                )
                result = result.merge(
                    evaluated.without_normal(), join_states=join_states
                )
                if evaluated.normal is None:
                    continue
                branch = evaluated.normal
                append(following, (matched, deferred_raise), branch)
                handled = execute_handler(handler, branch)
                result = result.merge(
                    cls(
                        returned=handled.returned,
                        broken=handled.broken,
                        continued=handled.continued,
                        effects=handled.effects,
                    ),
                    join_states=join_states,
                )
                if handled.normal is not None:
                    append(following, (True, deferred_raise), handled.normal)
                if handled.raised is not None:
                    append(following, (True, True), handled.raised)
            pending = following
        for (matched, deferred_raise), successor in pending.items():
            reconstructed = merge_group(successor).sequence(
                lambda assembled: cls(raised=assembled), join_states=join_states
            )
            result = result.merge(reconstructed, join_states=join_states)
            if matched and not deferred_raise:
                result = result.merge(cls(normal=successor), join_states=join_states)
        return result


@dataclass(frozen=True, slots=True)
class PythonStatementFact:
    """Executed effects and successors, excluding deferred definition bodies."""

    node: PythonNodeKey
    scope_id: int
    effects: EffectMask
    module_namespace_observable: bool
    completions: PythonCompletion
    iteration: PythonIterationFact | None = None


@dataclass(frozen=True, slots=True)
class PythonCallSiteFact:
    node: PythonNodeKey
    scope_id: int
    callee_identities: IdentityMask
    result_identities: IdentityMask
    effects: EffectMask
    evaluation_effects: EffectMask
    invocation_effects: EffectMask
    cleanup_effects: EffectMask
    callee_elision_safe: bool
    callee_retention_safe: bool
    maybe_invalidated_members_after: MemberMask
    definitely_invalidated_members_after: MemberMask

    def callee_is(self, identity: PythonIdentity) -> bool:
        return identity_fact_is_exact(self.callee_identities, identity)

    def callee_may_be(self, identity: PythonIdentity) -> bool:
        return identity_fact_may_be(self.callee_identities, identity)

    def exact_builtin_name(self) -> str | None:
        return _BUILTIN_SHAPE_NAMES_BY_IDENTITY.get(self.callee_identities)

    def possible_import_call_kinds(self) -> tuple[PythonImportCallKind, ...]:
        kinds: list[PythonImportCallKind] = []
        if self.callee_may_be(PythonIdentity.IMPORTLIB_IMPORT_MODULE):
            kinds.append("import_module")
        if self.callee_may_be(PythonIdentity.BUILTINS_IMPORT):
            kinds.append("dunder_import")
        return tuple(kinds)

    def exact_import_call_kind(self) -> PythonImportCallKind | None:
        if self.callee_is(PythonIdentity.IMPORTLIB_IMPORT_MODULE):
            return "import_module"
        if self.callee_is(PythonIdentity.BUILTINS_IMPORT):
            return "dunder_import"
        return None


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
    activation_namespace_stable: bool


@dataclass(frozen=True, slots=True)
class PythonBindingTelemetry:
    binding_lookups: int
    join_calls: int
    join_node_visits: int
    join_shared_subtrees_skipped: int
    join_chunk_merges: int
    structural_diff_cache_entries: int
    structural_diff_node_visits: int
    structural_diff_shared_subtrees_skipped: int


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
    statements: tuple[PythonStatementFact, ...]
    iterations: tuple[tuple[PythonNodeKey, PythonIterationFact], ...]
    calls: tuple[PythonCallSiteFact, ...]
    scopes: tuple[PythonScopeFact, ...]
    class_annotation_namespaces: frozenset[PythonNodeKey]
    state_count: int
    telemetry: PythonBindingTelemetry
    slot_names: tuple[str, ...]
    _expression_lookup: Mapping[PythonNodeKey, PythonExpressionFact]
    _statement_lookup: Mapping[PythonNodeKey, PythonStatementFact]
    _iteration_lookup: Mapping[PythonNodeKey, PythonIterationFact]
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
        statements: tuple[PythonStatementFact, ...],
        iterations: tuple[tuple[PythonNodeKey, PythonIterationFact], ...],
        calls: tuple[PythonCallSiteFact, ...],
        scopes: tuple[PythonScopeFact, ...],
        class_annotation_namespaces: frozenset[PythonNodeKey],
        state_count: int,
        telemetry: PythonBindingTelemetry,
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
            statements=statements,
            iterations=iterations,
            calls=calls,
            scopes=scopes,
            class_annotation_namespaces=class_annotation_namespaces,
            state_count=state_count,
            telemetry=telemetry,
            slot_names=slot_names,
            _expression_lookup=MappingProxyType(
                {fact.node: fact for fact in expressions}
            ),
            _statement_lookup=MappingProxyType(
                {fact.node: fact for fact in statements}
            ),
            _iteration_lookup=MappingProxyType(dict(iterations)),
            _call_lookup=MappingProxyType({fact.node: fact for fact in calls}),
        )

    def expression_fact(self, node: ast.AST) -> PythonExpressionFact | None:
        return self._expression_lookup.get(PythonNodeKey.from_node(node))

    def class_annotation_namespace_required(self, node: ast.ClassDef) -> bool:
        """A deferred evaluator needs this class's live namespace owner.

        This is a whole-body storage fact, not an allocation request at the
        first source use: that use may be conditional or execute repeatedly.
        """
        return PythonNodeKey.from_node(node) in self.class_annotation_namespaces

    def statement_fact(self, node: ast.stmt) -> PythonStatementFact | None:
        return self._statement_lookup.get(PythonNodeKey.from_node(node))

    def iteration_fact(self, node: ast.AST) -> PythonIterationFact | None:
        """Return the canonical executed iterator/item fact for a loop clause."""

        return self._iteration_lookup.get(PythonNodeKey.from_node(node))

    def statement_completions(self, node: ast.stmt) -> PythonCompletion | None:
        """None is absent analysis; NONE is proven absence of a successor."""
        fact = self.statement_fact(node)
        return None if fact is None else fact.completions

    def module_namespace_may_be_observed(self, node: ast.AST) -> bool:
        """Project executed namespace exposure; missing source facts fail closed.

        Target pruning may wrap an original condition in a synthetic Expr.
        Query its original expression fact without rescanning names or entering
        bodies that are not executed at this source point.
        """
        if isinstance(node, ast.Module):
            return any(
                self.module_namespace_may_be_observed(stmt) for stmt in node.body
            )
        fact: PythonStatementFact | PythonExpressionFact | None
        if isinstance(node, ast.stmt):
            fact = self.statement_fact(node)
            if fact is None and isinstance(node, ast.Expr):
                fact = self.expression_fact(node.value)
        else:
            fact = self.expression_fact(node)
        return fact is None or fact.module_namespace_observable

    def call_fact(self, node: ast.Call) -> PythonCallSiteFact | None:
        return self._call_lookup.get(PythonNodeKey.from_node(node))

    def expression_result(self, node: ast.expr) -> StaticExpressionResult:
        """Unknown source-point facts never authorize spelling-based constants."""
        fact = self.expression_fact(node)
        return UNKNOWN_EXPRESSION_RESULT if fact is None else fact.result

    def static_truth(self, node: ast.expr) -> bool | None:
        return self.expression_result(node).truth

    def static_value(self, node: ast.expr) -> PythonStaticValue:
        fact = self._expression_lookup.get(PythonNodeKey.from_node(node))
        return None if fact is None else fact.static_value


__all__ = [
    "ALL_INVALID_MEMBERS",
    "BUILTIN_SHAPE_IDENTITIES",
    "BUILTIN_SHAPE_MEMBERS",
    "IdentityMask",
    "MemberMask",
    "NO_IDENTITIES",
    "OTHER_IDENTITY",
    "PythonBindingIndex",
    "PythonBindingTelemetry",
    "PythonCallSiteFact",
    "PythonCompletion",
    "PythonCompletionFlow",
    "PythonExpressionFact",
    "PythonIdentity",
    "PythonImportCallKind",
    "PythonMember",
    "PythonNodeKey",
    "PythonParameterRef",
    "PythonStaticValue",
    "PythonStatementFact",
    "PythonIterationFact",
    "PythonStringAlternatives",
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
