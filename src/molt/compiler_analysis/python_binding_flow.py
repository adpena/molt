"""Canonical source-ordered Python lexical binding and capability analysis.

This module is deliberately consumer-neutral.  It performs the control-flow and
lexical-identity work once, then exposes immutable facts to import discovery,
frontend lowering, and future semantic passes.  It never executes user code and
fails closed whenever Python can re-enter through reflection, imports, reference
release, descriptors, iteration, context managers, or comparison callbacks.
"""

from __future__ import annotations

import ast
import hashlib
from collections import OrderedDict
from collections.abc import Callable
from dataclasses import dataclass, field
from threading import Event, RLock
from typing import Final, Iterable, Literal, Sequence, cast

from molt.compiler_analysis.python_binding_facts import (
    ALL_INVALID_MEMBERS,
    NO_IDENTITIES,
    OTHER_IDENTITY,
    UNBOUND_IDENTITY,
    IdentityMask,
    MemberMask,
    PythonBindingIndex,
    PythonBindingState,
    PythonCallSiteFact,
    PythonExpressionFact,
    PythonIdentity,
    PythonMember,
    PythonNodeKey,
    PythonParameterRef,
    PythonScopeFact,
    PythonStaticValue,
    exact_identity,
    possible_identity,
)
from molt.compiler_analysis.python_effects_generated import (
    ALLOCATES,
    EXECUTES_ARBITRARY_PYTHON,
    INVOKES_COMPARISON_CALLBACK,
    INVOKES_CONTEXT_CALLBACK,
    INVOKES_DESCRIPTOR,
    INVOKES_IMPORT_SYSTEM,
    INVOKES_ITERATION_CALLBACK,
    NO_EFFECTS,
    RAISES,
    READS_FRAME_STATE,
    READS_GLOBAL_NAMESPACE,
    READS_OBJECT_STATE,
    REFLECTS_NAMESPACE,
    RELEASES_REFERENCE,
    RUNS_FINALIZER,
    RUNS_WEAKREF_CALLBACK,
    SUSPENDS,
    UNKNOWN_EFFECTS,
    WRITES_FRAME_STATE,
    WRITES_GLOBAL_NAMESPACE,
    WRITES_MODULE_METADATA,
    WRITES_OBJECT_STATE,
    EffectMask,
)


_ANALYSIS_SCHEMA: Final = 2
_MAX_LOOP_FIXPOINT_STEPS: Final = 8
_METADATA_NAMES: Final = frozenset({"__name__", "__package__", "__spec__", "__path__"})
_RELEASE_CALLBACK_EFFECTS: Final[EffectMask] = (
    RELEASES_REFERENCE | RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK
)
_IMPORT_EXECUTION_INVALID_MEMBERS: Final[MemberMask] = (
    ALL_INVALID_MEMBERS & ~int(PythonMember.TYPING_TYPE_CHECKING)
)


_BUILTIN_IDENTITIES: Final[dict[str, IdentityMask]] = {
    "__import__": exact_identity(PythonIdentity.BUILTINS_IMPORT),
    "globals": exact_identity(PythonIdentity.BUILTIN_GLOBALS),
    "locals": exact_identity(PythonIdentity.BUILTIN_LOCALS),
    "vars": exact_identity(PythonIdentity.BUILTIN_VARS),
    "setattr": exact_identity(PythonIdentity.BUILTIN_SETATTR),
    "eval": exact_identity(PythonIdentity.BUILTIN_EVAL),
    "exec": exact_identity(PythonIdentity.BUILTIN_EXEC),
}
_CANONICAL_IMPORT_IDENTITIES: Final[dict[str, PythonIdentity]] = {
    "importlib": PythonIdentity.IMPORTLIB_MODULE,
    "importlib.util": PythonIdentity.IMPORTLIB_UTIL_MODULE,
    "importlib.machinery": PythonIdentity.IMPORTLIB_MACHINERY_MODULE,
    "builtins": PythonIdentity.BUILTINS_MODULE,
    "sys": PythonIdentity.SYS_MODULE,
    "inspect": PythonIdentity.INSPECT_MODULE,
    "typing": PythonIdentity.TYPING_MODULE,
    "typing_extensions": PythonIdentity.TYPING_MODULE,
}


@dataclass(frozen=True, slots=True)
class PythonBindingPolicy:
    """Semantic assumptions which are part of the deterministic cache key."""

    target_python: tuple[int, int] = (3, 12)
    target_sys_platform: str | None = None
    module_name: str | None = None
    module_spec_name: str | None = None
    module_is_package: bool = False
    module_execution_kind: Literal["imported", "module", "script"] = "imported"
    standard_imports_are_canonical: bool = True


class _StatePool:
    def __init__(self) -> None:
        initial = PythonBindingState()
        self._states: list[PythonBindingState] = [initial]
        self._ids: dict[PythonBindingState, int] = {initial: 0}
        self._binding_maps: list[dict[int, IdentityMask]] = [{}]
        self._static_maps: list[dict[int, PythonStaticValue]] = [{}]

    def intern(self, state: PythonBindingState) -> int:
        known = self._ids.get(state)
        if known is not None:
            return known
        index = len(self._states)
        self._states.append(state)
        if not state.parents:
            bindings: dict[int, IdentityMask] = {}
        elif len(state.parents) == 1:
            bindings = self._binding_maps[state.parents[0]]
        else:
            bindings = {}
            slots = {
                slot
                for parent in state.parents
                for slot in self._binding_maps[parent]
            }
            for slot in slots:
                value = 0
                for parent in state.parents:
                    value |= self._binding_maps[parent].get(slot, UNBOUND_IDENTITY)
                if value != UNBOUND_IDENTITY:
                    bindings[slot] = value
        if state.updated_slot >= 0:
            bindings = bindings.copy()
            if state.updated_value == UNBOUND_IDENTITY:
                bindings.pop(state.updated_slot, None)
            else:
                bindings[state.updated_slot] = state.updated_value
        if not state.parents:
            static_values: dict[int, PythonStaticValue] = {}
        elif len(state.parents) == 1:
            static_values = self._static_maps[state.parents[0]]
        else:
            static_values = dict[int, PythonStaticValue]()
            static_slots = {
                slot for parent in state.parents for slot in self._static_maps[parent]
            }
            for slot in static_slots:
                values = tuple(
                    self._static_maps[parent].get(slot)
                    for parent in state.parents
                )
                joined = values[0] if values else None
                if joined is not None and all(
                    value == joined for value in values[1:]
                ):
                    static_values[slot] = joined
        if state.updated_slot >= 0:
            static_values = static_values.copy()
            if state.updated_static_value is None:
                static_values.pop(state.updated_slot, None)
            else:
                static_values[state.updated_slot] = state.updated_static_value
        self._binding_maps.append(bindings)
        self._static_maps.append(static_values)
        self._ids[state] = index
        return index

    def get(self, state_id: int) -> PythonBindingState:
        return self._states[state_id]

    def binding(self, state_id: int, slot: int) -> IdentityMask:
        value = self._binding_maps[state_id].get(slot, UNBOUND_IDENTITY)
        if (
            value != UNBOUND_IDENTITY
            and not self._states[state_id].clean_slots & (1 << slot)
        ):
            value |= OTHER_IDENTITY
        return value

    def static_value(self, state_id: int, slot: int) -> PythonStaticValue:
        if not self._states[state_id].clean_slots & (1 << slot):
            return None
        return self._static_maps[state_id].get(slot)

    def set_binding(
        self,
        state_id: int,
        slot: int,
        value: IdentityMask,
        static_value: PythonStaticValue = None,
    ) -> int:
        if (
            self.binding(state_id, slot) == value
            and self.static_value(state_id, slot) == static_value
            and self._states[state_id].clean_slots & (1 << slot)
        ):
            return state_id
        state = self._states[state_id]
        return self.intern(
            PythonBindingState(
                parents=(state_id,),
                updated_slot=slot,
                updated_value=value,
                updated_static_value=static_value,
                clean_slots=state.clean_slots | (1 << slot),
                maybe_invalidated_members=state.maybe_invalidated_members,
                definitely_invalidated_members=state.definitely_invalidated_members,
            )
        )

    def taint_slots(self, state_id: int, slots: int) -> int:
        state = self._states[state_id]
        clean_slots = state.clean_slots & ~slots
        if clean_slots == state.clean_slots:
            return state_id
        return self.intern(
            PythonBindingState(
                parents=(state_id,),
                clean_slots=clean_slots,
                maybe_invalidated_members=state.maybe_invalidated_members,
                definitely_invalidated_members=state.definitely_invalidated_members,
            )
        )

    def invalidate_members(
        self, state_id: int, members: MemberMask, *, definite: bool = False
    ) -> int:
        state = self._states[state_id]
        maybe = state.maybe_invalidated_members | members
        definitely = (
            state.definitely_invalidated_members | members
            if definite
            else state.definitely_invalidated_members
        )
        if (
            maybe == state.maybe_invalidated_members
            and definitely == state.definitely_invalidated_members
        ):
            return state_id
        return self.intern(
            PythonBindingState(
                parents=(state_id,),
                clean_slots=state.clean_slots,
                maybe_invalidated_members=maybe,
                definitely_invalidated_members=definitely,
            )
        )

    def validate_members(self, state_id: int, members: MemberMask) -> int:
        state = self._states[state_id]
        maybe = state.maybe_invalidated_members & ~members
        definitely = state.definitely_invalidated_members & ~members
        if (
            maybe == state.maybe_invalidated_members
            and definitely == state.definitely_invalidated_members
        ):
            return state_id
        return self.intern(
            PythonBindingState(
                parents=(state_id,),
                clean_slots=state.clean_slots,
                maybe_invalidated_members=maybe,
                definitely_invalidated_members=definitely,
            )
        )

    def join_member_state(
        self, state_id: int, summary_id: int, summarized_slots: int
    ) -> int:
        state = self._states[state_id]
        summary = self._states[summary_id]
        return self.intern(
            PythonBindingState(
                parents=(state_id,),
                clean_slots=(state.clean_slots & ~summarized_slots)
                | (
                    state.clean_slots
                    & summary.clean_slots
                    & summarized_slots
                ),
                maybe_invalidated_members=(
                    state.maybe_invalidated_members
                    | summary.maybe_invalidated_members
                ),
                definitely_invalidated_members=(
                    state.definitely_invalidated_members
                    & summary.definitely_invalidated_members
                ),
            )
        )

    def join(self, *state_ids: int) -> int:
        if not state_ids:
            return 0
        parents = tuple(sorted(set(state_ids)))
        if len(parents) == 1:
            return parents[0]
        maybe_invalidated = 0
        definitely_invalidated = ALL_INVALID_MEMBERS
        clean_slots = -1
        for state_id in parents:
            state = self._states[state_id]
            maybe_invalidated |= state.maybe_invalidated_members
            definitely_invalidated &= state.definitely_invalidated_members
            clean_slots &= state.clean_slots
        return self.intern(
            PythonBindingState(
                parents=parents,
                clean_slots=clean_slots,
                maybe_invalidated_members=maybe_invalidated,
                definitely_invalidated_members=definitely_invalidated,
            )
        )

    def equivalent(self, left_id: int, right_id: int) -> bool:
        if left_id == right_id:
            return True
        left = self._states[left_id]
        right = self._states[right_id]
        if (
            left.maybe_invalidated_members != right.maybe_invalidated_members
            or left.definitely_invalidated_members
            != right.definitely_invalidated_members
        ):
            return False
        slots = self._binding_maps[left_id].keys() | self._binding_maps[right_id].keys()
        return all(
            self.binding(left_id, slot) == self.binding(right_id, slot)
            and self.static_value(left_id, slot) == self.static_value(right_id, slot)
            for slot in slots
        )

    def export(self) -> tuple[PythonBindingState, ...]:
        return tuple(self._states)


_ScopeKind = Literal["module", "function", "class", "comprehension", "lambda"]


@dataclass(slots=True)
class _Scope:
    scope_id: int
    parent: _Scope | None
    kind: _ScopeKind
    name: str
    locals: frozenset[str]
    globals: frozenset[str]
    nonlocals: frozenset[str]
    slots: dict[str, int]
    entry_state_id: int = 0
    exit_state_id: int = 0


@dataclass(frozen=True, slots=True)
class _ScopeDeclarations:
    bound: frozenset[str]
    globals: frozenset[str]
    nonlocals: frozenset[str]


def _target_names(target: ast.AST | None) -> set[str]:
    if target is None:
        return set()
    if isinstance(target, ast.Name):
        return {target.id}
    if isinstance(target, (ast.Tuple, ast.List)):
        return {name for item in target.elts for name in _target_names(item)}
    if isinstance(target, ast.Starred):
        return _target_names(target.value)
    if isinstance(target, (ast.MatchAs, ast.MatchStar)):
        names = {target.name} if target.name else set()
        if isinstance(target, ast.MatchAs):
            names.update(_target_names(target.pattern))
        return names
    if isinstance(target, ast.MatchMapping):
        names = {target.rest} if target.rest else set()
        for pattern in target.patterns:
            names.update(_target_names(pattern))
        return names
    if isinstance(target, ast.MatchSequence):
        return {name for pattern in target.patterns for name in _target_names(pattern)}
    if isinstance(target, ast.MatchClass):
        return {
            name
            for pattern in (*target.patterns, *target.kwd_patterns)
            for name in _target_names(pattern)
        }
    if isinstance(target, ast.MatchOr):
        return {name for pattern in target.patterns for name in _target_names(pattern)}
    return set()


class _DeclarationCollector(ast.NodeVisitor):
    """One scope-local symbol-table pass; nested lexical scopes are skipped."""

    def __init__(self) -> None:
        self.bound: set[str] = set()
        self.globals: set[str] = set()
        self.nonlocals: set[str] = set()

    def visit_Name(self, node: ast.Name) -> None:
        if isinstance(node.ctx, (ast.Store, ast.Del)):
            self.bound.add(node.id)

    def visit_Import(self, node: ast.Import) -> None:
        self.bound.update(alias.asname or alias.name.split(".", 1)[0] for alias in node.names)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        self.bound.update(alias.asname or alias.name for alias in node.names if alias.name != "*")

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self.bound.add(node.name)
        for decorator in node.decorator_list:
            self.visit(decorator)
        for expression in (*node.args.defaults, *node.args.kw_defaults):
            if expression is not None:
                self.visit(expression)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self.bound.add(node.name)
        for decorator in node.decorator_list:
            self.visit(decorator)
        for expression in (*node.args.defaults, *node.args.kw_defaults):
            if expression is not None:
                self.visit(expression)

    def visit_Lambda(self, node: ast.Lambda) -> None:
        for expression in (*node.args.defaults, *node.args.kw_defaults):
            if expression is not None:
                self.visit(expression)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        self.bound.add(node.name)
        for expression in (*node.decorator_list, *node.bases):
            self.visit(expression)
        for keyword in node.keywords:
            self.visit(keyword.value)

    def visit_Global(self, node: ast.Global) -> None:
        self.globals.update(node.names)

    def visit_Nonlocal(self, node: ast.Nonlocal) -> None:
        self.nonlocals.update(node.names)

    def visit_comprehension(self, node: ast.comprehension) -> None:
        self.visit(node.iter)
        for condition in node.ifs:
            self.visit(condition)


def _scope_declarations(
    body: Sequence[ast.stmt],
    parameters: Iterable[str] = (),
) -> _ScopeDeclarations:
    collector = _DeclarationCollector()
    for statement in body:
        collector.visit(statement)
    bound = (collector.bound | set(parameters)) - collector.globals - collector.nonlocals
    return _ScopeDeclarations(
        frozenset(bound), frozenset(collector.globals), frozenset(collector.nonlocals)
    )


def _argument_names(arguments: ast.arguments) -> tuple[str, ...]:
    names = [
        argument.arg
        for argument in (*arguments.posonlyargs, *arguments.args, *arguments.kwonlyargs)
    ]
    if arguments.vararg is not None:
        names.append(arguments.vararg.arg)
    if arguments.kwarg is not None:
        names.append(arguments.kwarg.arg)
    return tuple(names)


def _node_key(node: ast.AST) -> PythonNodeKey:
    return PythonNodeKey.from_node(node)


def _literal_string(node: ast.AST | None) -> str | None:
    return node.value if isinstance(node, ast.Constant) and isinstance(node.value, str) else None


def _literal_truth(node: ast.expr) -> bool | None:
    if isinstance(node, ast.Constant):
        return bool(node.value)
    if isinstance(node, (ast.Tuple, ast.List, ast.Set)):
        return bool(node.elts)
    if isinstance(node, ast.Dict):
        return bool(node.keys)
    if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.Not):
        operand = _literal_truth(node.operand)
        return None if operand is None else not operand
    return None


def _identity_can_release(mask: IdentityMask) -> bool:
    inert = int(
        PythonIdentity.IMPORTLIB_MODULE
        | PythonIdentity.IMPORTLIB_IMPORT_MODULE
        | PythonIdentity.IMPORTLIB_MACHINERY_MODULE
        | PythonIdentity.MODULE_SPEC_CLASS
        | PythonIdentity.BUILTINS_MODULE
        | PythonIdentity.BUILTINS_IMPORT
        | PythonIdentity.SYS_MODULE
        | PythonIdentity.SYS_MODULES
        | PythonIdentity.INSPECT_MODULE
        | PythonIdentity.INSPECT_CURRENTFRAME
        | PythonIdentity.CURRENT_GLOBALS
        | PythonIdentity.CURRENT_LOCALS
        | PythonIdentity.CURRENT_FRAME
        | PythonIdentity.BUILTIN_GLOBALS
        | PythonIdentity.BUILTIN_LOCALS
        | PythonIdentity.BUILTIN_VARS
        | PythonIdentity.BUILTIN_SETATTR
        | PythonIdentity.BUILTIN_EVAL
        | PythonIdentity.BUILTIN_EXEC
        | PythonIdentity.INERT_VALUE
        | PythonIdentity.IMPORTLIB_UTIL_MODULE
        | PythonIdentity.IMPORTLIB_FIND_SPEC
    )
    return bool(mask & ~inert) and mask != UNBOUND_IDENTITY


@dataclass(slots=True)
class _ExpressionResult:
    state_id: int
    identities: IdentityMask
    effects: EffectMask
    static_value: PythonStaticValue = None


@dataclass(slots=True)
class _FunctionJob:
    node: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda
    parent_scope: _Scope
    outer_state_id: int
    module_history_start: int | None
    module_states: tuple[int, ...] | None


class _Analyzer:
    def __init__(self, policy: PythonBindingPolicy, source_digest: str) -> None:
        self.policy = policy
        self.source_digest = source_digest
        self.states = _StatePool()
        self.scopes: list[_Scope] = []
        self.slot_names: list[str] = []
        self.expressions: dict[PythonNodeKey, PythonExpressionFact] = {}
        self.calls: dict[PythonNodeKey, PythonCallSiteFact] = {}
        self.function_jobs: list[_FunctionJob] = []
        self.module_scope: _Scope | None = None
        self.module_slots: list[int] = []
        self.module_slot_mask = 0
        self._observed_stack: list[list[int]] = []
        self._module_history: list[int] = [0]
        self._active_module_states: tuple[int, ...] | None = None

    def _queue_function(
        self,
        node: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda,
        scope: _Scope,
        state_id: int,
    ) -> None:
        self.function_jobs.append(
            _FunctionJob(
                node,
                scope,
                state_id,
                (
                    None
                    if self._active_module_states is not None
                    else len(self._module_history)
                ),
                self._active_module_states,
            )
        )

    def _new_scope(
        self,
        *,
        parent: _Scope | None,
        kind: _ScopeKind,
        name: str,
        declarations: _ScopeDeclarations,
    ) -> _Scope:
        scope = _Scope(
            scope_id=len(self.scopes),
            parent=parent,
            kind=kind,
            name=name,
            locals=declarations.bound,
            globals=declarations.globals,
            nonlocals=declarations.nonlocals,
            slots={},
        )
        if self.module_scope is not None:
            for global_name in sorted(scope.globals):
                if global_name not in self.module_scope.slots:
                    slot = len(self.slot_names)
                    self.module_scope.slots[global_name] = slot
                    self.slot_names.append(
                        f"{self.module_scope.scope_id}:{global_name}"
                    )
                    self.module_slots.append(slot)
                    self.module_slot_mask |= 1 << slot
        for local_name in sorted(scope.locals):
            scope.slots[local_name] = len(self.slot_names)
            self.slot_names.append(f"{scope.scope_id}:{local_name}")
        self.scopes.append(scope)
        return scope

    def _module_slot(self, name: str) -> int | None:
        assert self.module_scope is not None
        return self.module_scope.slots.get(name)

    def _nonlocal_slot(self, scope: _Scope, name: str) -> int | None:
        parent = scope.parent
        while parent is not None and parent.kind != "module":
            slot = parent.slots.get(name)
            if slot is not None:
                return slot
            parent = parent.parent
        return None

    def _slot_for_name(self, scope: _Scope, name: str) -> int | None:
        if name in scope.globals:
            return self._module_slot(name)
        if name in scope.nonlocals:
            return self._nonlocal_slot(scope, name)
        slot = scope.slots.get(name)
        if slot is not None:
            return slot
        parent = scope.parent
        while parent is not None:
            slot = parent.slots.get(name)
            if slot is not None:
                return slot
            parent = parent.parent
        return None

    def _lookup_name(self, state_id: int, scope: _Scope, name: str) -> IdentityMask:
        slot = self._slot_for_name(scope, name)
        if slot is not None:
            value = self.states.binding(state_id, slot)
            if value != UNBOUND_IDENTITY or scope.kind in {"function", "lambda", "comprehension"} and name in scope.locals:
                return value
        builtin = _BUILTIN_IDENTITIES.get(name)
        if builtin is None:
            return OTHER_IDENTITY | UNBOUND_IDENTITY
        if name == "__import__":
            state = self.states.get(state_id)
            guard = int(PythonMember.BUILTINS_IMPORT)
            if state.definitely_invalidated_members & guard:
                return OTHER_IDENTITY
            if state.maybe_invalidated_members & guard:
                return builtin | OTHER_IDENTITY
        return builtin

    def _lookup_static_value(
        self, state_id: int, scope: _Scope, name: str
    ) -> PythonStaticValue:
        slot = self._slot_for_name(scope, name)
        return None if slot is None else self.states.static_value(state_id, slot)

    def _record_state(self, state_id: int) -> None:
        for observed in self._observed_stack:
            observed.append(state_id)

    def _record_expression(
        self,
        node: ast.AST,
        scope: _Scope,
        state_before: int,
        identities: IdentityMask,
        effects: EffectMask,
        static_value: PythonStaticValue,
    ) -> None:
        self.expressions[_node_key(node)] = PythonExpressionFact(
            _node_key(node),
            scope.scope_id,
            state_before,
            identities,
            effects,
            static_value,
        )

    def _widen_module_bindings(self, state_id: int) -> int:
        return self.states.taint_slots(state_id, self.module_slot_mask)

    def _apply_effects(self, state_id: int, effects: EffectMask) -> int:
        callback_effects = (
            RUNS_FINALIZER
            | RUNS_WEAKREF_CALLBACK
            | INVOKES_DESCRIPTOR
            | INVOKES_ITERATION_CALLBACK
            | INVOKES_CONTEXT_CALLBACK
            | INVOKES_COMPARISON_CALLBACK
        )
        if effects & (
            WRITES_GLOBAL_NAMESPACE
            | WRITES_FRAME_STATE
            | EXECUTES_ARBITRARY_PYTHON
            | callback_effects
        ):
            state_id = self._widen_module_bindings(state_id)
        if effects & (
            EXECUTES_ARBITRARY_PYTHON
            | INVOKES_IMPORT_SYSTEM
            | WRITES_OBJECT_STATE
            | callback_effects
        ):
            state_id = self.states.invalidate_members(state_id, ALL_INVALID_MEMBERS)
        return state_id

    def _member_value(
        self,
        state_id: int,
        base: IdentityMask,
        member: str,
    ) -> IdentityMask:
        state = self.states.get(state_id)
        maybe_invalidated = state.maybe_invalidated_members
        definitely_invalidated = state.definitely_invalidated_members

        def admitted(owner: PythonIdentity, guard: PythonMember, result: PythonIdentity) -> IdentityMask:
            if not base & int(owner):
                return NO_IDENTITIES
            if definitely_invalidated & int(guard):
                return OTHER_IDENTITY
            exact = base == int(owner) and not maybe_invalidated & int(guard)
            return exact_identity(result) if exact else possible_identity(result)

        value = NO_IDENTITIES
        if member == "import_module":
            value |= admitted(
                PythonIdentity.IMPORTLIB_MODULE,
                PythonMember.IMPORTLIB_IMPORT_MODULE,
                PythonIdentity.IMPORTLIB_IMPORT_MODULE,
            )
        elif member == "machinery":
            value |= admitted(
                PythonIdentity.IMPORTLIB_MODULE,
                PythonMember.IMPORTLIB_MACHINERY,
                PythonIdentity.IMPORTLIB_MACHINERY_MODULE,
            )
        elif member == "util":
            value |= admitted(
                PythonIdentity.IMPORTLIB_MODULE,
                PythonMember.IMPORTLIB_UTIL,
                PythonIdentity.IMPORTLIB_UTIL_MODULE,
            )
        elif member == "ModuleSpec":
            value |= admitted(
                PythonIdentity.IMPORTLIB_MACHINERY_MODULE,
                PythonMember.MACHINERY_MODULE_SPEC,
                PythonIdentity.MODULE_SPEC_CLASS,
            )
        elif member == "__import__":
            value |= admitted(
                PythonIdentity.BUILTINS_MODULE,
                PythonMember.BUILTINS_IMPORT,
                PythonIdentity.BUILTINS_IMPORT,
            )
        elif member == "modules":
            value |= admitted(
                PythonIdentity.SYS_MODULE,
                PythonMember.SYS_MODULES,
                PythonIdentity.SYS_MODULES,
            )
        elif member == "currentframe":
            value |= admitted(
                PythonIdentity.INSPECT_MODULE,
                PythonMember.INSPECT_CURRENTFRAME,
                PythonIdentity.INSPECT_CURRENTFRAME,
            )
        elif member == "find_spec":
            value |= admitted(
                PythonIdentity.IMPORTLIB_UTIL_MODULE,
                PythonMember.UTIL_FIND_SPEC,
                PythonIdentity.IMPORTLIB_FIND_SPEC,
            )
        elif member == "TYPE_CHECKING":
            value |= admitted(
                PythonIdentity.TYPING_MODULE,
                PythonMember.TYPING_TYPE_CHECKING,
                PythonIdentity.STATIC_FALSE,
            )
        elif member in {"f_globals", "__globals__"}:
            owners = int(PythonIdentity.CURRENT_FRAME | PythonIdentity.USER_FUNCTION)
            if base & owners:
                value |= exact_identity(PythonIdentity.CURRENT_GLOBALS)
                if base & ~owners:
                    value |= OTHER_IDENTITY
        if value == NO_IDENTITIES:
            return OTHER_IDENTITY
        if base & OTHER_IDENTITY:
            value |= OTHER_IDENTITY
        return value

    def _invalidate_member_target(
        self,
        state_id: int,
        base: IdentityMask,
        member: str,
    ) -> int:
        members = 0
        if base & int(PythonIdentity.IMPORTLIB_MODULE):
            if member == "import_module":
                members |= int(PythonMember.IMPORTLIB_IMPORT_MODULE)
            elif member == "machinery":
                members |= int(PythonMember.IMPORTLIB_MACHINERY)
            elif member == "util":
                members |= int(PythonMember.IMPORTLIB_UTIL)
        if base & int(PythonIdentity.IMPORTLIB_MACHINERY_MODULE) and member == "ModuleSpec":
            members |= int(PythonMember.MACHINERY_MODULE_SPEC)
        if base & int(PythonIdentity.IMPORTLIB_UTIL_MODULE) and member == "find_spec":
            members |= int(PythonMember.UTIL_FIND_SPEC)
        if base & int(PythonIdentity.TYPING_MODULE) and member == "TYPE_CHECKING":
            members |= int(PythonMember.TYPING_TYPE_CHECKING)
        if base & int(PythonIdentity.MODULE_SPEC_CLASS):
            members |= int(PythonMember.MODULE_SPEC_CLASS)
        if base & int(PythonIdentity.BUILTINS_MODULE) and member == "__import__":
            members |= int(PythonMember.BUILTINS_IMPORT | PythonMember.IMPORT_HOOKS)
        if base & int(PythonIdentity.SYS_MODULE) and member in {"meta_path", "path_hooks"}:
            members |= int(PythonMember.IMPORT_HOOKS)
        if base & int(PythonIdentity.SYS_MODULE) and member == "modules":
            members |= int(PythonMember.SYS_MODULES | PythonMember.IMPORT_HOOKS)
        return self.states.invalidate_members(state_id, members, definite=True)

    def _call_semantics(
        self,
        state_id: int,
        scope: _Scope,
        node: ast.Call,
        callee: IdentityMask,
        argument_effects: EffectMask,
    ) -> tuple[IdentityMask, EffectMask, int]:
        effects = argument_effects
        result = OTHER_IDENTITY
        exact = callee.bit_count() == 1
        if exact and callee == int(PythonIdentity.BUILTIN_GLOBALS):
            result = exact_identity(PythonIdentity.CURRENT_GLOBALS)
            effects |= REFLECTS_NAMESPACE | READS_GLOBAL_NAMESPACE
        elif exact and callee in {
            int(PythonIdentity.BUILTIN_LOCALS), int(PythonIdentity.BUILTIN_VARS)
        } and not node.args and not node.keywords:
            result = exact_identity(
                PythonIdentity.CURRENT_GLOBALS if scope.kind == "module" else PythonIdentity.CURRENT_LOCALS
            )
            effects |= REFLECTS_NAMESPACE | READS_FRAME_STATE
        elif exact and callee == int(PythonIdentity.INSPECT_CURRENTFRAME):
            result = exact_identity(PythonIdentity.CURRENT_FRAME)
            effects |= READS_FRAME_STATE | REFLECTS_NAMESPACE | ALLOCATES
        elif callee & int(PythonIdentity.MODULE_SPEC_CLASS):
            result = exact_identity(PythonIdentity.MODULE_SPEC_INSTANCE)
            member_state = self.states.get(state_id)
            if (
                not exact
                or member_state.maybe_invalidated_members
                & int(PythonMember.MODULE_SPEC_CLASS)
            ):
                result |= OTHER_IDENTITY
            effects |= ALLOCATES | RAISES
        elif callee & int(
            PythonIdentity.IMPORTLIB_IMPORT_MODULE
            | PythonIdentity.IMPORTLIB_FIND_SPEC
            | PythonIdentity.BUILTINS_IMPORT
        ):
            result = OTHER_IDENTITY
            effects |= EXECUTES_ARBITRARY_PYTHON | INVOKES_IMPORT_SYSTEM | ALLOCATES | RAISES
        elif exact and callee == int(PythonIdentity.BUILTIN_SETATTR):
            effects |= (
                WRITES_OBJECT_STATE | INVOKES_DESCRIPTOR | EXECUTES_ARBITRARY_PYTHON
                | _RELEASE_CALLBACK_EFFECTS | RAISES
            )
            if len(node.args) >= 2:
                owner = self._expression_identity(node.args[0])
                member = _literal_string(node.args[1])
                if member is not None:
                    state_id = self._invalidate_member_target(state_id, owner, member)
                    if owner & int(PythonIdentity.CURRENT_MODULE) and member in _METADATA_NAMES:
                        effects |= WRITES_MODULE_METADATA | WRITES_GLOBAL_NAMESPACE
        elif callee & int(PythonIdentity.BUILTIN_EVAL | PythonIdentity.BUILTIN_EXEC):
            effects |= UNKNOWN_EFFECTS
        else:
            effects |= UNKNOWN_EFFECTS
        return result, effects, state_id

    def _expression_identity(self, node: ast.AST) -> IdentityMask:
        fact = self.expressions.get(_node_key(node))
        return fact.identities if fact is not None else OTHER_IDENTITY

    def eval_expr(self, node: ast.expr, state_id: int, scope: _Scope) -> _ExpressionResult:
        before = state_id
        effects = NO_EFFECTS
        identities = OTHER_IDENTITY
        static_value: PythonStaticValue = None
        if isinstance(node, ast.Constant):
            identities = exact_identity(
                PythonIdentity.STATIC_FALSE
                if node.value is False
                else PythonIdentity.INERT_VALUE
            )
            if isinstance(node.value, str) or (
                isinstance(node.value, int) and not isinstance(node.value, bool)
            ):
                static_value = node.value
        elif isinstance(node, ast.Name):
            identities = self._lookup_name(state_id, scope, node.id)
            static_value = self._lookup_static_value(state_id, scope, node.id)
        elif isinstance(node, ast.Attribute):
            base = self.eval_expr(node.value, state_id, scope)
            state_id = base.state_id
            effects |= base.effects
            identities = self._member_value(state_id, base.identities, node.attr)
            effects |= READS_OBJECT_STATE | RAISES
            if identities == OTHER_IDENTITY:
                effects |= INVOKES_DESCRIPTOR | EXECUTES_ARBITRARY_PYTHON
        elif isinstance(node, ast.Subscript):
            owner = self.eval_expr(node.value, state_id, scope)
            index = self.eval_expr(node.slice, owner.state_id, scope)
            state_id = index.state_id
            effects |= owner.effects | index.effects | READS_OBJECT_STATE | RAISES
            if owner.identities & int(PythonIdentity.SYS_MODULES) and isinstance(node.slice, ast.Name) and node.slice.id == "__name__":
                identities = exact_identity(PythonIdentity.CURRENT_MODULE)
                if owner.identities != int(PythonIdentity.SYS_MODULES):
                    identities |= OTHER_IDENTITY
            else:
                identities = OTHER_IDENTITY
                effects |= EXECUTES_ARBITRARY_PYTHON
        elif isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            left = self.eval_expr(node.left, state_id, scope)
            right = self.eval_expr(node.right, left.state_id, scope)
            state_id = right.state_id
            effects |= left.effects | right.effects | RAISES
            identities = possible_identity(PythonIdentity.INERT_VALUE)
            if isinstance(left.static_value, str) and isinstance(
                right.static_value, str
            ):
                static_value = left.static_value + right.static_value
            elif isinstance(left.static_value, int) and isinstance(
                right.static_value, int
            ):
                static_value = left.static_value + right.static_value
            else:
                effects |= EXECUTES_ARBITRARY_PYTHON
        elif isinstance(node, ast.Call):
            callee_result = self.eval_expr(node.func, state_id, scope)
            state_id = callee_result.state_id
            effects |= callee_result.effects
            for argument in node.args:
                result = self.eval_expr(argument, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
            for keyword in node.keywords:
                result = self.eval_expr(keyword.value, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
            identities, effects, state_id = self._call_semantics(
                state_id, scope, node, callee_result.identities, effects
            )
            state_id = self._apply_effects(state_id, effects)
            self.calls[_node_key(node)] = PythonCallSiteFact(
                _node_key(node), scope.scope_id, before, callee_result.identities,
                identities,
                effects,
                self.states.get(state_id).maybe_invalidated_members,
                self.states.get(state_id).definitely_invalidated_members,
            )
        elif isinstance(node, ast.NamedExpr):
            value = self.eval_expr(node.value, state_id, scope)
            state_id, target_effects = self.assign_target(
                node.target,
                value.identities,
                value.state_id,
                scope,
                static_value=value.static_value,
            )
            effects = value.effects | target_effects
            identities = value.identities
            static_value = value.static_value
        elif isinstance(node, ast.Lambda):
            state_id, defaults_effect = self._eval_arguments(node.args, state_id, scope)
            effects |= defaults_effect | ALLOCATES
            identities = exact_identity(PythonIdentity.USER_FUNCTION)
            self._queue_function(node, scope, state_id)
        elif isinstance(node, ast.IfExp):
            test = self.eval_expr(node.test, state_id, scope)
            truth = _literal_truth(node.test)
            if truth is not None:
                selected = self.eval_expr(
                    node.body if truth else node.orelse, test.state_id, scope
                )
                state_id = selected.state_id
                identities = selected.identities
                effects = test.effects | selected.effects
            else:
                left = self.eval_expr(node.body, test.state_id, scope)
                right = self.eval_expr(node.orelse, test.state_id, scope)
                state_id = self.states.join(left.state_id, right.state_id)
                identities = left.identities | right.identities
                effects = (
                    test.effects
                    | left.effects
                    | right.effects
                    | INVOKES_COMPARISON_CALLBACK
                    | RAISES
                )
        elif isinstance(node, ast.BoolOp):
            possible_exits: list[int] = []
            identities = NO_IDENTITIES
            current = state_id
            for index, value in enumerate(node.values):
                result = self.eval_expr(value, current, scope)
                effects |= result.effects
                truth = _literal_truth(value)
                final_value = index == len(node.values) - 1
                stops = (
                    final_value
                    or isinstance(node.op, ast.And)
                    and truth is False
                    or isinstance(node.op, ast.Or)
                    and truth is True
                )
                if stops or truth is None:
                    possible_exits.append(result.state_id)
                    identities |= result.identities
                if stops:
                    break
                if truth is None:
                    callback_effects = INVOKES_COMPARISON_CALLBACK | RAISES
                    effects |= callback_effects
                    current = self._apply_effects(
                        result.state_id, callback_effects
                    )
                else:
                    current = result.state_id
            state_id = self.states.join(*possible_exits)
        elif isinstance(node, (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)):
            state_id, comp_effects = self._eval_comprehension(node, state_id, scope)
            identities = exact_identity(PythonIdentity.OTHER)
            effects |= comp_effects | ALLOCATES
            if not isinstance(node, ast.GeneratorExp):
                effects |= INVOKES_ITERATION_CALLBACK | EXECUTES_ARBITRARY_PYTHON | RAISES
        elif isinstance(node, (ast.Tuple, ast.List, ast.Set)):
            element_static_values: list[PythonStaticValue] = []
            for element in node.elts:
                result = self.eval_expr(element, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
                element_static_values.append(result.static_value)
            identities = exact_identity(PythonIdentity.INERT_VALUE) | OTHER_IDENTITY
            effects |= ALLOCATES
            if not isinstance(node, ast.Set) and all(
                isinstance(value, str) for value in element_static_values
            ):
                static_value = tuple(cast(str, value) for value in element_static_values)
        elif isinstance(node, ast.Dict):
            for key, value in zip(node.keys, node.values):
                if key is not None:
                    result = self.eval_expr(key, state_id, scope)
                    state_id = result.state_id
                    effects |= result.effects
                result = self.eval_expr(value, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
                if key is None:
                    effects |= INVOKES_ITERATION_CALLBACK | EXECUTES_ARBITRARY_PYTHON | RAISES
            identities = possible_identity(PythonIdentity.INERT_VALUE)
            effects |= ALLOCATES
        elif isinstance(node, (ast.Await, ast.Yield, ast.YieldFrom)):
            value = getattr(node, "value", None)
            if isinstance(value, ast.expr):
                result = self.eval_expr(value, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
            effects |= EXECUTES_ARBITRARY_PYTHON | SUSPENDS | RAISES
            identities = OTHER_IDENTITY
        else:
            for child in ast.iter_child_nodes(node):
                if isinstance(child, ast.expr):
                    result = self.eval_expr(child, state_id, scope)
                    state_id = result.state_id
                    effects |= result.effects
            identities = OTHER_IDENTITY
            effects |= EXECUTES_ARBITRARY_PYTHON | RAISES
            if isinstance(node, (ast.Compare, ast.UnaryOp, ast.BinOp)):
                effects |= INVOKES_COMPARISON_CALLBACK
        if not isinstance(node, ast.Call):
            state_id = self._apply_effects(state_id, effects)
        self._record_expression(
            node, scope, before, identities, effects, static_value
        )
        self._record_state(state_id)
        return _ExpressionResult(state_id, identities, effects, static_value)

    def _eval_arguments(self, arguments: ast.arguments, state_id: int, scope: _Scope) -> tuple[int, EffectMask]:
        effects = NO_EFFECTS
        for expression in (*arguments.defaults, *arguments.kw_defaults):
            if expression is None:
                continue
            result = self.eval_expr(expression, state_id, scope)
            state_id = result.state_id
            effects |= result.effects
        return state_id, effects

    def _eval_comprehension(self, node: ast.expr, state_id: int, parent: _Scope) -> tuple[int, EffectMask]:
        generators = tuple(getattr(node, "generators"))
        names = {name for generator in generators for name in _target_names(generator.target)}
        declarations = _ScopeDeclarations(frozenset(names), frozenset(), frozenset())
        scope = self._new_scope(parent=parent, kind="comprehension", name="<comprehension>", declarations=declarations)
        first_iterable = self.eval_expr(generators[0].iter, state_id, parent)
        immediate_effects = (
            first_iterable.effects
            | INVOKES_ITERATION_CALLBACK
            | EXECUTES_ARBITRARY_PYTHON
            | RAISES
        )
        scope.entry_state_id = first_iterable.state_id
        deferred_effects = NO_EFFECTS
        current = first_iterable.state_id
        for index, generator in enumerate(generators):
            if index:
                iterable = self.eval_expr(generator.iter, current, scope)
                current = iterable.state_id
                deferred_effects |= iterable.effects
            deferred_effects |= (
                INVOKES_ITERATION_CALLBACK | EXECUTES_ARBITRARY_PYTHON | RAISES
            )
            current, target_effects = self.assign_target(generator.target, OTHER_IDENTITY, current, scope)
            deferred_effects |= target_effects
            for condition in generator.ifs:
                condition_result = self.eval_expr(condition, current, scope)
                current = condition_result.state_id
                deferred_effects |= (
                    condition_result.effects | INVOKES_COMPARISON_CALLBACK
                )
        payloads: list[ast.expr]
        if isinstance(node, ast.DictComp):
            payloads = [node.key, node.value]
        elif isinstance(node, (ast.ListComp, ast.SetComp, ast.GeneratorExp)):
            payloads = [node.elt]
        else:
            raise AssertionError(type(node).__name__)
        for payload in payloads:
            result = self.eval_expr(payload, current, scope)
            current = result.state_id
            deferred_effects |= result.effects
        scope.exit_state_id = current
        if isinstance(node, ast.GeneratorExp):
            return first_iterable.state_id, immediate_effects
        return current, immediate_effects | deferred_effects

    def _release_binding(self, state_id: int, slot: int) -> tuple[int, EffectMask]:
        previous = self.states.binding(state_id, slot)
        if not _identity_can_release(previous):
            return state_id, NO_EFFECTS
        effects = _RELEASE_CALLBACK_EFFECTS
        return self._apply_effects(state_id, effects), effects

    def assign_target(
        self,
        target: ast.AST,
        value: IdentityMask,
        state_id: int,
        scope: _Scope,
        *,
        static_value: PythonStaticValue = None,
    ) -> tuple[int, EffectMask]:
        effects = NO_EFFECTS
        if isinstance(target, ast.Name):
            slot = self._slot_for_name(scope, target.id)
            if slot is None:
                return state_id, effects
            state_id, release_effects = self._release_binding(state_id, slot)
            effects |= release_effects
            if release_effects:
                value |= OTHER_IDENTITY
            state_id = self.states.set_binding(
                state_id, slot, value, static_value
            )
            return state_id, effects
        if isinstance(target, (ast.Tuple, ast.List)):
            effects |= INVOKES_ITERATION_CALLBACK | EXECUTES_ARBITRARY_PYTHON | RAISES
            state_id = self._apply_effects(state_id, effects)
            for element in target.elts:
                state_id, element_effects = self.assign_target(element, OTHER_IDENTITY, state_id, scope)
                effects |= element_effects
            return state_id, effects
        if isinstance(target, ast.Starred):
            return self.assign_target(target.value, OTHER_IDENTITY, state_id, scope)
        if isinstance(target, ast.Attribute):
            owner = self.eval_expr(target.value, state_id, scope)
            state_id = self._invalidate_member_target(owner.state_id, owner.identities, target.attr)
            effects |= owner.effects | WRITES_OBJECT_STATE | INVOKES_DESCRIPTOR | EXECUTES_ARBITRARY_PYTHON | _RELEASE_CALLBACK_EFFECTS | RAISES
            if owner.identities & int(PythonIdentity.CURRENT_MODULE) and target.attr in _METADATA_NAMES:
                effects |= WRITES_MODULE_METADATA | WRITES_GLOBAL_NAMESPACE
            return self._apply_effects(state_id, effects), effects
        if isinstance(target, ast.Subscript):
            owner = self.eval_expr(target.value, state_id, scope)
            index = self.eval_expr(target.slice, owner.state_id, scope)
            effects |= owner.effects | index.effects | WRITES_OBJECT_STATE | EXECUTES_ARBITRARY_PYTHON | _RELEASE_CALLBACK_EFFECTS | RAISES
            if owner.identities & int(PythonIdentity.CURRENT_GLOBALS):
                effects |= WRITES_GLOBAL_NAMESPACE
                if _literal_string(target.slice) in _METADATA_NAMES:
                    effects |= WRITES_MODULE_METADATA
            return self._apply_effects(index.state_id, effects), effects
        return self._apply_effects(state_id, UNKNOWN_EFFECTS), UNKNOWN_EFFECTS

    def delete_target(self, target: ast.AST, state_id: int, scope: _Scope) -> tuple[int, EffectMask]:
        if isinstance(target, ast.Name):
            slot = self._slot_for_name(scope, target.id)
            if slot is None:
                return state_id, NO_EFFECTS
            state_id, effects = self._release_binding(state_id, slot)
            return self.states.set_binding(state_id, slot, UNBOUND_IDENTITY), effects
        if isinstance(target, (ast.Tuple, ast.List)):
            effects = NO_EFFECTS
            for element in target.elts:
                state_id, child_effects = self.delete_target(element, state_id, scope)
                effects |= child_effects
            return state_id, effects
        return self.assign_target(target, OTHER_IDENTITY, state_id, scope)

    def _canonical_imports_available(self, state_id: int) -> bool:
        return self.policy.standard_imports_are_canonical and not (
            self.states.get(state_id).maybe_invalidated_members
            & int(PythonMember.IMPORT_HOOKS)
        )

    def _import_identity(self, module: str, state_id: int) -> IdentityMask:
        identity = _CANONICAL_IMPORT_IDENTITIES.get(module)
        if identity is None:
            return OTHER_IDENTITY
        return (
            exact_identity(identity)
            if self._canonical_imports_available(state_id)
            else possible_identity(identity)
        )

    def _from_import_identity(
        self, module: str | None, name: str, state_id: int
    ) -> IdentityMask:
        identity = {
            ("importlib", "import_module"): PythonIdentity.IMPORTLIB_IMPORT_MODULE,
            ("importlib", "util"): PythonIdentity.IMPORTLIB_UTIL_MODULE,
            ("importlib", "machinery"): PythonIdentity.IMPORTLIB_MACHINERY_MODULE,
            ("importlib.util", "find_spec"): PythonIdentity.IMPORTLIB_FIND_SPEC,
            ("importlib.machinery", "ModuleSpec"): PythonIdentity.MODULE_SPEC_CLASS,
            ("builtins", "__import__"): PythonIdentity.BUILTINS_IMPORT,
            ("inspect", "currentframe"): PythonIdentity.INSPECT_CURRENTFRAME,
            ("typing", "TYPE_CHECKING"): PythonIdentity.STATIC_FALSE,
            ("typing_extensions", "TYPE_CHECKING"): PythonIdentity.STATIC_FALSE,
        }.get((module, name))
        if identity is None:
            return OTHER_IDENTITY
        return (
            exact_identity(identity)
            if self._canonical_imports_available(state_id)
            else possible_identity(identity)
        )

    def _bind_name(self, name: str, value: IdentityMask, state_id: int, scope: _Scope) -> tuple[int, EffectMask]:
        synthetic = ast.Name(id=name, ctx=ast.Store())
        return self.assign_target(synthetic, value, state_id, scope)

    def exec_statements(self, body: Sequence[ast.stmt], state_id: int, scope: _Scope) -> tuple[int, EffectMask]:
        effects = NO_EFFECTS
        for statement in body:
            state_id, statement_effects = self.exec_statement(statement, state_id, scope)
            effects |= statement_effects
            self._record_state(state_id)
            if scope.kind == "module":
                self._module_history.append(state_id)
        return state_id, effects

    def exec_statement(self, node: ast.stmt, state_id: int, scope: _Scope) -> tuple[int, EffectMask]:
        effects = NO_EFFECTS
        if isinstance(node, ast.Expr):
            result = self.eval_expr(node.value, state_id, scope)
            return result.state_id, result.effects
        if isinstance(node, ast.Assign):
            result = self.eval_expr(node.value, state_id, scope)
            state_id = result.state_id
            effects |= result.effects
            for target in node.targets:
                state_id, target_effects = self.assign_target(
                    target,
                    result.identities,
                    state_id,
                    scope,
                    static_value=result.static_value,
                )
                effects |= target_effects
        elif isinstance(node, ast.AnnAssign):
            if self.policy.target_python < (3, 14):
                annotation = self.eval_expr(node.annotation, state_id, scope)
                state_id = annotation.state_id
                effects |= annotation.effects
            if node.value is not None:
                result = self.eval_expr(node.value, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
                state_id, target_effects = self.assign_target(
                    node.target,
                    result.identities,
                    state_id,
                    scope,
                    static_value=result.static_value,
                )
                effects |= target_effects
        elif isinstance(node, ast.AugAssign):
            target_read = self.eval_expr(node.target, state_id, scope) if isinstance(node.target, ast.expr) else _ExpressionResult(state_id, OTHER_IDENTITY, NO_EFFECTS)
            value = self.eval_expr(node.value, target_read.state_id, scope)
            effects |= target_read.effects | value.effects | EXECUTES_ARBITRARY_PYTHON | RAISES
            state_id, target_effects = self.assign_target(node.target, OTHER_IDENTITY, value.state_id, scope)
            effects |= target_effects
        elif isinstance(node, ast.Delete):
            for target in node.targets:
                state_id, target_effects = self.delete_target(target, state_id, scope)
                effects |= target_effects
        elif isinstance(node, ast.Import):
            canonical_before_import = self._canonical_imports_available(state_id)
            effects |= EXECUTES_ARBITRARY_PYTHON | INVOKES_IMPORT_SYSTEM | RAISES
            identity_modules = tuple(
                alias.name if alias.asname else alias.name.split(".", 1)[0]
                for alias in node.names
            )
            canonical_statement = canonical_before_import and all(
                module in _CANONICAL_IMPORT_IDENTITIES
                for module in identity_modules
            )
            if not canonical_statement:
                state_id = self.states.invalidate_members(
                    state_id, _IMPORT_EXECUTION_INVALID_MEMBERS
                )
            for alias in node.names:
                bound = alias.asname or alias.name.split(".", 1)[0]
                identity_module = alias.name if alias.asname else alias.name.split(".", 1)[0]
                state_id, bind_effects = self._bind_name(
                    bound,
                    self._import_identity(identity_module, state_id),
                    state_id,
                    scope,
                )
                effects |= bind_effects
        elif isinstance(node, ast.ImportFrom):
            canonical_before_import = self._canonical_imports_available(state_id)
            effects |= EXECUTES_ARBITRARY_PYTHON | INVOKES_IMPORT_SYSTEM | RAISES
            canonical_statement = (
                canonical_before_import
                and node.level == 0
                and node.module is not None
                and all(
                    alias.name != "*"
                    and (
                        node.module in _CANONICAL_IMPORT_IDENTITIES
                        or f"{node.module}.{alias.name}"
                        in _CANONICAL_IMPORT_IDENTITIES
                    )
                    for alias in node.names
                )
            )
            if not canonical_statement:
                state_id = self.states.invalidate_members(
                    state_id, _IMPORT_EXECUTION_INVALID_MEMBERS
                )
            if any(alias.name == "*" for alias in node.names):
                state_id = self._widen_module_bindings(state_id)
            for alias in node.names:
                if alias.name == "*":
                    continue
                state_id, bind_effects = self._bind_name(
                    alias.asname or alias.name,
                    self._from_import_identity(
                        node.module if node.level == 0 else None,
                        alias.name,
                        state_id,
                    ),
                    state_id,
                    scope,
                )
                effects |= bind_effects
        elif isinstance(node, ast.If):
            test = self.eval_expr(node.test, state_id, scope)
            truth = _literal_truth(node.test)
            if truth is None and test.identities == int(PythonIdentity.STATIC_FALSE):
                truth = False
            if truth is not None:
                state_id, branch_effects = self.exec_statements(
                    node.body if truth else node.orelse,
                    test.state_id,
                    scope,
                )
                effects |= test.effects | branch_effects
            else:
                left, left_effects = self.exec_statements(
                    node.body, test.state_id, scope
                )
                right, right_effects = self.exec_statements(
                    node.orelse, test.state_id, scope
                )
                state_id = self.states.join(left, right)
                callback_effects = INVOKES_COMPARISON_CALLBACK | RAISES
                effects |= (
                    test.effects | left_effects | right_effects | callback_effects
                )
                state_id = self._apply_effects(state_id, callback_effects)
        elif isinstance(node, (ast.For, ast.AsyncFor, ast.While)):
            state_id, loop_effects = self._exec_loop(node, state_id, scope)
            effects |= loop_effects
        elif isinstance(node, (ast.With, ast.AsyncWith)):
            for item in node.items:
                context = self.eval_expr(item.context_expr, state_id, scope)
                state_id = context.state_id
                effects |= context.effects | INVOKES_CONTEXT_CALLBACK | EXECUTES_ARBITRARY_PYTHON | RAISES
                if item.optional_vars is not None:
                    state_id, target_effects = self.assign_target(item.optional_vars, OTHER_IDENTITY, state_id, scope)
                    effects |= target_effects
            state_id, body_effects = self.exec_statements(node.body, state_id, scope)
            effects |= body_effects | INVOKES_CONTEXT_CALLBACK | EXECUTES_ARBITRARY_PYTHON | RAISES
            state_id = self._apply_effects(state_id, effects)
        elif isinstance(node, (ast.Try, getattr(ast, "TryStar", ast.Try))):
            state_id, try_effects = self._exec_try(node, state_id, scope)
            effects |= try_effects
        elif isinstance(node, ast.Match):
            subject = self.eval_expr(node.subject, state_id, scope)
            effects |= subject.effects | INVOKES_COMPARISON_CALLBACK | RAISES
            branches = [subject.state_id]
            for case in node.cases:
                branch = subject.state_id
                for name in _target_names(case.pattern):
                    branch, target_effects = self._bind_name(name, OTHER_IDENTITY, branch, scope)
                    effects |= target_effects
                if case.guard is not None:
                    guard = self.eval_expr(case.guard, branch, scope)
                    branch = guard.state_id
                    effects |= guard.effects | INVOKES_COMPARISON_CALLBACK
                branch, branch_effects = self.exec_statements(case.body, branch, scope)
                effects |= branch_effects
                branches.append(branch)
            state_id = self.states.join(*branches)
            state_id = self._apply_effects(
                state_id, INVOKES_COMPARISON_CALLBACK | RAISES
            )
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            for decorator in node.decorator_list:
                result = self.eval_expr(decorator, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
            state_id, default_effects = self._eval_arguments(node.args, state_id, scope)
            effects |= default_effects | ALLOCATES
            function_identity = exact_identity(PythonIdentity.USER_FUNCTION)
            if node.decorator_list:
                decorator_effects = EXECUTES_ARBITRARY_PYTHON | RAISES
                effects |= decorator_effects
                state_id = self._apply_effects(state_id, decorator_effects)
                function_identity |= OTHER_IDENTITY
            state_id, bind_effects = self._bind_name(
                node.name, function_identity, state_id, scope
            )
            effects |= bind_effects
            self._queue_function(node, scope, state_id)
        elif isinstance(node, ast.ClassDef):
            for expression in (*node.decorator_list, *node.bases):
                result = self.eval_expr(expression, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
            for keyword in node.keywords:
                result = self.eval_expr(keyword.value, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
            declarations = _scope_declarations(node.body)
            class_scope = self._new_scope(parent=scope, kind="class", name=node.name, declarations=declarations)
            class_scope.entry_state_id = state_id
            class_state, class_effects = self.exec_statements(node.body, state_id, class_scope)
            class_scope.exit_state_id = class_state
            effects |= class_effects | EXECUTES_ARBITRARY_PYTHON | ALLOCATES | RAISES
            state_id = self._apply_effects(state_id, effects)
            class_identity = possible_identity(PythonIdentity.USER_CLASS)
            state_id, bind_effects = self._bind_name(
                node.name, class_identity, state_id, scope
            )
            effects |= bind_effects
        elif isinstance(node, (ast.Return, ast.Raise)):
            value = getattr(node, "value", None) or getattr(node, "exc", None)
            if isinstance(value, ast.expr):
                result = self.eval_expr(value, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
            effects |= RAISES if isinstance(node, ast.Raise) else NO_EFFECTS
        elif isinstance(node, (ast.Assert,)):
            test = self.eval_expr(node.test, state_id, scope)
            state_id = test.state_id
            effects |= test.effects | INVOKES_COMPARISON_CALLBACK | RAISES
            if node.msg is not None:
                message = self.eval_expr(node.msg, state_id, scope)
                state_id = self.states.join(state_id, message.state_id)
                effects |= message.effects
            state_id = self._apply_effects(
                state_id, INVOKES_COMPARISON_CALLBACK | RAISES
            )
        elif isinstance(node, (ast.Global, ast.Nonlocal, ast.Pass, ast.Break, ast.Continue)):
            pass
        else:
            for child in ast.iter_child_nodes(node):
                if isinstance(child, ast.expr):
                    result = self.eval_expr(child, state_id, scope)
                    state_id = result.state_id
                    effects |= result.effects
            effects |= UNKNOWN_EFFECTS
            state_id = self._apply_effects(state_id, effects)
        self._record_state(state_id)
        return state_id, effects

    def _exec_loop(self, node: ast.For | ast.AsyncFor | ast.While, state_id: int, scope: _Scope) -> tuple[int, EffectMask]:
        effects = NO_EFFECTS
        entry = state_id
        if isinstance(node, (ast.For, ast.AsyncFor)):
            iterable = self.eval_expr(node.iter, entry, scope)
            entry = iterable.state_id
            effects |= iterable.effects | INVOKES_ITERATION_CALLBACK | EXECUTES_ARBITRARY_PYTHON | RAISES
        else:
            test = self.eval_expr(node.test, entry, scope)
            entry = test.state_id
            effects |= test.effects | INVOKES_COMPARISON_CALLBACK | RAISES
        header = entry
        for _step in range(_MAX_LOOP_FIXPOINT_STEPS):
            body_entry = header
            if isinstance(node, (ast.For, ast.AsyncFor)):
                body_entry, target_effects = self.assign_target(node.target, OTHER_IDENTITY, body_entry, scope)
                effects |= target_effects
            body_exit, body_effects = self.exec_statements(node.body, body_entry, scope)
            effects |= body_effects
            next_header = self.states.join(entry, body_exit)
            if self.states.equivalent(next_header, header):
                header = next_header
                break
            header = next_header
        else:
            header = self._widen_module_bindings(header)
            header = self.states.invalidate_members(header, ALL_INVALID_MEMBERS)
        orelse, else_effects = self.exec_statements(node.orelse, header, scope)
        effects |= else_effects
        state_id = self.states.join(entry, header, orelse)
        return self._apply_effects(state_id, effects), effects

    def _exec_try(self, node: ast.Try | ast.TryStar, state_id: int, scope: _Scope) -> tuple[int, EffectMask]:
        observed: list[int] = [state_id]
        self._observed_stack.append(observed)
        try:
            body, effects = self.exec_statements(node.body, state_id, scope)
        finally:
            self._observed_stack.pop()
        exceptional = self.states.join(*observed)
        branches = [body]
        for handler in node.handlers:
            branch = exceptional
            if handler.type is not None:
                result = self.eval_expr(handler.type, branch, scope)
                branch = result.state_id
                effects |= result.effects
            if handler.name:
                branch, bind_effects = self._bind_name(handler.name, OTHER_IDENTITY, branch, scope)
                effects |= bind_effects
            branch, handler_effects = self.exec_statements(handler.body, branch, scope)
            effects |= handler_effects
            if handler.name:
                branch, delete_effects = self.delete_target(ast.Name(id=handler.name, ctx=ast.Del()), branch, scope)
                effects |= delete_effects
            branches.append(branch)
        normal, else_effects = self.exec_statements(node.orelse, body, scope)
        effects |= else_effects
        branches.append(normal)
        joined = self.states.join(*branches)
        final, final_effects = self.exec_statements(node.finalbody, joined, scope)
        return final, effects | final_effects

    def _analyze_function_job(self, job: _FunctionJob, outer_state: int) -> None:
        node = job.node
        arguments = node.args
        parameters = _argument_names(arguments)
        body = node.body if isinstance(node.body, list) else [ast.Return(value=node.body)]
        declarations = _scope_declarations(body, parameters)
        kind: _ScopeKind = "lambda" if isinstance(node, ast.Lambda) else "function"
        name = "<lambda>" if isinstance(node, ast.Lambda) else node.name
        scope = self._new_scope(parent=job.parent_scope, kind=kind, name=name, declarations=declarations)
        state_id = outer_state
        for local_name in scope.locals:
            state_id = self.states.set_binding(state_id, scope.slots[local_name], UNBOUND_IDENTITY)
        for parameter in parameters:
            slot = scope.slots.get(parameter)
            if slot is not None:
                state_id = self.states.set_binding(
                    state_id,
                    slot,
                    OTHER_IDENTITY,
                    PythonParameterRef(parameter),
                )
        scope.entry_state_id = state_id
        observed: list[int] = [state_id]
        self._observed_stack.append(observed)
        try:
            exit_state, _effects = self.exec_statements(body, state_id, scope)
        finally:
            self._observed_stack.pop()
        scope.exit_state_id = self.states.join(exit_state, *observed)

    def _overlay_module_summary(self, state_id: int, summary_id: int) -> int:
        for slot in self.module_slots:
            value = self.states.binding(state_id, slot) | self.states.binding(
                summary_id, slot
            )
            state_id = self.states.set_binding(state_id, slot, value)
        return self.states.join_member_state(
            state_id, summary_id, self.module_slot_mask
        )

    def analyze(self, tree: ast.Module) -> PythonBindingIndex:
        declarations = _scope_declarations(tree.body)
        module = self._new_scope(parent=None, kind="module", name="<module>", declarations=declarations)
        self.module_scope = module
        self.module_slots = list(module.slots.values())
        self.module_slot_mask = sum(1 << slot for slot in self.module_slots)
        module.entry_state_id = 0
        observed: list[int] = [0]
        self._observed_stack.append(observed)
        try:
            module_exit, _effects = self.exec_statements(tree.body, 0, module)
        finally:
            self._observed_stack.pop()
        module.exit_state_id = module_exit
        cursor = 0
        while cursor < len(self.function_jobs):
            job = self.function_jobs[cursor]
            cursor += 1
            # Deferred bodies can run after any observed module state.  Parent
            # function state is retained for closure cells; module globals are
            # widened by the shared summary through the join.
            module_states = job.module_states
            if module_states is None:
                assert job.module_history_start is not None
                module_states = tuple(self._module_history[job.module_history_start :])
            if not module_states:
                module_states = (module_exit,)
            module_summary = self.states.join(*module_states)
            job_outer = self._overlay_module_summary(
                job.outer_state_id, module_summary
            )
            previous_module_states = self._active_module_states
            self._active_module_states = module_states
            try:
                self._analyze_function_job(job, job_outer)
            finally:
                self._active_module_states = previous_module_states
        scope_facts = tuple(
            PythonScopeFact(
                scope.scope_id,
                scope.parent.scope_id if scope.parent is not None else None,
                scope.kind,
                scope.name,
                tuple(sorted(scope.locals)),
                tuple(sorted(scope.globals)),
                tuple(sorted(scope.nonlocals)),
                tuple(
                    sorted(
                        (name, slot)
                        for name in {
                            candidate
                            for visible in self._scope_chain(scope)
                            for candidate in visible.slots
                        }
                        if (slot := self._slot_for_name(scope, name)) is not None
                    )
                ),
                scope.entry_state_id,
                scope.exit_state_id,
            )
            for scope in self.scopes
        )
        from molt.compiler_analysis.python_imports import (
            ModuleImportContext,
            ModuleImportFlow,
            _analyze_module_import_flow_uncached,
            _module_import_flow_required,
            context_import_state,
        )

        import_context = ModuleImportContext(
            module_name=self.policy.module_name,
            is_package=self.policy.module_is_package,
            spec_name=self.policy.module_spec_name,
            target_python=self.policy.target_python,
            execution_kind=self.policy.module_execution_kind,
        )
        if _module_import_flow_required(tree):
            module_import_flow = _analyze_module_import_flow_uncached(
                tree, import_context
            )
        else:
            import_state = context_import_state(import_context)
            module_import_flow = ModuleImportFlow(
                {}, (import_state,), (import_state,)
            )
        return PythonBindingIndex.create(
            source_digest=self.source_digest,
            target_python=self.policy.target_python,
            target_sys_platform=self.policy.target_sys_platform,
            module_name=self.policy.module_name,
            module_spec_name=self.policy.module_spec_name,
            module_is_package=self.policy.module_is_package,
            module_execution_kind=self.policy.module_execution_kind,
            module_import_flow=module_import_flow,
            expressions=tuple(sorted(self.expressions.values(), key=lambda fact: fact.node)),
            calls=tuple(sorted(self.calls.values(), key=lambda fact: fact.node)),
            scopes=scope_facts,
            states=self.states.export(),
            slot_names=tuple(self.slot_names),
        )

    @staticmethod
    def _scope_chain(scope: _Scope) -> tuple[_Scope, ...]:
        chain: list[_Scope] = []
        current: _Scope | None = scope
        while current is not None:
            chain.append(current)
            current = current.parent
        return tuple(chain)


@dataclass(slots=True)
class _PendingAnalysis:
    ready: Event = field(default_factory=Event)
    result: PythonBindingIndex | None = None
    error: BaseException | None = None


class _BindingIndexCache:
    """Content-addressed, single-flight cache safe for free-threaded callers."""

    def __init__(self, max_entries: int = 128) -> None:
        self._max_entries = max_entries
        self._lock = RLock()
        self._ready: OrderedDict[tuple[object, ...], PythonBindingIndex] = OrderedDict()
        self._pending: dict[tuple[object, ...], _PendingAnalysis] = {}

    def get_or_compute(
        self,
        key: tuple[object, ...],
        compute: Callable[[], PythonBindingIndex],
    ) -> PythonBindingIndex:
        owner = False
        with self._lock:
            cached = self._ready.get(key)
            if cached is not None:
                return cached
            pending = self._pending.get(key)
            if pending is None:
                pending = _PendingAnalysis()
                self._pending[key] = pending
                owner = True
        if not owner:
            pending.ready.wait()
            if pending.error is not None:
                raise pending.error
            assert pending.result is not None
            return pending.result
        try:
            result = compute()
        except BaseException as exc:
            with self._lock:
                pending.error = exc
                self._pending.pop(key, None)
                pending.ready.set()
            raise
        with self._lock:
            self._ready[key] = result
            while len(self._ready) > self._max_entries:
                self._ready.popitem(last=False)
            pending.result = result
            self._pending.pop(key, None)
            pending.ready.set()
        return result


_INDEX_CACHE = _BindingIndexCache()


def python_source_digest(source: str) -> str:
    return hashlib.sha256(source.encode("utf-8")).hexdigest()


def python_ast_digest(tree: ast.AST) -> str:
    """Return a filename- and object-identity-independent AST content key."""

    serialized = ast.dump(tree, annotate_fields=True, include_attributes=False)
    return hashlib.sha256(serialized.encode("utf-8")).hexdigest()


def analyze_python_bindings(
    tree: ast.Module,
    *,
    source_digest: str,
    policy: PythonBindingPolicy = PythonBindingPolicy(),
) -> PythonBindingIndex:
    """Analyze an AST through the canonical content-addressed index cache."""

    key = (_ANALYSIS_SCHEMA, source_digest, policy)
    return _INDEX_CACHE.get_or_compute(
        key,
        lambda: _Analyzer(policy, source_digest).analyze(tree),
    )


def analyze_python_source_bindings(
    source: str,
    *,
    filename: str = "<unknown>",
    policy: PythonBindingPolicy = PythonBindingPolicy(),
) -> PythonBindingIndex:
    """Parse and analyze source through the deterministic single-flight cache."""

    digest = python_source_digest(source)
    key = (_ANALYSIS_SCHEMA, digest, policy)

    def compute() -> PythonBindingIndex:
        tree = ast.parse(source, filename=filename, feature_version=policy.target_python)
        return _Analyzer(policy, digest).analyze(tree)

    return _INDEX_CACHE.get_or_compute(key, compute)


__all__ = [
    "PythonBindingPolicy",
    "analyze_python_bindings",
    "analyze_python_source_bindings",
    "python_ast_digest",
    "python_source_digest",
]
