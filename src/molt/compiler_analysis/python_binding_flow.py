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
from bisect import bisect_left, bisect_right
from collections import OrderedDict
from collections.abc import Callable
from dataclasses import dataclass, field, replace
from threading import Event, RLock
from typing import Final, Literal, Sequence, cast

from molt.compiler_analysis.python_call_arguments import call_argument_schedule

from molt.compiler_analysis.python_binding_facts import (
    ALL_INVALID_MEMBERS,
    NO_IDENTITIES,
    NO_INVALID_MEMBERS,
    OTHER_IDENTITY,
    UNBOUND_IDENTITY,
    IdentityMask,
    MemberMask,
    PythonBindingIndex,
    PythonBindingTelemetry,
    PythonCallSiteFact,
    PythonCompletion,
    PythonCompletionFlow,
    PythonExpressionFact,
    PythonIdentity,
    PythonIterationFact,
    PythonStringAlternatives,
    PythonMember,
    PythonNameLookup,
    PythonNodeKey,
    PythonParameterRef,
    PythonScopeFact,
    PythonStatementFact,
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
from molt.compiler_analysis.static_truth import (
    StaticExpressionResult,
    UNKNOWN_EXPRESSION_RESULT,
    static_comparison_result,
    static_expression_result,
)
from molt.compiler_analysis.python_effects import (
    AccumulatedKeyEffects,
    iterable_unpack_effects,
    mapping_unpack_effects,
)
from molt.compiler_analysis.python_imports import import_metadata_target_name
from molt.compiler_analysis.python_lexical_scope import (
    PythonDependencyAuthority,
    PythonScopeDeclarations,
    python_scope_declarations,
    function_annotation_expressions,
    function_parameter_names,
    type_parameter_expressions,
)
from molt.compiler_analysis.python_source_keys import (
    python_pattern_capture_names,
    python_pattern_irrefutable_reason,
    python_pattern_is_capture_only,
)


_ANALYSIS_SCHEMA: Final = 17
_METADATA_NAMES: Final = frozenset({"__name__", "__package__", "__spec__", "__path__"})
_RELEASE_CALLBACK_EFFECTS: Final[EffectMask] = (
    RELEASES_REFERENCE | RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK
)
_IMPORT_EXECUTION_INVALID_MEMBERS: Final[MemberMask] = ALL_INVALID_MEMBERS
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
    "__future__": PythonIdentity.OTHER,
    "importlib": PythonIdentity.IMPORTLIB_MODULE,
    "importlib.util": PythonIdentity.IMPORTLIB_UTIL_MODULE,
    "importlib.machinery": PythonIdentity.IMPORTLIB_MACHINERY_MODULE,
    "builtins": PythonIdentity.BUILTINS_MODULE,
    "sys": PythonIdentity.SYS_MODULE,
    "inspect": PythonIdentity.INSPECT_MODULE,
    "typing": PythonIdentity.TYPING_MODULE,
    "typing_extensions": PythonIdentity.TYPING_MODULE,
    "_intrinsics": PythonIdentity.INTRINSICS_MODULE,
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
    analyze_deferred_bodies: bool = True


_BINDING_CHUNK_SHIFT: Final = 5
_BINDING_CHUNK_SIZE: Final = 1 << _BINDING_CHUNK_SHIFT
_BINDING_CHUNK_MASK: Final = _BINDING_CHUNK_SIZE - 1
_BINDING_CHUNK_BITS_MASK: Final = (1 << _BINDING_CHUNK_SIZE) - 1
_BINDING_TREE_SHIFT: Final = 2
_BINDING_TREE_SIZE: Final = 1 << _BINDING_TREE_SHIFT
_BINDING_TREE_MASK: Final = _BINDING_TREE_SIZE - 1


@dataclass(frozen=True, slots=True)
class _BindingChunk:
    identities: tuple[IdentityMask, ...]
    static_values: tuple[PythonStaticValue, ...]
    clean_epochs: tuple[int, ...]
    active_mask: int
    clean_mask: int


_EMPTY_BINDING_CHUNK: Final = _BindingChunk(
    (UNBOUND_IDENTITY,) * _BINDING_CHUNK_SIZE,
    (None,) * _BINDING_CHUNK_SIZE,
    (0,) * _BINDING_CHUNK_SIZE,
    0,
    0,
)


@dataclass(frozen=True, slots=True)
class _BindingEnvironment:
    root: tuple[object, ...] = ()
    chunk_count: int = 0
    depth: int = 1


_EMPTY_BINDING_ENVIRONMENT: Final = _BindingEnvironment()


@dataclass(frozen=True, slots=True)
class _BindingResolution:
    """One canonical value/static/clean result for a binding lookup."""

    identities: IdentityMask
    static_value: PythonStaticValue
    clean: bool


_UNBOUND_BINDING_RESOLUTION: Final = _BindingResolution(UNBOUND_IDENTITY, None, False)


@dataclass(frozen=True, slots=True)
class _BindingState:
    """One interned persistent control-flow environment node."""

    parents: tuple[int, ...] = ()
    updated_slot: int = -1
    updated_value: IdentityMask = UNBOUND_IDENTITY
    updated_static_value: PythonStaticValue = None
    updated_clean: bool | None = None
    updated_bindings: tuple[tuple[int, IdentityMask, PythonStaticValue, bool], ...] = ()
    taint_epoch: int = 0
    maybe_invalidated_members: MemberMask = 0
    definitely_invalidated_members: MemberMask = 0


class _StatePool:
    def __init__(self) -> None:
        initial = _BindingState()
        self._states: list[_BindingState] = [initial]
        self._ids: dict[_BindingState, int] = {initial: 0}
        self._binding_environments: list[_BindingEnvironment] = [
            _EMPTY_BINDING_ENVIRONMENT
        ]
        self._taint_domain_mask = 0
        self.structural_diff_node_visits = 0
        self.structural_diff_shared_skips = 0
        self.binding_lookups = 0
        self.join_calls = 0
        self.join_node_visits = 0
        self.join_shared_subtrees_skipped = 0
        self.join_chunk_merges = 0

    def set_taint_domain(self, slots: int) -> None:
        if slots & self._taint_domain_mask != self._taint_domain_mask:
            raise RuntimeError("binding taint domain can only grow")
        self._taint_domain_mask = slots

    @staticmethod
    def _chunk_at(environment: _BindingEnvironment, chunk_index: int) -> _BindingChunk:
        if chunk_index >= environment.chunk_count:
            return _EMPTY_BINDING_CHUNK
        node = environment.root
        for level in range(environment.depth - 1, -1, -1):
            offset = (chunk_index >> (level * _BINDING_TREE_SHIFT)) & (
                _BINDING_TREE_MASK
            )
            if offset >= len(node):
                return _EMPTY_BINDING_CHUNK
            child = node[offset]
            if level == 0:
                return cast(_BindingChunk, child)
            node = cast(tuple[object, ...], child)
        return _EMPTY_BINDING_CHUNK

    def _binding_resolution(
        self,
        state_id: int,
        slot: int,
        memo: dict[int, _BindingResolution] | None = None,
    ) -> _BindingResolution:
        if memo is None:
            memo = {}
        cached = memo.get(state_id)
        if cached is not None:
            return cached
        environment = self._binding_environments[state_id]
        chunk = self._chunk_at(environment, slot >> _BINDING_CHUNK_SHIFT)
        chunk_offset = slot & _BINDING_CHUNK_MASK
        slot_bit = 1 << chunk_offset
        if chunk.active_mask & slot_bit or chunk.clean_mask & slot_bit:
            clean = bool(chunk.clean_mask & slot_bit)
            if clean and self._taint_domain_mask & (1 << slot):
                clean = (
                    chunk.clean_epochs[chunk_offset]
                    == self._states[state_id].taint_epoch
                )
            resolution = _BindingResolution(
                chunk.identities[chunk_offset],
                chunk.static_values[chunk_offset],
                clean,
            )
        else:
            # An absent slot is pristine until its namespace has actually
            # crossed a callback boundary. Absence is not itself dirty state.
            resolution = _BindingResolution(
                UNBOUND_IDENTITY,
                None,
                not self.slot_in_taint_domain(slot)
                or self._states[state_id].taint_epoch == 0,
            )
        memo[state_id] = resolution
        return resolution

    @classmethod
    def _updated_environment(
        cls,
        environment: _BindingEnvironment,
        slot: int,
        identity: IdentityMask,
        static_value: PythonStaticValue,
        clean: bool,
        clean_epoch: int,
    ) -> _BindingEnvironment:
        chunk_index = slot >> _BINDING_CHUNK_SHIFT
        previous = cls._chunk_at(environment, chunk_index)
        identities = list(previous.identities)
        identities[slot & _BINDING_CHUNK_MASK] = identity
        static_values = list(previous.static_values)
        static_values[slot & _BINDING_CHUNK_MASK] = static_value
        clean_epochs = list(previous.clean_epochs)
        clean_epochs[slot & _BINDING_CHUNK_MASK] = clean_epoch
        slot_bit = 1 << (slot & _BINDING_CHUNK_MASK)
        active_mask = previous.active_mask
        clean_mask = previous.clean_mask
        if identity == UNBOUND_IDENTITY and static_value is None:
            active_mask &= ~slot_bit
        else:
            active_mask |= slot_bit
        if clean:
            clean_mask |= slot_bit
        else:
            clean_mask &= ~slot_bit
        updated_chunk = _BindingChunk(
            tuple(identities),
            tuple(static_values),
            tuple(clean_epochs),
            active_mask,
            clean_mask,
        )

        def update_node(node: tuple[object, ...], level: int) -> tuple[object, ...]:
            offset = (chunk_index >> (level * _BINDING_TREE_SHIFT)) & (
                _BINDING_TREE_MASK
            )
            children = list(node)
            filler: object = _EMPTY_BINDING_CHUNK if level == 0 else ()
            if offset >= len(children):
                children.extend((filler,) * (offset + 1 - len(children)))
            if level == 0:
                children[offset] = updated_chunk
            else:
                children[offset] = update_node(
                    cast(tuple[object, ...], children[offset]), level - 1
                )
            while children and children[-1] == filler:
                children.pop()
            return tuple(children)

        root = environment.root
        depth = environment.depth
        while chunk_index >= _BINDING_TREE_SIZE**depth:
            root = (root,)
            depth += 1
        root = update_node(root, depth - 1)
        chunk_count = max(environment.chunk_count, chunk_index + 1)
        if not (active_mask or clean_mask) and chunk_index + 1 == chunk_count:
            while (
                chunk_count
                and not cls._chunk_at(
                    _BindingEnvironment(root, chunk_count, depth), chunk_count - 1
                ).active_mask
                and not cls._chunk_at(
                    _BindingEnvironment(root, chunk_count, depth), chunk_count - 1
                ).clean_mask
            ):
                chunk_count -= 1
        return _BindingEnvironment(
            root,
            chunk_count,
            depth,
        )

    def _joined_environment(
        self, parents: tuple[int, ...], taint_epoch: int
    ) -> _BindingEnvironment:
        environments = tuple(self._binding_environments[parent] for parent in parents)
        depth = max((environment.depth for environment in environments), default=1)
        roots: list[tuple[object, ...]] = []
        for environment in environments:
            root = environment.root
            for _level in range(environment.depth, depth):
                root = (root,) if root else ()
            roots.append(root)
        parent_epochs = tuple(self._states[parent].taint_epoch for parent in parents)

        def merge_chunks(
            chunks: tuple[_BindingChunk, ...], chunk_index: int
        ) -> _BindingChunk:
            self.join_chunk_merges += 1
            first = chunks[0]
            if all(chunk is first for chunk in chunks[1:]):
                self.join_shared_subtrees_skipped += 1
                return first
            chunk_epochs = parent_epochs
            if len(chunks) > 3:
                unique_chunks: list[_BindingChunk] = []
                unique_epochs: list[int] = []
                seen: set[tuple[int, int]] = set()
                for chunk, parent_epoch in zip(chunks, parent_epochs, strict=True):
                    key = (id(chunk), parent_epoch)
                    if key in seen:
                        continue
                    seen.add(key)
                    unique_chunks.append(chunk)
                    unique_epochs.append(parent_epoch)
                chunks = tuple(unique_chunks)
                chunk_epochs = tuple(unique_epochs)
            if len(chunks) == 1 and chunk_epochs[0] == taint_epoch:
                return chunks[0]
            candidate_mask = 0
            for chunk in chunks:
                candidate_mask |= chunk.active_mask | chunk.clean_mask
            if not candidate_mask:
                return _EMPTY_BINDING_CHUNK
            identities = list(_EMPTY_BINDING_CHUNK.identities)
            static_values = list(_EMPTY_BINDING_CHUNK.static_values)
            clean_epochs = list(_EMPTY_BINDING_CHUNK.clean_epochs)
            active_mask = 0
            clean_mask = 0
            taint_mask = (
                self._taint_domain_mask >> (chunk_index << _BINDING_CHUNK_SHIFT)
            ) & _BINDING_CHUNK_BITS_MASK
            if len(chunks) == 2:
                left, right = chunks
                left_epoch, right_epoch = chunk_epochs
                left_present = left.active_mask | left.clean_mask
                right_present = right.active_mask | right.clean_mask
                remaining = candidate_mask
                while remaining:
                    slot_bit = remaining & -remaining
                    chunk_offset = slot_bit.bit_length() - 1
                    remaining ^= slot_bit
                    left_identity = (
                        left.identities[chunk_offset]
                        if left_present & slot_bit
                        else UNBOUND_IDENTITY
                    )
                    right_identity = (
                        right.identities[chunk_offset]
                        if right_present & slot_bit
                        else UNBOUND_IDENTITY
                    )
                    identity = left_identity | right_identity
                    left_static = (
                        left.static_values[chunk_offset]
                        if left_present & slot_bit
                        else None
                    )
                    right_static = (
                        right.static_values[chunk_offset]
                        if right_present & slot_bit
                        else None
                    )
                    static_value = left_static if left_static == right_static else None
                    left_clean = bool(left.clean_mask & slot_bit)
                    right_clean = bool(right.clean_mask & slot_bit)
                    if taint_mask & slot_bit:
                        left_clean = (
                            left_clean and left.clean_epochs[chunk_offset] == left_epoch
                        ) or (left_identity == UNBOUND_IDENTITY and left_epoch == 0)
                        right_clean = (
                            right_clean
                            and right.clean_epochs[chunk_offset] == right_epoch
                        ) or (right_identity == UNBOUND_IDENTITY and right_epoch == 0)
                    clean = left_clean and right_clean
                    identities[chunk_offset] = identity
                    static_values[chunk_offset] = static_value
                    clean_epochs[chunk_offset] = taint_epoch
                    if identity != UNBOUND_IDENTITY or static_value is not None:
                        active_mask |= slot_bit
                    if clean:
                        clean_mask |= slot_bit
                if not (active_mask or clean_mask):
                    return _EMPTY_BINDING_CHUNK
                return _BindingChunk(
                    tuple(identities),
                    tuple(static_values),
                    tuple(clean_epochs),
                    active_mask,
                    clean_mask,
                )
            remaining = candidate_mask
            while remaining:
                slot_bit = remaining & -remaining
                chunk_offset = slot_bit.bit_length() - 1
                remaining ^= slot_bit
                identity = NO_IDENTITIES
                static_value: PythonStaticValue = None
                static_initialized = False
                clean = True
                for chunk, parent_epoch in zip(chunks, chunk_epochs, strict=True):
                    present = bool((chunk.active_mask | chunk.clean_mask) & slot_bit)
                    identity |= (
                        chunk.identities[chunk_offset] if present else UNBOUND_IDENTITY
                    )
                    candidate_static = (
                        chunk.static_values[chunk_offset] if present else None
                    )
                    if not static_initialized:
                        static_value = candidate_static
                        static_initialized = True
                    elif candidate_static != static_value:
                        static_value = None
                    parent_clean = bool(chunk.clean_mask & slot_bit)
                    if taint_mask & slot_bit:
                        parent_clean = (
                            parent_clean
                            and chunk.clean_epochs[chunk_offset] == parent_epoch
                        ) or (
                            (
                                not present
                                or chunk.identities[chunk_offset] == UNBOUND_IDENTITY
                            )
                            and parent_epoch == 0
                        )
                    clean = clean and parent_clean
                identities[chunk_offset] = identity
                static_values[chunk_offset] = static_value
                clean_epochs[chunk_offset] = taint_epoch
                if identity != UNBOUND_IDENTITY or static_value is not None:
                    active_mask |= slot_bit
                if clean:
                    clean_mask |= slot_bit
            if not (active_mask or clean_mask):
                return _EMPTY_BINDING_CHUNK
            return _BindingChunk(
                tuple(identities),
                tuple(static_values),
                tuple(clean_epochs),
                active_mask,
                clean_mask,
            )

        def merge_nodes(
            nodes: tuple[tuple[object, ...], ...],
            level: int,
            chunk_prefix: int,
        ) -> tuple[object, ...]:
            self.join_node_visits += 1
            first = nodes[0]
            if all(node is first for node in nodes[1:]):
                self.join_shared_subtrees_skipped += 1
                return first
            child_count = max((len(node) for node in nodes), default=0)
            children: list[object] = []
            filler: object = _EMPTY_BINDING_CHUNK if level == 0 else ()
            for offset in range(child_count):
                branch = tuple(
                    node[offset] if offset < len(node) else filler for node in nodes
                )
                child_prefix = chunk_prefix | (offset << (level * _BINDING_TREE_SHIFT))
                if level == 0:
                    child = merge_chunks(
                        cast(tuple[_BindingChunk, ...], branch), child_prefix
                    )
                else:
                    child = merge_nodes(
                        cast(tuple[tuple[object, ...], ...], branch),
                        level - 1,
                        child_prefix,
                    )
                children.append(child)
            while children and children[-1] is filler:
                children.pop()
            return tuple(children)

        root = merge_nodes(tuple(roots), depth - 1, 0)
        return _BindingEnvironment(
            root,
            max((environment.chunk_count for environment in environments), default=0),
            depth,
        )

    def intern(self, state: _BindingState) -> int:
        known = self._ids.get(state)
        if known is not None:
            return known
        index = len(self._states)
        self._states.append(state)
        if not state.parents:
            environment = _EMPTY_BINDING_ENVIRONMENT
        elif len(state.parents) == 1:
            environment = self._binding_environments[state.parents[0]]
        else:
            environment = self._joined_environment(state.parents, state.taint_epoch)
        if state.updated_slot >= 0:
            assert state.updated_clean is not None
            environment = self._updated_environment(
                environment,
                state.updated_slot,
                state.updated_value,
                state.updated_static_value,
                state.updated_clean,
                state.taint_epoch,
            )
        for slot, value, static_value, clean in state.updated_bindings:
            environment = self._updated_environment(
                environment,
                slot,
                value,
                static_value,
                clean,
                state.taint_epoch,
            )
        self._binding_environments.append(environment)
        self._ids[state] = index
        return index

    def get(self, state_id: int) -> _BindingState:
        return self._states[state_id]

    def _binding_details(
        self, state_id: int, slot: int
    ) -> tuple[IdentityMask, PythonStaticValue, bool]:
        resolution = self._binding_resolution(state_id, slot)
        value = resolution.identities
        if not resolution.clean:
            value |= OTHER_IDENTITY
        static_value = resolution.static_value if resolution.clean else None
        return value, static_value, resolution.clean

    def binding(self, state_id: int, slot: int) -> IdentityMask:
        self.binding_lookups += 1
        return self._binding_details(state_id, slot)[0]

    def static_value(self, state_id: int, slot: int) -> PythonStaticValue:
        return self._binding_details(state_id, slot)[1]

    def set_binding(
        self,
        state_id: int,
        slot: int,
        value: IdentityMask,
        static_value: PythonStaticValue = None,
    ) -> int:
        return self.set_bindings(state_id, ((slot, value, static_value),))

    def set_bindings(
        self,
        state_id: int,
        bindings: Sequence[tuple[int, IdentityMask, PythonStaticValue]],
    ) -> int:
        updates: list[tuple[int, IdentityMask, PythonStaticValue, bool]] = []
        for slot, value, static_value in bindings:
            current_value, current_static_value, clean = self._binding_details(
                state_id, slot
            )
            if (
                current_value == value
                and current_static_value == static_value
                and clean
            ):
                continue
            updates.append((slot, value, static_value, True))
        if not updates:
            return state_id
        state = self._states[state_id]
        if len(updates) == 1:
            slot, value, static_value, clean = updates[0]
            return self.intern(
                _BindingState(
                    parents=(state_id,),
                    updated_slot=slot,
                    updated_value=value,
                    updated_static_value=static_value,
                    updated_clean=clean,
                    taint_epoch=state.taint_epoch,
                    maybe_invalidated_members=state.maybe_invalidated_members,
                    definitely_invalidated_members=state.definitely_invalidated_members,
                )
            )
        return self.intern(
            _BindingState(
                parents=(state_id,),
                updated_bindings=tuple(updates),
                taint_epoch=state.taint_epoch,
                maybe_invalidated_members=state.maybe_invalidated_members,
                definitely_invalidated_members=state.definitely_invalidated_members,
            )
        )

    def changed_slots_between(self, previous: int, current: int) -> tuple[int, ...]:
        """Return public-identity changes via persistent-trie structural diff."""

        previous_environment = self._binding_environments[previous]
        current_environment = self._binding_environments[current]
        depth = max(previous_environment.depth, current_environment.depth)

        def root_at_depth(
            environment: _BindingEnvironment,
        ) -> tuple[object, ...]:
            root = environment.root
            for _level in range(environment.depth, depth):
                root = (root,) if root else ()
            return root

        changed: set[int] = set()
        pending = [
            (
                root_at_depth(previous_environment),
                root_at_depth(current_environment),
                depth - 1,
                0,
            )
        ]
        while pending:
            previous_node, current_node, level, chunk_prefix = pending.pop()
            self.structural_diff_node_visits += 1
            if previous_node is current_node:
                self.structural_diff_shared_skips += 1
                continue
            child_count = max(len(previous_node), len(current_node))
            filler: object = _EMPTY_BINDING_CHUNK if level == 0 else ()
            for offset in range(child_count):
                previous_child = (
                    previous_node[offset] if offset < len(previous_node) else filler
                )
                current_child = (
                    current_node[offset] if offset < len(current_node) else filler
                )
                if previous_child is current_child:
                    self.structural_diff_shared_skips += 1
                    continue
                child_prefix = chunk_prefix | (offset << (level * _BINDING_TREE_SHIFT))
                if level:
                    pending.append(
                        (
                            cast(tuple[object, ...], previous_child),
                            cast(tuple[object, ...], current_child),
                            level - 1,
                            child_prefix,
                        )
                    )
                    continue
                previous_chunk = cast(_BindingChunk, previous_child)
                current_chunk = cast(_BindingChunk, current_child)
                candidate_mask = (
                    previous_chunk.active_mask
                    | previous_chunk.clean_mask
                    | current_chunk.active_mask
                    | current_chunk.clean_mask
                )
                while candidate_mask:
                    slot_bit = candidate_mask & -candidate_mask
                    candidate_mask ^= slot_bit
                    slot = (child_prefix << _BINDING_CHUNK_SHIFT) | (
                        slot_bit.bit_length() - 1
                    )
                    if (
                        self._binding_details(previous, slot)[:2]
                        != self._binding_details(current, slot)[:2]
                    ):
                        changed.add(slot)
        return tuple(sorted(changed))

    def transition_binding_events(
        self, previous: int, current: int
    ) -> tuple[tuple[int, IdentityMask], ...]:
        direct: dict[int, IdentityMask] = {}
        cursor = current
        while cursor != previous:
            state = self._states[cursor]
            if state.updated_slot >= 0:
                value = state.updated_value
                if state.updated_clean is False and value != UNBOUND_IDENTITY:
                    value |= OTHER_IDENTITY
                direct.setdefault(state.updated_slot, value)
            for slot, value, _static_value, clean in reversed(state.updated_bindings):
                if not clean and value != UNBOUND_IDENTITY:
                    value |= OTHER_IDENTITY
                direct.setdefault(slot, value)
            if len(state.parents) != 1:
                break
            cursor = state.parents[0]
        if cursor == previous:
            return tuple(sorted(direct.items()))
        return tuple(
            (slot, self.binding(current, slot))
            for slot in self.changed_slots_between(previous, current)
        )

    def taint_module_bindings(self, state_id: int) -> int:
        # The namespace can acquire previously undeclared names, even when the
        # module has no statically allocated binding slots (e.g. import-star).
        state = self._states[state_id]
        return self.intern(
            _BindingState(
                parents=(state_id,),
                taint_epoch=state.taint_epoch + 1,
                maybe_invalidated_members=state.maybe_invalidated_members,
                definitely_invalidated_members=state.definitely_invalidated_members,
            )
        )

    def taint_slots(self, state_id: int, slots: int) -> int:
        if slots & self._taint_domain_mask:
            state_id = self.taint_module_bindings(state_id)
            slots &= ~self._taint_domain_mask
        remaining = slots
        while remaining:
            slot_bit = remaining & -remaining
            slot = slot_bit.bit_length() - 1
            remaining ^= slot_bit
            resolution = self._binding_resolution(state_id, slot)
            if not resolution.clean:
                continue
            state = self._states[state_id]
            state_id = self.intern(
                _BindingState(
                    parents=(state_id,),
                    updated_slot=slot,
                    updated_value=resolution.identities,
                    updated_static_value=resolution.static_value,
                    updated_clean=False,
                    taint_epoch=state.taint_epoch,
                    maybe_invalidated_members=state.maybe_invalidated_members,
                    definitely_invalidated_members=(
                        state.definitely_invalidated_members
                    ),
                )
            )
        return state_id

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
            _BindingState(
                parents=(state_id,),
                taint_epoch=state.taint_epoch,
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
            _BindingState(
                parents=(state_id,),
                taint_epoch=state.taint_epoch,
                maybe_invalidated_members=maybe,
                definitely_invalidated_members=definitely,
            )
        )

    def overlay_summary_properties(
        self,
        state_id: int,
        *,
        maybe_invalidated_members: MemberMask,
        definitely_invalidated_members: MemberMask,
        taint_epoch: int,
    ) -> int:
        state = self._states[state_id]
        maybe = state.maybe_invalidated_members | maybe_invalidated_members
        definitely = (
            state.definitely_invalidated_members & definitely_invalidated_members
        )
        epoch = max(state.taint_epoch, taint_epoch)
        if (
            maybe == state.maybe_invalidated_members
            and definitely == state.definitely_invalidated_members
            and epoch == state.taint_epoch
        ):
            return state_id
        return self.intern(
            _BindingState(
                parents=(state_id,),
                taint_epoch=epoch,
                maybe_invalidated_members=maybe,
                definitely_invalidated_members=definitely,
            )
        )

    def slot_in_taint_domain(self, slot: int) -> bool:
        return bool(self._taint_domain_mask & (1 << slot))

    def join(self, *state_ids: int) -> int:
        self.join_calls += 1
        if not state_ids:
            return 0
        parents = tuple(sorted(set(state_ids)))
        if len(parents) == 1:
            return parents[0]
        maybe_invalidated = 0
        definitely_invalidated = ALL_INVALID_MEMBERS
        taint_epoch = 0
        for state_id in parents:
            state = self._states[state_id]
            maybe_invalidated |= state.maybe_invalidated_members
            definitely_invalidated &= state.definitely_invalidated_members
            taint_epoch = max(taint_epoch, state.taint_epoch)
        return self.intern(
            _BindingState(
                parents=parents,
                taint_epoch=taint_epoch,
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
        if self.changed_slots_between(left_id, right_id):
            return False
        remaining = self._taint_domain_mask
        while remaining:
            slot_bit = remaining & -remaining
            remaining ^= slot_bit
            slot = slot_bit.bit_length() - 1
            if (
                self._binding_details(left_id, slot)[:2]
                != self._binding_details(right_id, slot)[:2]
            ):
                return False
        return True

    def __len__(self) -> int:
        return len(self._states)


_ScopeKind = Literal[
    "module", "function", "class", "comprehension", "lambda", "annotation"
]


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
    dynamic_class_namespace: bool = False
    annotation_evaluator: bool = False
    needs_annotation_namespace: bool = False

    def class_namespace_owner(self) -> _Scope | None:
        owner: _Scope | None = self
        while owner is not None and owner.kind == "annotation":
            owner = owner.parent
        return owner if owner is not None and owner.kind == "class" else None

    def namespace_lookup_owner(
        self, name: str, *, write: bool = False
    ) -> _Scope | None:
        # LOAD_CLASSDEREF consults the class mapping before a closure cell,
        # including explicit nonlocals. STORE/DELETE_DEREF bypass that mapping.
        if self.kind == "annotation":
            if write or name in self.locals or name in self.globals:
                return None
            # A deferred 3.14 annotation is a child scope: the class mapping
            # precedes its captured type parameters. The eager 3.12/3.13
            # annotation executes in the parameter scope itself, whose locals
            # precede the class. Do not search ancestor locals before the map.
            owner = self.class_namespace_owner()
        else:
            owner = self if self.kind == "class" else None
        if owner is None or owner.kind != "class" or name in owner.globals:
            return None
        if write and name in owner.nonlocals:
            return None
        return owner

    def namespace_can_call(
        self, name: str, *, write: bool = False, namespace_tainted: bool = False
    ) -> bool:
        owner = self.namespace_lookup_owner(name, write=write)
        return owner is not None and (
            owner.dynamic_class_namespace or namespace_tainted
        )


def _target_names(target: ast.AST | None) -> set[str]:
    if target is None:
        return set()
    if isinstance(target, ast.Name):
        return {target.id}
    if isinstance(target, (ast.Tuple, ast.List)):
        return {name for item in target.elts for name in _target_names(item)}
    if isinstance(target, ast.Starred):
        return _target_names(target.value)
    if isinstance(target, ast.pattern):
        return set(python_pattern_capture_names(target))
    return set()


def _literal_string(node: ast.AST | None) -> str | None:
    return (
        node.value
        if isinstance(node, ast.Constant) and isinstance(node.value, str)
        else None
    )


def _identity_can_release(mask: IdentityMask) -> bool:
    # Identity is not retained-owner custody. Modules/functions can be evicted
    # from registries while private aliases remain exact; dropping the last
    # alias can release contents or run weakref callbacks. Locals/frame owners
    # can similarly escape their defining invocation (including 3.12 locals).
    # Only inert values, absence, and the globals dictionary still rooted by
    # the executing function/frame are intrinsically safe in this abstraction.
    inert = int(
        PythonIdentity.CURRENT_GLOBALS
        | PythonIdentity.INERT_VALUE
        | PythonIdentity.STATIC_FALSE
        | PythonIdentity.UNBOUND
    )
    return bool(mask & ~inert)


@dataclass(frozen=True, slots=True)
class _EvaluatedMemberTarget:
    node: ast.Attribute | ast.Subscript
    owner_identities: IdentityMask
    static_index: PythonStaticValue = None


@dataclass(slots=True)
class _ExpressionResult:
    state_id: int
    identities: IdentityMask
    effects: EffectMask
    static_value: PythonStaticValue = None
    assignment_target: _EvaluatedMemberTarget | None = None


@dataclass(slots=True)
class _FunctionJob:
    node: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda
    parent_scope: _Scope
    outer_state_id: int
    module_history_start: int | None
    module_states: tuple[int, ...] | None
    module_base_bindings: tuple[tuple[int, IdentityMask], ...]
    lexical_history: list[int] | None
    lexical_history_start: int
    lexical_base_bindings: tuple[tuple[int, IdentityMask], ...]
    parameter_default_identities: tuple[tuple[str, IdentityMask], ...]
    annotation_scope: bool = False


@dataclass(slots=True)
class _HistorySummary:
    states: Sequence[int]
    maybe_suffix: tuple[MemberMask, ...]
    definitely_suffix: tuple[MemberMask, ...]
    taint_epoch_suffix: tuple[int, ...]
    taint_indices: tuple[int, ...]
    slot_events: dict[
        int,
        tuple[
            tuple[int, ...],
            tuple[IdentityMask, ...],
            tuple[IdentityMask, ...],
        ],
    ]
    initial_values: dict[int, IdentityMask]

    @classmethod
    def build(cls, pool: _StatePool, states: Sequence[int]) -> _HistorySummary:
        count = len(states)
        maybe_suffix = [0] * count
        definitely_suffix = [ALL_INVALID_MEMBERS] * count
        taint_epoch_suffix = [0] * count
        maybe = 0
        definitely = ALL_INVALID_MEMBERS
        taint_epoch = 0
        for index in range(count - 1, -1, -1):
            state = pool.get(states[index])
            maybe |= state.maybe_invalidated_members
            definitely &= state.definitely_invalidated_members
            taint_epoch = max(taint_epoch, state.taint_epoch)
            maybe_suffix[index] = maybe
            definitely_suffix[index] = definitely
            taint_epoch_suffix[index] = taint_epoch

        events: dict[int, list[tuple[int, IdentityMask]]] = {}
        taint_indices: list[int] = []
        for index in range(1, count):
            previous = states[index - 1]
            current = states[index]
            if pool.get(current).taint_epoch > pool.get(previous).taint_epoch:
                taint_indices.append(index)
            for slot, value in pool.transition_binding_events(previous, current):
                events.setdefault(slot, []).append((index, value))

        slot_events: dict[
            int,
            tuple[
                tuple[int, ...],
                tuple[IdentityMask, ...],
                tuple[IdentityMask, ...],
            ],
        ] = {}
        for slot, rows in events.items():
            suffix_values = [NO_IDENTITIES] * len(rows)
            value = NO_IDENTITIES
            for index in range(len(rows) - 1, -1, -1):
                value |= rows[index][1]
                suffix_values[index] = value
            slot_events[slot] = (
                tuple(row[0] for row in rows),
                tuple(row[1] for row in rows),
                tuple(suffix_values),
            )
        return cls(
            states,
            tuple(maybe_suffix),
            tuple(definitely_suffix),
            tuple(taint_epoch_suffix),
            tuple(taint_indices),
            slot_events,
            {},
        )

    def properties(self, start: int) -> tuple[MemberMask, MemberMask, int]:
        return (
            self.maybe_suffix[start],
            self.definitely_suffix[start],
            self.taint_epoch_suffix[start],
        )

    def binding(self, pool: _StatePool, start: int, slot: int) -> IdentityMask:
        rows = self.slot_events.get(slot)
        if slot not in self.initial_values:
            self.initial_values[slot] = pool.binding(self.states[0], slot)
        base_value = self.initial_values[slot]
        value = base_value
        if rows is not None:
            indices, event_values, suffix_values = rows
            previous_event = bisect_right(indices, start) - 1
            if previous_event >= 0:
                base_value = event_values[previous_event]
                value = base_value
            event_index = bisect_left(indices, start)
            if event_index < len(indices):
                value |= suffix_values[event_index]
        if pool.slot_in_taint_domain(slot):
            taint_index = bisect_left(self.taint_indices, start)
            if taint_index < len(self.taint_indices):
                # A callback may insert a previously absent binding as well
                # as replace an existing one. Deferred overlays retain both.
                value |= OTHER_IDENTITY
        return value


class _Analyzer:
    def __init__(self, policy: PythonBindingPolicy, source_digest: str) -> None:
        self.policy = policy
        self.source_digest = source_digest
        self.states = _StatePool()
        self.scopes: list[_Scope] = []
        self.slot_names: list[str] = []
        self.expressions: dict[PythonNodeKey, PythonExpressionFact] = {}
        self.statements: dict[PythonNodeKey, PythonStatementFact] = {}
        self.iterations: dict[PythonNodeKey, PythonIterationFact] = {}
        self.assignment_effects: dict[PythonNodeKey, EffectMask] = {}
        self._namespace_observation_epoch = 0
        self._future_annotations = False
        self._dependency_authority: PythonDependencyAuthority | None = None
        self.calls: dict[PythonNodeKey, PythonCallSiteFact] = {}
        self.function_jobs: list[_FunctionJob] = []
        self._queued_function_jobs: dict[
            tuple[PythonNodeKey, int, bool], _FunctionJob
        ] = {}
        self._source_scopes: dict[
            tuple[PythonNodeKey, int | None, _ScopeKind], _Scope
        ] = {}
        self._active_lexical_history: list[int] | None = None
        self.module_scope: _Scope | None = None
        self.module_slots: list[int] = []
        self.module_slot_mask = 0
        self.callback_slot_mask = 0
        self._observed_stack: list[list[int]] = []
        self._module_history: list[int] = [0]
        self._active_module_states: tuple[int, ...] | None = None
        self._module_import_flow_required = False
        self._scope_slot_cache: dict[tuple[int, str], int | None] = {}
        self._history_summaries: dict[int, _HistorySummary] = {}
        # AST instances use identity equality/hash. Keeping the node itself as the
        # key both avoids repeated source-key construction and retains synthetic
        # nodes for the analysis lifetime, so CPython cannot recycle an id into a
        # false cache hit.
        self._node_keys: dict[ast.AST, PythonNodeKey] = {}

    @property
    def _eager_annotations(self) -> bool:
        return self.policy.target_python < (3, 14) and not self._future_annotations

    def _node_key(self, node: ast.AST) -> PythonNodeKey:
        key = self._node_keys.get(node)
        if key is None:
            key = PythonNodeKey.from_node(node)
            self._node_keys[node] = key
        return key

    @staticmethod
    def _target_may_write_import_metadata(target: ast.AST) -> bool:
        if isinstance(target, (ast.Tuple, ast.List)):
            return any(
                _Analyzer._target_may_write_import_metadata(element)
                for element in target.elts
            )
        return import_metadata_target_name(target) in _METADATA_NAMES

    def _expression_exposes_module_globals(self, expression: ast.AST) -> bool:
        pending = [expression]
        while pending:
            node = pending.pop()
            fact = self.expressions.get(self._node_key(node))
            if fact is not None and fact.identities & int(
                PythonIdentity.CURRENT_GLOBALS
            ):
                return True
            pending.extend(ast.iter_child_nodes(node))
        return False

    def _queue_function(
        self,
        node: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda,
        scope: _Scope,
        state_id: int,
        parameter_default_identities: tuple[tuple[str, IdentityMask], ...] = (),
        *,
        annotation_scope: bool = False,
    ) -> None:
        module_slots, lexical_slots = self._deferred_slot_dependencies(
            node, scope, annotation_scope=annotation_scope
        )
        self.callback_slot_mask |= sum(1 << slot for slot in lexical_slots)
        self.states.set_taint_domain(self.module_slot_mask | self.callback_slot_mask)
        lexical_history = self._active_lexical_history if lexical_slots else None
        job = _FunctionJob(
            node,
            scope,
            state_id,
            None
            if self._active_module_states is not None
            else len(self._module_history),
            self._active_module_states,
            tuple((slot, self.states.binding(state_id, slot)) for slot in module_slots),
            lexical_history,
            len(lexical_history) if lexical_history is not None else 0,
            tuple(
                (slot, self.states.binding(state_id, slot)) for slot in lexical_slots
            ),
            parameter_default_identities,
            annotation_scope,
        )
        key = (self._node_key(node), scope.scope_id, annotation_scope)
        previous = self._queued_function_jobs.get(key)
        if previous is None:
            self._queued_function_jobs[key] = job
            self.function_jobs.append(job)
            return
        # A source-owned definition has one lexical history owner even when
        # loop/finally predecessors visit its creation point repeatedly.
        if previous.lexical_history is not lexical_history:
            raise RuntimeError(
                "deferred source definition changed lexical history owner"
            )
        previous.outer_state_id = self.states.join(previous.outer_state_id, state_id)

        def merge_bindings[K](
            left: tuple[tuple[K, IdentityMask], ...],
            right: tuple[tuple[K, IdentityMask], ...],
        ) -> tuple[tuple[K, IdentityMask], ...]:
            merged = dict(left)
            for name, identities in right:
                merged[name] = merged.get(name, NO_IDENTITIES) | identities
            return tuple(merged.items())

        previous.module_base_bindings = merge_bindings(
            previous.module_base_bindings, job.module_base_bindings
        )
        previous.lexical_base_bindings = merge_bindings(
            previous.lexical_base_bindings, job.lexical_base_bindings
        )
        previous.parameter_default_identities = merge_bindings(
            previous.parameter_default_identities, parameter_default_identities
        )
        previous.lexical_history_start = min(
            previous.lexical_history_start, job.lexical_history_start
        )
        if previous.module_states is None or job.module_states is None:
            if previous.module_states is not job.module_states:
                raise RuntimeError(
                    "deferred source definition changed module history owner"
                )
            assert previous.module_history_start is not None
            assert job.module_history_start is not None
            previous.module_history_start = min(
                previous.module_history_start, job.module_history_start
            )
        else:
            previous.module_states = tuple(
                sorted(set((*previous.module_states, *job.module_states)))
            )

    def _deferred_slot_dependencies(
        self,
        node: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda,
        parent_scope: _Scope,
        *,
        annotation_scope: bool = False,
    ) -> tuple[tuple[int, ...], tuple[int, ...]]:
        assert self._dependency_authority is not None
        dependencies = self._dependency_authority.summary(node).body
        assert self.module_scope is not None
        module_slots: set[int] = {
            slot
            for name in dependencies.globals
            if (slot := self.module_scope.slots.get(name)) is not None
        }
        lexical_slots: set[int] = set()
        for name in dependencies.lexical:
            slot = None
            visible: _Scope | None = parent_scope
            while visible is not None:
                candidate = (
                    visible.slots.get(name)
                    if visible.kind != "class"
                    or annotation_scope
                    and visible is parent_scope
                    else None
                )
                if candidate is not None:
                    slot = candidate
                    break
                visible = visible.parent
            if slot is None:
                continue
            if slot in self.module_slots:
                module_slots.add(slot)
            else:
                lexical_slots.add(slot)
        return tuple(sorted(module_slots)), tuple(sorted(lexical_slots))

    def _queue_annotation_expression(
        self,
        expression: ast.expr,
        scope: _Scope,
        state_id: int,
        type_params: Sequence[ast.AST],
    ) -> None:
        parameters = [
            ast.arg(arg=name)
            for type_param in type_params
            if isinstance((name := getattr(type_param, "name", None)), str)
        ]
        deferred = ast.copy_location(
            ast.Lambda(
                args=ast.arguments(
                    posonlyargs=[],
                    args=parameters,
                    vararg=None,
                    kwonlyargs=[],
                    kw_defaults=[],
                    kwarg=None,
                    defaults=[],
                ),
                body=expression,
            ),
            expression,
        )
        self._queue_function(deferred, scope, state_id, annotation_scope=True)

    def _type_parameter_scope(
        self, node: ast.AST, state_id: int, parent: _Scope
    ) -> tuple[int, _Scope]:
        parameters = tuple(getattr(node, "type_params", ()))
        if not parameters:
            return state_id, parent
        name = getattr(node, "name", "alias")
        if isinstance(name, ast.Name):
            name = name.id
        scope = self._new_scope(
            parent=parent,
            source_node=node,
            kind="annotation",
            name=f"<{name} type parameters>",
            declarations=PythonScopeDeclarations(
                frozenset(parameter.name for parameter in parameters),
                frozenset(),
                frozenset(),
            ),
        )
        state_id = self.states.set_bindings(
            state_id,
            tuple((slot, OTHER_IDENTITY, None) for slot in scope.slots.values()),
        )
        for expression in type_parameter_expressions(parameters):
            self._queue_annotation_expression(expression, scope, state_id, ())
        return state_id, scope

    def _new_scope(
        self,
        *,
        parent: _Scope | None,
        source_node: ast.AST,
        kind: _ScopeKind,
        name: str,
        declarations: PythonScopeDeclarations,
    ) -> _Scope:
        key = (
            self._node_key(source_node),
            parent.scope_id if parent is not None else None,
            kind,
        )
        existing = self._source_scopes.get(key)
        if existing is not None:
            return existing
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
                self._ensure_module_slot(global_name)
        for local_name in sorted(scope.locals):
            scope.slots[local_name] = len(self.slot_names)
            self.slot_names.append(f"{scope.scope_id}:{local_name}")
        if kind == "class":
            # Class bodies execute against a mapping, not private fast locals.
            self.callback_slot_mask |= sum(1 << slot for slot in scope.slots.values())
            self.states.set_taint_domain(
                self.module_slot_mask | self.callback_slot_mask
            )
        self.scopes.append(scope)
        self._source_scopes[key] = scope
        return scope

    def _ensure_module_slot(self, name: str) -> int:
        """One slot authority for global declarations and reflective item writes."""
        assert self.module_scope is not None
        existing = self.module_scope.slots.get(name)
        if existing is not None:
            return existing
        slot = len(self.slot_names)
        self.module_scope.slots[name] = slot
        self.slot_names.append(f"{self.module_scope.scope_id}:{name}")
        self.module_slots.append(slot)
        self.module_slot_mask |= 1 << slot
        self.states.set_taint_domain(self.module_slot_mask | self.callback_slot_mask)
        self._scope_slot_cache.clear()
        return slot

    def _module_slot(self, name: str) -> int | None:
        assert self.module_scope is not None
        return self.module_scope.slots.get(name)

    def _nonlocal_slot(self, scope: _Scope, name: str) -> int | None:
        parent = scope.parent
        while parent is not None and parent.kind != "module":
            slot = parent.slots.get(name) if parent.kind != "class" else None
            if slot is not None:
                return slot
            parent = parent.parent
        return None

    def _slot_for_name(self, scope: _Scope, name: str) -> int | None:
        cache_key = (scope.scope_id, name)
        if cache_key in self._scope_slot_cache:
            return self._scope_slot_cache[cache_key]
        if name in scope.globals:
            slot = self._module_slot(name)
        elif name in scope.nonlocals:
            slot = self._nonlocal_slot(scope, name)
        elif (
            scope.kind == "annotation"
            and name not in scope.locals
            and (class_owner := scope.class_namespace_owner()) is not None
            and name in class_owner.globals
        ):
            # Class global directives carry into class-visible annotation
            # scopes, but never override a current scope's own type parameter.
            slot = self._module_slot(name)
        else:
            slot = scope.slots.get(name)
            parent = scope.parent
            while slot is None and parent is not None:
                if (
                    parent.kind != "class"
                    or scope.kind == "annotation"
                    and parent is scope.parent
                ):
                    slot = parent.slots.get(name)
                parent = parent.parent
        self._scope_slot_cache[cache_key] = slot
        return slot

    def _read_name_resolution(
        self, state_id: int, scope: _Scope, name: str
    ) -> _BindingResolution:
        if scope.namespace_can_call(
            name, namespace_tainted=self.states.get(state_id).taint_epoch != 0
        ):
            # __prepare__ may return an arbitrary mapping, and an initially
            # exact dictionary can gain callback-bearing keys via reflection.
            # A prior STORE_NAME cannot establish the next lookup's behavior.
            return _BindingResolution(OTHER_IDENTITY | UNBOUND_IDENTITY, None, False)
        slot = self._slot_for_name(scope, name)
        own = (
            _BindingResolution(*self.states._binding_details(state_id, slot))
            if slot is not None
            else _UNBOUND_BINDING_RESOLUTION
        )
        if (
            scope.kind == "annotation"
            and name not in scope.locals
            and name not in scope.globals
            and (class_owner := scope.class_namespace_owner()) is not None
            and name not in class_owner.globals
        ):
            # Class-visible annotations observe live namespace contents, even
            # through an intervening type-parameter scope. An undeclared name
            # can be added before deferred evaluation just like a declared one.
            return _BindingResolution(OTHER_IDENTITY | UNBOUND_IDENTITY, None, False)
        if (
            scope.kind == "class"
            and name in scope.locals
            and own.identities & UNBOUND_IDENTITY
        ):
            # LOAD_NAME: an unbound class-local falls back to the module, not
            # the enclosing class or an identically named closure cell.
            global_slot = self._module_slot(name)
            fallback = (
                _BindingResolution(*self.states._binding_details(state_id, global_slot))
                if global_slot is not None
                else _UNBOUND_BINDING_RESOLUTION
            )
            if own.identities == UNBOUND_IDENTITY:
                return fallback
            return _BindingResolution(
                (own.identities & ~UNBOUND_IDENTITY) | fallback.identities,
                None,
                own.clean and fallback.clean,
            )
        return own

    def _resolve_name(
        self, state_id: int, scope: _Scope, name: str
    ) -> tuple[IdentityMask, PythonStaticValue, bool, bool]:
        """One source-point lookup owns value, cleanliness and storage facts."""
        slot = self._slot_for_name(scope, name)
        self.states.binding_lookups += 1
        resolution = self._read_name_resolution(state_id, scope, name)
        value = resolution.identities
        local_miss = (
            scope.kind in {"function", "lambda", "comprehension", "annotation"}
            and name in scope.locals
        )
        if value == UNBOUND_IDENTITY and not local_miss:
            value = _BUILTIN_IDENTITIES.get(name, OTHER_IDENTITY | UNBOUND_IDENTITY)
            if name == "__import__":
                state = self.states.get(state_id)
                guard = int(PythonMember.BUILTINS_IMPORT)
                if state.definitely_invalidated_members & guard:
                    value = OTHER_IDENTITY
                elif state.maybe_invalidated_members & guard:
                    value |= OTHER_IDENTITY
        lexical_cell = (
            scope.kind != "class"
            and slot is not None
            and not self.states.slot_in_taint_domain(slot)
        )
        invalidated = (
            not lexical_cell
            and self.states.get(state_id).taint_epoch != 0
            and not resolution.clean
        )
        bound = lexical_cell or resolution.identities != UNBOUND_IDENTITY
        return (
            value,
            resolution.static_value if resolution.clean else None,
            invalidated,
            bound,
        )

    def _record_state(self, state_id: int) -> None:
        # Record a transfer when it happens, never when a completion partition
        # later republishes its summary. Replaying old exceptional states after
        # a definition would fabricate impossible future closure environments.
        for observed in self._observed_stack:
            observed.append(state_id)
        if self._active_module_states is None:
            self._module_history.append(state_id)

    def _record_expression(
        self,
        node: ast.AST,
        scope: _Scope,
        identities: IdentityMask,
        effects: EffectMask,
        static_value: PythonStaticValue,
        binding_invalidated: bool = False,
        binding_is_bound: bool = False,
        module_namespace_observable: bool = False,
    ) -> None:
        key = self._node_key(node)
        result = (
            static_expression_result(node, fact_result=self._known_expression_result)
            if isinstance(node, ast.expr)
            else UNKNOWN_EXPRESSION_RESULT
        )
        if isinstance(node, (ast.Name, ast.Attribute)):
            if identities == int(PythonIdentity.STATIC_FALSE):
                result = StaticExpressionResult.scalar(
                    False,
                    evaluation_required=isinstance(node, ast.Attribute)
                    or effects != NO_EFFECTS,
                )
            elif static_value is not None and type(static_value) in {str, int}:
                result = StaticExpressionResult.scalar(
                    static_value, evaluation_required=True
                )
            else:
                result = UNKNOWN_EXPRESSION_RESULT
        name_lookup: PythonNameLookup = "none"
        if isinstance(node, ast.Name):
            slot = self._slot_for_name(scope, node.id)
            if (owner := scope.namespace_lookup_owner(node.id)) is not None:
                if scope.annotation_evaluator:
                    owner.needs_annotation_namespace = True
                # Class locals use globals on a mapping miss, never an outer
                # same-spelled local or the method's captured type parameter.
                name_lookup = (
                    "class_lexical"
                    if node.id not in owner.locals
                    and slot is not None
                    and not self.module_slot_mask & (1 << slot)
                    else "class_global"
                )
            elif (
                slot is not None
                and not self.module_slot_mask & (1 << slot)
                or scope.kind in {"function", "lambda", "comprehension", "annotation"}
                and node.id in scope.locals
                and node.id not in scope.globals
            ):
                name_lookup = "lexical"
            else:
                name_lookup = "global"
        previous = self.expressions.get(key)
        self.expressions[key] = PythonExpressionFact(
            key,
            scope.scope_id,
            identities
            | (previous.identities if previous is not None else NO_IDENTITIES),
            effects | (previous.effects if previous is not None else NO_EFFECTS),
            static_value
            if previous is None or previous.static_value == static_value
            else None,
            binding_invalidated
            or (previous is not None and previous.binding_invalidated),
            binding_is_bound and (previous is None or previous.binding_is_bound),
            result
            if previous is None or previous.result == result
            else UNKNOWN_EXPRESSION_RESULT,
            module_namespace_observable
            or (previous is not None and previous.module_namespace_observable),
            previous.truth_effects if previous is not None else NO_EFFECTS,
            name_lookup,
        )

    def _known_expression_result(self, node: ast.expr) -> StaticExpressionResult:
        fact = self.expressions.get(self._node_key(node))
        return UNKNOWN_EXPRESSION_RESULT if fact is None else fact.result

    def _widen_module_bindings(self, state_id: int) -> int:
        self._namespace_observation_epoch += 1
        return self.states.taint_module_bindings(state_id)

    def _apply_effects(self, state_id: int, effects: EffectMask) -> int:
        if effects & (REFLECTS_NAMESPACE | READS_GLOBAL_NAMESPACE | READS_FRAME_STATE):
            self._namespace_observation_epoch += 1
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

        def admitted(
            owner: PythonIdentity, guard: PythonMember, result: PythonIdentity
        ) -> IdentityMask:
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
        elif member == "require_intrinsic":
            value |= admitted(
                PythonIdentity.INTRINSICS_MODULE,
                PythonMember.INTRINSICS_REQUIRE,
                PythonIdentity.INTRINSICS_REQUIRE,
            )
        elif member in {"f_globals", "__globals__"}:
            owners = int(PythonIdentity.CURRENT_FRAME | PythonIdentity.USER_FUNCTION)
            if base & owners:
                value |= exact_identity(PythonIdentity.CURRENT_GLOBALS)
                if base & ~owners:
                    value |= OTHER_IDENTITY
        elif member in {"__setitem__", "__delitem__"}:
            if base & int(PythonIdentity.CURRENT_GLOBALS):
                value |= int(
                    PythonIdentity.GLOBALS_SETITEM
                    if member == "__setitem__"
                    else PythonIdentity.GLOBALS_DELITEM
                )
                if base != int(PythonIdentity.CURRENT_GLOBALS):
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
        if (
            base & int(PythonIdentity.IMPORTLIB_MACHINERY_MODULE)
            and member == "ModuleSpec"
        ):
            members |= int(PythonMember.MACHINERY_MODULE_SPEC)
        if base & int(PythonIdentity.IMPORTLIB_UTIL_MODULE) and member == "find_spec":
            members |= int(PythonMember.UTIL_FIND_SPEC)
        if base & int(PythonIdentity.TYPING_MODULE) and member == "TYPE_CHECKING":
            members |= int(PythonMember.TYPING_TYPE_CHECKING)
        if (
            base & int(PythonIdentity.INTRINSICS_MODULE)
            and member == "require_intrinsic"
        ):
            members |= int(PythonMember.INTRINSICS_REQUIRE)
        if base & int(PythonIdentity.MODULE_SPEC_CLASS):
            members |= int(PythonMember.MODULE_SPEC_CLASS)
        if base & int(PythonIdentity.BUILTINS_MODULE) and member == "__import__":
            members |= int(PythonMember.BUILTINS_IMPORT | PythonMember.IMPORT_HOOKS)
        if base & int(PythonIdentity.SYS_MODULE) and member in {
            "meta_path",
            "path_hooks",
        }:
            members |= int(PythonMember.IMPORT_HOOKS)
        if base & int(PythonIdentity.SYS_MODULE) and member == "platform":
            members |= int(PythonMember.SYS_PLATFORM)
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
            int(PythonIdentity.GLOBALS_SETITEM),
            int(PythonIdentity.GLOBALS_DELITEM),
        }:
            deleting = callee == int(PythonIdentity.GLOBALS_DELITEM)
            arity = 1 if deleting else 2
            if (
                len(node.args) == arity
                and not node.keywords
                and not any(isinstance(arg, ast.Starred) for arg in node.args)
            ):
                key_fact = self.expressions.get(self._node_key(node.args[0]))
                value_fact = (
                    None
                    if deleting
                    else self.expressions.get(self._node_key(node.args[1]))
                )
                state_id, mutation_effects = self._store_namespace_key(
                    state_id,
                    None if key_fact is None else key_fact.static_value,
                    UNBOUND_IDENTITY
                    if deleting
                    else (
                        OTHER_IDENTITY if value_fact is None else value_fact.identities
                    ),
                    None if value_fact is None else value_fact.static_value,
                )
                key = self._node_key(node)
                self.assignment_effects[key] = (
                    self.assignment_effects.get(key, NO_EFFECTS) | mutation_effects
                )
                # Callee/arguments have executed; publication applies only its
                # own release effects. Reapplying aggregate namespace writes
                # here would erase the exact replacement we just established.
                return (
                    int(PythonIdentity.INERT_VALUE),
                    effects | mutation_effects,
                    state_id,
                )
            else:
                effects |= UNKNOWN_EFFECTS
            result = int(PythonIdentity.INERT_VALUE)
        elif (
            exact
            and callee
            in {int(PythonIdentity.BUILTIN_LOCALS), int(PythonIdentity.BUILTIN_VARS)}
            and not node.args
            and not node.keywords
        ):
            result = exact_identity(
                PythonIdentity.CURRENT_GLOBALS
                if scope.kind == "module"
                else PythonIdentity.CURRENT_LOCALS
            )
            effects |= REFLECTS_NAMESPACE | READS_FRAME_STATE
        elif exact and callee == int(PythonIdentity.INSPECT_CURRENTFRAME):
            result = exact_identity(PythonIdentity.CURRENT_FRAME)
            effects |= READS_FRAME_STATE | REFLECTS_NAMESPACE | ALLOCATES
        elif callee & int(PythonIdentity.MODULE_SPEC_CLASS):
            result = exact_identity(PythonIdentity.MODULE_SPEC_INSTANCE)
            member_state = self.states.get(state_id)
            if not exact or member_state.maybe_invalidated_members & int(
                PythonMember.MODULE_SPEC_CLASS
            ):
                result |= OTHER_IDENTITY
            effects |= ALLOCATES | RAISES
        elif callee & int(
            PythonIdentity.IMPORTLIB_IMPORT_MODULE
            | PythonIdentity.IMPORTLIB_FIND_SPEC
            | PythonIdentity.BUILTINS_IMPORT
        ):
            result = OTHER_IDENTITY
            effects |= (
                EXECUTES_ARBITRARY_PYTHON | INVOKES_IMPORT_SYSTEM | ALLOCATES | RAISES
            )
        elif exact and callee == int(PythonIdentity.BUILTIN_SETATTR):
            effects |= (
                WRITES_OBJECT_STATE
                | INVOKES_DESCRIPTOR
                | EXECUTES_ARBITRARY_PYTHON
                | _RELEASE_CALLBACK_EFFECTS
                | RAISES
            )
            if len(node.args) >= 2:
                owner = self._expression_identity(node.args[0])
                member = _literal_string(node.args[1])
                if member is not None:
                    state_id = self._invalidate_member_target(state_id, owner, member)
                    if (
                        owner & int(PythonIdentity.CURRENT_MODULE)
                        and member in _METADATA_NAMES
                    ):
                        effects |= WRITES_MODULE_METADATA | WRITES_GLOBAL_NAMESPACE
        elif callee & int(PythonIdentity.BUILTIN_EVAL | PythonIdentity.BUILTIN_EXEC):
            effects |= UNKNOWN_EFFECTS
        else:
            effects |= UNKNOWN_EFFECTS
        return result, effects, self._apply_effects(state_id, effects)

    def _expression_identity(self, node: ast.AST) -> IdentityMask:
        fact = self.expressions.get(self._node_key(node))
        return fact.identities if fact is not None else OTHER_IDENTITY

    def _eval_truth_test(
        self, node: ast.expr, state_id: int, scope: _Scope
    ) -> _ExpressionResult:
        result = self.eval_expr(node, state_id, scope)
        # Identity comparisons and exact inert constants produce builtin truth
        # values. An unknown object's __bool__/__len__ executes before either
        # successor and can publish or replace module bindings.
        truth_effects = (
            NO_EFFECTS
            if self._known_expression_result(node).kind != "unknown"
            or result.identities
            in {
                int(PythonIdentity.INERT_VALUE),
                int(PythonIdentity.STATIC_FALSE),
            }
            else INVOKES_COMPARISON_CALLBACK | RAISES
        )
        key = self._node_key(node)
        fact = self.expressions[key]
        self.expressions[key] = replace(
            fact, truth_effects=fact.truth_effects | truth_effects
        )
        return _ExpressionResult(
            self._apply_effects(result.state_id, truth_effects),
            result.identities,
            result.effects | truth_effects,
            result.static_value,
        )

    def eval_expr(
        self, node: ast.expr, state_id: int, scope: _Scope
    ) -> _ExpressionResult:
        observation_before = self._namespace_observation_epoch
        effects = NO_EFFECTS
        identities = OTHER_IDENTITY
        static_value: PythonStaticValue = None
        binding_invalidated = False
        binding_is_bound = False
        effects_applied = False
        assignment_target: _EvaluatedMemberTarget | None = None
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
            if scope.namespace_can_call(
                node.id, namespace_tainted=self.states.get(state_id).taint_epoch != 0
            ):
                effects |= READS_OBJECT_STATE | EXECUTES_ARBITRARY_PYTHON | RAISES
            identities, static_value, binding_invalidated, binding_is_bound = (
                self._resolve_name(state_id, scope, node.id)
            )
            if identities & UNBOUND_IDENTITY or binding_invalidated:
                effects |= RAISES
        elif isinstance(node, ast.Attribute):
            assignment_target, state_id, effects = self._evaluate_member_target(
                node, state_id, scope
            )
            owner_identities = assignment_target.owner_identities
            identities = self._member_value(state_id, owner_identities, node.attr)
            if (
                node.attr == "platform"
                and owner_identities == int(PythonIdentity.SYS_MODULE)
                and self.policy.target_sys_platform is not None
                and not self.states.get(state_id).maybe_invalidated_members
                & int(PythonMember.SYS_PLATFORM)
            ):
                identities = int(PythonIdentity.INERT_VALUE)
                static_value = self.policy.target_sys_platform
            effects |= READS_OBJECT_STATE | RAISES
            if identities == OTHER_IDENTITY:
                effects |= INVOKES_DESCRIPTOR | EXECUTES_ARBITRARY_PYTHON
        elif isinstance(node, ast.Subscript):
            assignment_target, state_id, effects = self._evaluate_member_target(
                node, state_id, scope
            )
            owner_identities = assignment_target.owner_identities
            effects |= READS_OBJECT_STATE | RAISES
            if (
                owner_identities & int(PythonIdentity.SYS_MODULES)
                and isinstance(node.slice, ast.Name)
                and node.slice.id == "__name__"
            ):
                identities = exact_identity(PythonIdentity.CURRENT_MODULE)
                if owner_identities != int(PythonIdentity.SYS_MODULES):
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
            effects_applied = True
            callee_result = self.eval_expr(node.func, state_id, scope)
            state_id = callee_result.state_id
            effects |= callee_result.effects
            state_id, argument_effects = self._eval_call_arguments(
                node, state_id, scope
            )
            effects |= argument_effects
            callee_may_require_intrinsic = bool(
                callee_result.identities & int(PythonIdentity.INTRINSICS_REQUIRE)
            )
            if (
                callee_result.identities
                & int(PythonIdentity.BUILTIN_EXEC | PythonIdentity.BUILTIN_EVAL)
                or not callee_may_require_intrinsic
                and any(
                    self._expression_exposes_module_globals(argument)
                    for argument in (
                        node.func,
                        *node.args,
                        *(keyword.value for keyword in node.keywords),
                    )
                )
                or isinstance(node.func, ast.Name)
                and node.func.id == "setattr"
                and len(node.args) >= 2
                and _literal_string(node.args[1]) in _METADATA_NAMES
            ):
                self._module_import_flow_required = True
            identities, effects, state_id = self._call_semantics(
                state_id, scope, node, callee_result.identities, effects
            )
            key = self._node_key(node)
            previous_call = self.calls.get(key)
            after = self.states.get(state_id)
            self.calls[key] = PythonCallSiteFact(
                key,
                scope.scope_id,
                callee_result.identities
                | (
                    previous_call.callee_identities
                    if previous_call is not None
                    else NO_IDENTITIES
                ),
                identities
                | (
                    previous_call.result_identities
                    if previous_call is not None
                    else NO_IDENTITIES
                ),
                effects
                | (previous_call.effects if previous_call is not None else NO_EFFECTS),
                after.maybe_invalidated_members
                | (
                    previous_call.maybe_invalidated_members_after
                    if previous_call is not None
                    else NO_INVALID_MEMBERS
                ),
                after.definitely_invalidated_members
                & (
                    previous_call.definitely_invalidated_members_after
                    if previous_call is not None
                    else ALL_INVALID_MEMBERS
                ),
            )
        elif isinstance(node, ast.NamedExpr):
            if self._target_may_write_import_metadata(node.target):
                self._module_import_flow_required = True
            effects_applied = True
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
            state_id, defaults_effect, defaults = self._eval_arguments(
                node.args, state_id, scope
            )
            effects |= defaults_effect | ALLOCATES
            identities = exact_identity(PythonIdentity.USER_FUNCTION)
            self._queue_function(node, scope, state_id, defaults)
        elif isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.Not):
            effects_applied = True
            operand = self._eval_truth_test(node.operand, state_id, scope)
            state_id, effects = operand.state_id, operand.effects
            identities = int(PythonIdentity.INERT_VALUE)
        elif isinstance(node, ast.IfExp):
            effects_applied = True
            test = self._eval_truth_test(node.test, state_id, scope)
            truth = self._known_expression_result(node.test).truth
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
                effects = test.effects | left.effects | right.effects
        elif isinstance(node, ast.BoolOp):
            effects_applied = True
            possible_exits: list[int] = []
            identities = NO_IDENTITIES
            current = state_id
            for index, value in enumerate(node.values):
                final_value = index == len(node.values) - 1
                result = (
                    self.eval_expr(value, current, scope)
                    if final_value
                    else self._eval_truth_test(value, current, scope)
                )
                effects |= result.effects
                truth = self._known_expression_result(value).truth
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
                current = result.state_id
            state_id = self.states.join(*possible_exits)
        elif isinstance(node, ast.Compare):
            effects_applied = True
            left = self.eval_expr(node.left, state_id, scope)
            left_fact = self._known_expression_result(node.left)
            state_id = left.state_id
            effects |= left.effects
            exits: list[int] = []
            identities = NO_IDENTITIES
            for index, (operator, comparator) in enumerate(
                zip(node.ops, node.comparators, strict=True)
            ):
                right = self.eval_expr(comparator, state_id, scope)
                right_fact = self._known_expression_result(comparator)
                state_id = right.state_id
                effects |= right.effects
                comparison = static_comparison_result(left_fact, operator, right_fact)
                boundary = NO_EFFECTS
                if (
                    not isinstance(operator, (ast.Is, ast.IsNot))
                    and comparison.truth is None
                ):
                    boundary |= (
                        EXECUTES_ARBITRARY_PYTHON | INVOKES_COMPARISON_CALLBACK | RAISES
                    )
                final = index == len(node.ops) - 1
                if not final and comparison.kind == "unknown":
                    boundary |= INVOKES_COMPARISON_CALLBACK | RAISES
                # Operands and intermediate results can be released between
                # comparisons. Conservatively invalidate before the next read,
                # even when a particular execution retains the middle operand.
                # Pure loads keep their namespace owners alive. Otherwise use
                # the shared result's recursive ownership fact, including
                # dictionary values (which membership shape alone omits).
                left_node = node.left if index == 0 else node.comparators[index - 1]
                stable_loads = (
                    scope.kind != "class"
                    and isinstance(left_node, (ast.Name, ast.Constant))
                    and isinstance(comparator, (ast.Name, ast.Constant))
                    and boundary == NO_EFFECTS
                )
                if not stable_loads and (
                    _identity_can_release(left.identities)
                    and left_fact.release_may_call
                    or _identity_can_release(right.identities)
                    and right_fact.release_may_call
                ):
                    boundary |= _RELEASE_CALLBACK_EFFECTS
                effects |= boundary
                state_id = self._apply_effects(state_id, boundary)
                if final or comparison.truth is not True:
                    exits.append(state_id)
                    identities |= (
                        int(PythonIdentity.INERT_VALUE)
                        if comparison.kind == "bool"
                        else OTHER_IDENTITY
                    )
                if final or comparison.truth is False:
                    break
                left, left_fact = right, right_fact
            state_id = self.states.join(*exits)
        elif isinstance(
            node, (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)
        ):
            state_id, comp_effects = self._eval_comprehension(node, state_id, scope)
            identities = exact_identity(PythonIdentity.OTHER)
            effects |= comp_effects | ALLOCATES
            if not isinstance(node, ast.GeneratorExp):
                effects |= (
                    INVOKES_ITERATION_CALLBACK | EXECUTES_ARBITRARY_PYTHON | RAISES
                )
        elif isinstance(node, ast.Starred):
            effects_applied = True
            value = self.eval_expr(node.value, state_id, scope)
            boundary = iterable_unpack_effects(
                node.value,
                fact_result=self._known_expression_result,
            )
            effects = value.effects | boundary
            state_id = self._apply_effects(value.state_id, boundary)
        elif isinstance(node, (ast.Tuple, ast.List, ast.Set)):
            effects_applied = True
            keys = AccumulatedKeyEffects()
            pending_hash = NO_EFFECTS
            element_static_values: list[PythonStaticValue] = []
            for element in node.elts:
                if isinstance(element, ast.Starred):
                    state_id = self._apply_effects(state_id, pending_hash)
                    pending_hash = NO_EFFECTS
                result = self.eval_expr(element, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
                element_static_values.append(result.static_value)
                if isinstance(node, ast.Set):
                    key_result = static_expression_result(
                        element.value if isinstance(element, ast.Starred) else element,
                        fact_result=self._known_expression_result,
                    )
                    hashing = (
                        keys.extend(key_result)
                        if isinstance(element, ast.Starred)
                        else keys.add(key_result)
                    )
                    effects |= hashing
                    pending_hash |= hashing
                    # Conservatively cover incremental SET_ADD and batched
                    # BUILD_SET scheduling; flush again at segment boundaries.
                    state_id = self._apply_effects(state_id, hashing)
            state_id = self._apply_effects(state_id, pending_hash)
            identities = exact_identity(PythonIdentity.INERT_VALUE) | OTHER_IDENTITY
            effects |= ALLOCATES
            if not isinstance(node, ast.Set) and all(
                isinstance(value, str) for value in element_static_values
            ):
                static_value = tuple(
                    cast(str, value) for value in element_static_values
                )
        elif isinstance(node, ast.Dict):
            effects_applied = True
            keys = AccumulatedKeyEffects()
            pending_hash = NO_EFFECTS
            for key, value in zip(node.keys, node.values):
                if key is None:
                    state_id = self._apply_effects(state_id, pending_hash)
                    pending_hash = NO_EFFECTS
                if key is not None:
                    result = self.eval_expr(key, state_id, scope)
                    state_id = result.state_id
                    effects |= result.effects
                result = self.eval_expr(value, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
                if key is None:
                    boundary = mapping_unpack_effects(
                        value,
                        fact_result=self._known_expression_result,
                    )
                    boundary |= keys.extend(
                        static_expression_result(
                            value, fact_result=self._known_expression_result
                        )
                    )
                else:
                    boundary = keys.add(
                        static_expression_result(
                            key, fact_result=self._known_expression_result
                        )
                    )
                    pending_hash |= boundary
                effects |= boundary
                state_id = self._apply_effects(state_id, boundary)
            state_id = self._apply_effects(state_id, pending_hash)
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
        if not effects_applied:
            state_id = self._apply_effects(state_id, effects)
        if identities & int(
            PythonIdentity.CURRENT_MODULE
            | PythonIdentity.CURRENT_GLOBALS
            | PythonIdentity.CURRENT_LOCALS
            | PythonIdentity.CURRENT_FRAME
            | PythonIdentity.BUILTIN_GLOBALS
            | PythonIdentity.BUILTIN_LOCALS
            | PythonIdentity.BUILTIN_VARS
        ):
            self._namespace_observation_epoch += 1
        self._record_expression(
            node,
            scope,
            identities,
            effects,
            static_value,
            binding_invalidated,
            binding_is_bound,
            self._namespace_observation_epoch != observation_before,
        )
        self._record_state(state_id)
        return _ExpressionResult(
            state_id, identities, effects, static_value, assignment_target
        )

    def _eval_call_arguments(
        self, node: ast.Call | ast.ClassDef, state_id: int, scope: _Scope
    ) -> tuple[int, EffectMask]:
        effects = NO_EFFECTS
        keys = AccumulatedKeyEffects()
        for step in call_argument_schedule(node):
            if step.action == "evaluate":
                result = self.eval_expr(step.expression, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
            elif step.action == "kw":
                boundary = keys.add(StaticExpressionResult.scalar(step.name))
                effects |= boundary
                state_id = self._apply_effects(state_id, boundary)
            elif step.action in {"star", "kwstar"}:
                unpack = (
                    iterable_unpack_effects
                    if step.action == "star"
                    else mapping_unpack_effects
                )
                boundary = unpack(
                    step.expression, fact_result=self._known_expression_result
                )
                if step.action == "kwstar":
                    boundary |= keys.extend(
                        static_expression_result(
                            step.expression, fact_result=self._known_expression_result
                        )
                    )
                effects |= boundary
                state_id = self._apply_effects(state_id, boundary)
        return state_id, effects

    def _eval_arguments(
        self, arguments: ast.arguments, state_id: int, scope: _Scope
    ) -> tuple[int, EffectMask, tuple[tuple[str, IdentityMask], ...]]:
        effects = NO_EFFECTS
        defaults: list[tuple[str, IdentityMask]] = []
        positional = (*arguments.posonlyargs, *arguments.args)
        default_parameters = positional[len(positional) - len(arguments.defaults) :]
        expressions = [
            *zip(default_parameters, arguments.defaults, strict=True),
            *(
                (parameter, expression)
                for parameter, expression in zip(
                    arguments.kwonlyargs, arguments.kw_defaults, strict=True
                )
                if expression is not None
            ),
        ]
        for parameter, expression in expressions:
            result = self.eval_expr(expression, state_id, scope)
            state_id = result.state_id
            effects |= result.effects
            defaults.append((parameter.arg, result.identities))
        return state_id, effects, tuple(defaults)

    def _eval_comprehension(
        self, node: ast.expr, state_id: int, parent: _Scope
    ) -> tuple[int, EffectMask]:
        generators = tuple(getattr(node, "generators"))
        names = {
            name for generator in generators for name in _target_names(generator.target)
        }
        declarations = PythonScopeDeclarations(
            frozenset(names), frozenset(), frozenset()
        )
        scope = self._new_scope(
            parent=parent,
            source_node=node,
            kind="comprehension",
            name="<comprehension>",
            declarations=declarations,
        )
        first_iterable = self.eval_expr(generators[0].iter, state_id, parent)
        immediate_effects = (
            first_iterable.effects
            | INVOKES_ITERATION_CALLBACK
            | EXECUTES_ARBITRARY_PYTHON
            | RAISES
        )
        deferred_effects = NO_EFFECTS
        immediate_state = self._apply_effects(
            first_iterable.state_id, immediate_effects
        )
        immediate_observation = self._namespace_observation_epoch
        current = immediate_state
        for index, generator in enumerate(generators):
            if index:
                iterable = self.eval_expr(generator.iter, current, scope)
                current = iterable.state_id
                deferred_effects |= iterable.effects
            deferred_effects |= (
                INVOKES_ITERATION_CALLBACK | EXECUTES_ARBITRARY_PYTHON | RAISES
            )
            current = self._apply_effects(
                current, INVOKES_ITERATION_CALLBACK | EXECUTES_ARBITRARY_PYTHON | RAISES
            )
            current, target_effects = self.assign_target(
                generator.target, OTHER_IDENTITY, current, scope
            )
            deferred_effects |= target_effects
            for condition in generator.ifs:
                condition_result = self._eval_truth_test(condition, current, scope)
                current = condition_result.state_id
                deferred_effects |= condition_result.effects
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
        if isinstance(node, ast.GeneratorExp):
            # The body is analyzed for its own facts, but only acquisition of
            # the first iterator executes while creating a generator object.
            self._namespace_observation_epoch = immediate_observation
            return immediate_state, immediate_effects
        return current, immediate_effects | deferred_effects

    def _replace_binding(
        self,
        state_id: int,
        slot: int,
        value: IdentityMask,
        static_value: PythonStaticValue = None,
    ) -> tuple[int, EffectMask]:
        return self._replace_bindings(state_id, ((slot, value, static_value),))

    def _replace_bindings(
        self,
        state_id: int,
        bindings: Sequence[tuple[int, IdentityMask, PythonStaticValue]],
        *,
        may_write: bool = False,
    ) -> tuple[int, EffectMask]:
        # STORE/DELETE publishes before releasing the old value. For a finite
        # choice of namespace keys, the non-relational state domain joins each
        # slot independently: retain its old value as a may-write, and publish
        # the whole abstract update once instead of constructing N branches.
        updates: list[tuple[int, IdentityMask, PythonStaticValue]] = []
        releases_previous = False
        for slot, value, static_value in bindings:
            previous = self.states.binding(state_id, slot)
            releases_previous |= _identity_can_release(previous)
            if may_write:
                value |= previous
                if self.states.static_value(state_id, slot) != static_value:
                    static_value = None
            updates.append((slot, value, static_value))
        state_id = self.states.set_bindings(state_id, updates)
        if not releases_previous:
            return state_id, NO_EFFECTS
        effects = _RELEASE_CALLBACK_EFFECTS
        return self._apply_effects(state_id, effects), effects

    def _evaluate_member_target(
        self,
        target: ast.Attribute | ast.Subscript,
        state_id: int,
        scope: _Scope,
    ) -> tuple[_EvaluatedMemberTarget, int, EffectMask]:
        owner = self.eval_expr(target.value, state_id, scope)
        state_id = owner.state_id
        effects = owner.effects
        static_index: PythonStaticValue = None
        if isinstance(target, ast.Subscript):
            index = self.eval_expr(target.slice, state_id, scope)
            state_id = index.state_id
            effects |= index.effects
            static_index = index.static_value
        return (
            _EvaluatedMemberTarget(target, owner.identities, static_index),
            state_id,
            effects,
        )

    def _store_evaluated_member(
        self,
        target: _EvaluatedMemberTarget,
        state_id: int,
        value: IdentityMask = OTHER_IDENTITY,
        static_value: PythonStaticValue = None,
    ) -> tuple[int, EffectMask]:
        # Receiver/index identity is retained across RHS and in-place operator
        # callbacks. A store must not evaluate either source expression again.
        if isinstance(target.node, ast.Subscript) and target.owner_identities == int(
            PythonIdentity.CURRENT_GLOBALS
        ):
            return self._store_namespace_key(
                state_id, target.static_index, value, static_value
            )
        strings = (
            frozenset((target.static_index,))
            if isinstance(target.static_index, str)
            else target.static_index.values
            if isinstance(target.static_index, PythonStringAlternatives)
            else None
        )
        effects = (
            WRITES_OBJECT_STATE
            | EXECUTES_ARBITRARY_PYTHON
            | _RELEASE_CALLBACK_EFFECTS
            | RAISES
        )
        if isinstance(target.node, ast.Attribute):
            state_id = self._invalidate_member_target(
                state_id, target.owner_identities, target.node.attr
            )
            effects |= INVOKES_DESCRIPTOR
            if (
                target.owner_identities & int(PythonIdentity.CURRENT_MODULE)
                and target.node.attr in _METADATA_NAMES
            ):
                effects |= WRITES_MODULE_METADATA | WRITES_GLOBAL_NAMESPACE
        elif target.owner_identities & int(PythonIdentity.CURRENT_GLOBALS):
            effects |= WRITES_GLOBAL_NAMESPACE
            # An unknown key can select any import-metadata member.
            if strings is None or strings & _METADATA_NAMES:
                effects |= WRITES_MODULE_METADATA
        return self._apply_effects(state_id, effects), effects

    def _store_namespace_key(
        self,
        state_id: int,
        key: PythonStaticValue,
        value: IdentityMask,
        static_value: PythonStaticValue,
    ) -> tuple[int, EffectMask]:
        """Shared item-store/delete publication for syntax and bound dict methods."""
        strings = (
            frozenset((key,))
            if isinstance(key, str)
            else key.values
            if isinstance(key, PythonStringAlternatives)
            else None
        )
        if not strings:
            return self._apply_effects(state_id, UNKNOWN_EFFECTS), UNKNOWN_EFFECTS
        effects = WRITES_GLOBAL_NAMESPACE | WRITES_OBJECT_STATE | RAISES
        if strings & _METADATA_NAMES:
            effects |= WRITES_MODULE_METADATA
        state_id, release_effects = self._replace_bindings(
            state_id,
            tuple(
                (self._ensure_module_slot(name), value, static_value)
                for name in sorted(strings)
            ),
            may_write=len(strings) > 1,
        )
        return state_id, effects | release_effects

    def assign_target(
        self,
        target: ast.AST,
        value: IdentityMask,
        state_id: int,
        scope: _Scope,
        *,
        static_value: PythonStaticValue = None,
    ) -> tuple[int, EffectMask]:
        result = self._assign_target(
            target, value, state_id, scope, static_value=static_value
        )
        key = self._node_key(target)
        self.assignment_effects[key] = (
            self.assignment_effects.get(key, NO_EFFECTS) | result[1]
        )
        return result

    def _assign_target(
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
            return self._write_name(state_id, scope, target.id, value, static_value)
        if isinstance(target, (ast.Tuple, ast.List)):
            if (
                isinstance(static_value, tuple)
                and len(static_value) == len(target.elts)
                and not any(isinstance(element, ast.Starred) for element in target.elts)
            ):
                for element, item in zip(target.elts, static_value, strict=True):
                    state_id, element_effects = self.assign_target(
                        element,
                        int(PythonIdentity.INERT_VALUE),
                        state_id,
                        scope,
                        static_value=item,
                    )
                    effects |= element_effects
                return state_id, effects
            effects |= INVOKES_ITERATION_CALLBACK | EXECUTES_ARBITRARY_PYTHON | RAISES
            state_id = self._apply_effects(state_id, effects)
            for element in target.elts:
                state_id, element_effects = self.assign_target(
                    element, OTHER_IDENTITY, state_id, scope
                )
                effects |= element_effects
            return state_id, effects
        if isinstance(target, ast.Starred):
            return self.assign_target(target.value, OTHER_IDENTITY, state_id, scope)
        if isinstance(target, (ast.Attribute, ast.Subscript)):
            evaluated, state_id, evaluation_effects = self._evaluate_member_target(
                target, state_id, scope
            )
            state_id, store_effects = self._store_evaluated_member(
                evaluated, state_id, value, static_value
            )
            return state_id, evaluation_effects | store_effects
        return self._apply_effects(state_id, UNKNOWN_EFFECTS), UNKNOWN_EFFECTS

    def delete_target(
        self, target: ast.AST, state_id: int, scope: _Scope
    ) -> tuple[int, EffectMask]:
        result = self._delete_target(target, state_id, scope)
        key = self._node_key(target)
        self.assignment_effects[key] = (
            self.assignment_effects.get(key, NO_EFFECTS) | result[1]
        )
        return result

    def _delete_target(
        self, target: ast.AST, state_id: int, scope: _Scope
    ) -> tuple[int, EffectMask]:
        if isinstance(target, ast.Name):
            identities, _value, invalidated, bound = self._resolve_name(
                state_id, scope, target.id
            )
            updated, effects = self._write_name(
                state_id, scope, target.id, UNBOUND_IDENTITY
            )
            if not bound or identities & UNBOUND_IDENTITY or invalidated:
                effects |= RAISES
            return updated, effects
        if isinstance(target, (ast.Tuple, ast.List)):
            effects = NO_EFFECTS
            for element in target.elts:
                state_id, child_effects = self.delete_target(element, state_id, scope)
                effects |= child_effects
            return state_id, effects
        return self.assign_target(target, UNBOUND_IDENTITY, state_id, scope)

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
            ("_intrinsics", "require_intrinsic"): PythonIdentity.INTRINSICS_REQUIRE,
        }.get((module, name))
        if identity is None:
            return OTHER_IDENTITY
        return (
            exact_identity(identity)
            if self._canonical_imports_available(state_id)
            else possible_identity(identity)
        )

    def _bind_name(
        self, name: str, value: IdentityMask, state_id: int, scope: _Scope
    ) -> tuple[int, EffectMask]:
        return self._write_name(state_id, scope, name, value)

    def _write_name(
        self,
        state_id: int,
        scope: _Scope,
        name: str,
        value: IdentityMask,
        static_value: PythonStaticValue = None,
    ) -> tuple[int, EffectMask]:
        slot = self._slot_for_name(scope, name)
        if scope.namespace_can_call(
            name,
            write=True,
            namespace_tainted=self.states.get(state_id).taint_epoch != 0,
        ):
            if slot is not None:
                state_id = self.states.set_binding(
                    state_id, slot, OTHER_IDENTITY | UNBOUND_IDENTITY
                )
            effects = WRITES_OBJECT_STATE | EXECUTES_ARBITRARY_PYTHON | RAISES
            return self._apply_effects(state_id, effects), effects
        if slot is None:
            return state_id, NO_EFFECTS
        return self._replace_binding(state_id, slot, value, static_value)

    def _merge_flows(
        self, *flows: PythonCompletionFlow[int]
    ) -> PythonCompletionFlow[int]:
        result: PythonCompletionFlow[int] = PythonCompletionFlow()
        for flow in flows:
            result = result.merge(flow, join_states=self.states.join)
        return result

    def _normal_flow(
        self, state_id: int, effects: EffectMask = NO_EFFECTS
    ) -> PythonCompletionFlow[int]:
        self._record_state(state_id)
        # A leaf may fail before its final assignment. Preserve the entry and
        # intermediate expression states, not only its successful exit state.
        if effects & ALLOCATES:
            effects |= RAISES
        exceptional = (
            self.states.join(*self._observed_stack[-1], state_id)
            if effects & RAISES
            else None
        )
        return PythonCompletionFlow(
            normal=state_id, raised=exceptional, effects=effects
        )

    def exec_statements(
        self, body: Sequence[ast.stmt], state_id: int, scope: _Scope
    ) -> PythonCompletionFlow[int]:
        flow = PythonCompletionFlow(normal=state_id)
        for statement in body:
            if flow.normal is None:
                break
            flow = flow.sequence(
                lambda normal: self.exec_statement(statement, normal, scope),
                join_states=self.states.join,
            )
        return flow

    def exec_statement(
        self, node: ast.stmt, state_id: int, scope: _Scope
    ) -> PythonCompletionFlow[int]:
        observation_before = self._namespace_observation_epoch
        observed = [state_id]
        self._observed_stack.append(observed)
        try:
            flow = self._exec_statement(node, state_id, scope)
        finally:
            self._observed_stack.pop()
        if not flow.completions & PythonCompletion.NORMAL:
            self._module_import_flow_required = True
        key = self._node_key(node)
        previous = self.statements.get(key)
        self.statements[key] = PythonStatementFact(
            key,
            scope.scope_id,
            flow.effects | (previous.effects if previous is not None else NO_EFFECTS),
            self._namespace_observation_epoch != observation_before
            or (previous is not None and previous.module_namespace_observable),
            flow.completions
            | (previous.completions if previous is not None else PythonCompletion.NONE),
            self.iterations.get(key),
        )
        return flow

    def _exec_statement(
        self, node: ast.stmt, state_id: int, scope: _Scope
    ) -> PythonCompletionFlow[int]:
        effects = NO_EFFECTS
        if isinstance(
            node, (ast.For, ast.AsyncFor, ast.With, ast.AsyncWith, ast.Match)
        ):
            self._module_import_flow_required = True
        elif isinstance(node, ast.Assign) and any(
            self._target_may_write_import_metadata(target) for target in node.targets
        ):
            self._module_import_flow_required = True
        elif isinstance(
            node, (ast.AnnAssign, ast.AugAssign)
        ) and self._target_may_write_import_metadata(node.target):
            self._module_import_flow_required = True
        elif isinstance(node, ast.Delete) and any(
            self._target_may_write_import_metadata(target) for target in node.targets
        ):
            self._module_import_flow_required = True
        if isinstance(node, ast.Expr):
            result = self.eval_expr(node.value, state_id, scope)
            state_id, effects = result.state_id, result.effects
        elif isinstance(node, ast.Assign):
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
            elif isinstance(node.target, (ast.Attribute, ast.Subscript)):
                # An annotation without a value evaluates the target owner
                # (and index), but does not assign or release its member.
                target_parts = (
                    (node.target.value, node.target.slice)
                    if isinstance(node.target, ast.Subscript)
                    else (node.target.value,)
                )
                for expression in target_parts:
                    result = self.eval_expr(expression, state_id, scope)
                    state_id = result.state_id
                    effects |= result.effects
            if not self._future_annotations and scope.kind in {"module", "class"}:
                if self.policy.target_python < (3, 14):
                    # Eager annotations execute after the value is assigned.
                    annotation = self.eval_expr(node.annotation, state_id, scope)
                    state_id = annotation.state_id
                    effects |= annotation.effects
                elif node.simple:
                    self._queue_annotation_expression(
                        node.annotation, scope, state_id, ()
                    )
        elif isinstance(node, ast.AugAssign):
            target_read = (
                self.eval_expr(node.target, state_id, scope)
                if isinstance(node.target, ast.expr)
                else _ExpressionResult(state_id, OTHER_IDENTITY, NO_EFFECTS)
            )
            value = self.eval_expr(node.value, target_read.state_id, scope)
            effects |= (
                target_read.effects | value.effects | EXECUTES_ARBITRARY_PYTHON | RAISES
            )
            state_id = self._apply_effects(
                value.state_id, EXECUTES_ARBITRARY_PYTHON | RAISES
            )
            if target_read.assignment_target is not None:
                state_id, target_effects = self._store_evaluated_member(
                    target_read.assignment_target, state_id
                )
            else:
                state_id, target_effects = self.assign_target(
                    node.target, OTHER_IDENTITY, state_id, scope
                )
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
                module in _CANONICAL_IMPORT_IDENTITIES for module in identity_modules
            )
            if not canonical_statement:
                state_id = self._apply_effects(state_id, effects)
                state_id = self.states.invalidate_members(
                    state_id, _IMPORT_EXECUTION_INVALID_MEMBERS
                )
            for alias in node.names:
                bound = alias.asname or alias.name.split(".", 1)[0]
                identity_module = (
                    alias.name if alias.asname else alias.name.split(".", 1)[0]
                )
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
                        or f"{node.module}.{alias.name}" in _CANONICAL_IMPORT_IDENTITIES
                    )
                    for alias in node.names
                )
            )
            if not canonical_statement:
                state_id = self._apply_effects(state_id, effects)
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
            test = self._eval_truth_test(node.test, state_id, scope)
            truth = self._known_expression_result(node.test).truth
            if truth is None and test.identities == int(PythonIdentity.STATIC_FALSE):
                truth = False
            prefix = self._normal_flow(test.state_id, test.effects)
            branches = (
                (node.body if truth else node.orelse,)
                if truth is not None
                else (node.body, node.orelse)
            )
            return prefix.sequence(
                lambda normal: self._merge_flows(
                    *(
                        self.exec_statements(branch, normal, scope)
                        for branch in branches
                    )
                ),
                join_states=self.states.join,
            )
        elif isinstance(node, (ast.For, ast.AsyncFor, ast.While)):
            return self._exec_loop(node, state_id, scope)
        elif isinstance(node, (ast.With, ast.AsyncWith)):
            return self._exec_with(node, state_id, scope)
        elif isinstance(node, (ast.Try, getattr(ast, "TryStar", ast.Try))):
            return self._exec_try(node, state_id, scope)
        elif isinstance(node, ast.Match):
            subject = self.eval_expr(node.subject, state_id, scope)
            flow = self._normal_flow(subject.state_id, subject.effects).without_normal()
            unmatched: int | None = subject.state_id
            for case in node.cases:
                if unmatched is None:
                    break
                # A capture/wildcard pattern is irrefutable and has no protocol
                # callbacks. Other patterns retain the existing conservative
                # comparison boundary.
                irrefutable = (
                    python_pattern_irrefutable_reason(case.pattern) is not None
                )
                inert_capture = python_pattern_is_capture_only(case.pattern)
                pattern_effects = (
                    NO_EFFECTS
                    if inert_capture
                    else INVOKES_COMPARISON_CALLBACK | RAISES
                )
                branch = self._apply_effects(unmatched, pattern_effects)
                if not irrefutable:
                    unmatched = branch
                pattern_flow = self._normal_flow(branch, pattern_effects)
                flow = self._merge_flows(flow, pattern_flow.without_normal())
                for name in _target_names(case.pattern):
                    branch, target_effects = self._bind_name(
                        name, OTHER_IDENTITY, branch, scope
                    )
                    flow = self._merge_flows(
                        flow, self._normal_flow(branch, target_effects).without_normal()
                    )
                guard_truth = True
                if case.guard is not None:
                    guard = self._eval_truth_test(case.guard, branch, scope)
                    branch = guard.state_id
                    guard_truth = self._known_expression_result(case.guard).truth
                    flow = self._merge_flows(
                        flow, self._normal_flow(branch, guard.effects).without_normal()
                    )
                if guard_truth is not False:
                    flow = self._merge_flows(
                        flow, self.exec_statements(case.body, branch, scope)
                    )
                if irrefutable and guard_truth is True:
                    unmatched = None
                elif guard_truth is not True:
                    unmatched = self.states.join(unmatched, branch)
            if unmatched is not None:
                flow = self._merge_flows(flow, PythonCompletionFlow(normal=unmatched))
            return flow
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            for decorator in node.decorator_list:
                result = self.eval_expr(decorator, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
            # Generic defaults are evaluated before entering the type-parameter
            # annotation scope, just like ordinary function defaults.
            state_id, default_effects, defaults = self._eval_arguments(
                node.args, state_id, scope
            )
            effects |= default_effects | ALLOCATES
            state_id, definition_scope = self._type_parameter_scope(
                node, state_id, scope
            )
            for annotation in function_annotation_expressions(node):
                if self._future_annotations:
                    continue
                if self.policy.target_python >= (3, 14):
                    self._queue_annotation_expression(
                        annotation, definition_scope, state_id, ()
                    )
                else:
                    result = self.eval_expr(annotation, state_id, definition_scope)
                    state_id = result.state_id
                    effects |= result.effects
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
            self._queue_function(node, definition_scope, state_id, defaults)
        elif isinstance(node, ast.ClassDef):
            for expression in node.decorator_list:
                result = self.eval_expr(expression, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
            state_id, definition_scope = self._type_parameter_scope(
                node, state_id, scope
            )
            state_id, argument_effects = self._eval_call_arguments(
                node, state_id, definition_scope
            )
            effects |= argument_effects
            assert self._dependency_authority is not None
            declarations = self._dependency_authority.declarations(node)
            class_scope = self._new_scope(
                parent=definition_scope,
                source_node=node,
                kind="class",
                name=node.name,
                declarations=declarations,
            )
            plain_preparation = (
                not node.bases
                and not node.keywords
                and self._canonical_imports_available(state_id)
                and self.states.get(state_id).taint_epoch == 0
            )
            class_scope.dynamic_class_namespace |= not plain_preparation
            # Each execution creates a fresh namespace, despite sharing source
            # slots/facts with other visits to this class definition.
            state_id = self.states.set_bindings(
                state_id,
                tuple(
                    (slot, UNBOUND_IDENTITY, None)
                    for slot in class_scope.slots.values()
                ),
            )
            if not plain_preparation:
                # __mro_entries__, metaclass selection and __prepare__ all run
                # before body lookup; custom namespace initialization can call
                # Python before the first user statement as well.
                preparation_effects = EXECUTES_ARBITRARY_PYTHON | RAISES
                effects |= preparation_effects
                state_id = self._apply_effects(state_id, preparation_effects)
            prefix = self._normal_flow(state_id, effects)
            class_flow = self.exec_statements(node.body, state_id, class_scope)
            flow = self._merge_flows(
                prefix.without_normal(), class_flow.without_normal()
            )
            if class_flow.normal is not None:
                class_state = class_flow.normal
                # Body failure never creates or binds a class. Plain creation
                # remains a projection of the successful namespace state.
                plain_creation = (
                    plain_preparation
                    and not node.decorator_list
                    and self._canonical_imports_available(class_state)
                    and self.states.get(class_state).taint_epoch == 0
                    and all(
                        self.states.binding(class_state, slot)
                        in {
                            UNBOUND_IDENTITY,
                            int(PythonIdentity.INERT_VALUE),
                            int(PythonIdentity.STATIC_FALSE),
                            int(PythonIdentity.USER_FUNCTION),
                        }
                        for slot in class_scope.slots.values()
                    )
                )
                creation_effects = ALLOCATES | RAISES
                if not plain_creation:
                    creation_effects |= EXECUTES_ARBITRARY_PYTHON
                class_state = self._apply_effects(class_state, creation_effects)
                class_state, bind_effects = self._bind_name(
                    node.name,
                    possible_identity(PythonIdentity.USER_CLASS),
                    class_state,
                    scope,
                )
                flow = self._merge_flows(
                    flow,
                    self._normal_flow(class_state, creation_effects | bind_effects),
                )
            return flow
        elif isinstance(node, getattr(ast, "TypeAlias", ())):
            state_id, definition_scope = self._type_parameter_scope(
                node, state_id, scope
            )
            self._queue_annotation_expression(
                node.value, definition_scope, state_id, ()
            )
            if isinstance(node.name, ast.Name):
                state_id, bind_effects = self._bind_name(
                    node.name.id,
                    exact_identity(PythonIdentity.INERT_VALUE),
                    state_id,
                    scope,
                )
                effects |= bind_effects
            effects |= ALLOCATES
        elif isinstance(node, ast.Return):
            if node.value is not None:
                result = self.eval_expr(node.value, state_id, scope)
                state_id, effects = result.state_id, result.effects
            evaluated = self._normal_flow(state_id, effects)
            return self._merge_flows(
                evaluated.without_normal(), PythonCompletionFlow(returned=state_id)
            )
        elif isinstance(node, ast.Raise):
            for expression in (node.exc, node.cause):
                if expression is not None:
                    result = self.eval_expr(expression, state_id, scope)
                    state_id = result.state_id
                    effects |= result.effects
            # Exception class normalization/instantiation can invoke Python;
            # a bare re-raise has no new exception/cause expression to normalize.
            effects |= RAISES
            if node.exc is not None:
                effects |= EXECUTES_ARBITRARY_PYTHON
                state_id = self._apply_effects(
                    state_id, EXECUTES_ARBITRARY_PYTHON | RAISES
                )
            self._record_state(state_id)
            return PythonCompletionFlow(
                raised=self.states.join(*self._observed_stack[-1], state_id),
                effects=effects,
            )
        elif isinstance(node, ast.Assert):
            test = self._eval_truth_test(node.test, state_id, scope)
            truth = self._known_expression_result(node.test).truth
            flow = self._normal_flow(test.state_id, test.effects).without_normal()
            if truth is not False:
                flow = self._merge_flows(
                    flow, PythonCompletionFlow(normal=test.state_id)
                )
            if truth is not True:
                failure_state = test.state_id
                failure_effects = RAISES
                if node.msg is not None:
                    message = self.eval_expr(node.msg, failure_state, scope)
                    failure_state = message.state_id
                    failure_effects |= message.effects
                flow = self._merge_flows(
                    flow,
                    self._normal_flow(failure_state, failure_effects).without_normal(),
                )
            return flow
        elif isinstance(node, ast.Break):
            return PythonCompletionFlow(broken=state_id)
        elif isinstance(node, ast.Continue):
            return PythonCompletionFlow(continued=state_id)
        elif isinstance(node, (ast.Global, ast.Nonlocal, ast.Pass)):
            pass
        else:
            for child in ast.iter_child_nodes(node):
                if isinstance(child, ast.expr):
                    result = self.eval_expr(child, state_id, scope)
                    state_id = result.state_id
                    effects |= result.effects
            effects |= UNKNOWN_EFFECTS
            state_id = self._apply_effects(state_id, effects)
        return self._normal_flow(state_id, effects)

    def _exec_loop(
        self, node: ast.For | ast.AsyncFor | ast.While, state_id: int, scope: _Scope
    ) -> PythonCompletionFlow[int]:
        entry = state_id
        prefix: PythonCompletionFlow[int] = PythonCompletionFlow()
        iteration: PythonIterationFact | None = None
        if isinstance(node, (ast.For, ast.AsyncFor)):
            iterable = self.eval_expr(node.iter, entry, scope)
            entry = iterable.state_id
            iteration_effects = iterable_unpack_effects(
                node.iter, fact_result=self._known_expression_result
            )
            if isinstance(node, ast.AsyncFor):
                iteration_effects = (
                    INVOKES_ITERATION_CALLBACK
                    | EXECUTES_ARBITRARY_PYTHON
                    | SUSPENDS
                    | RAISES
                )
            iterable_result = self._known_expression_result(node.iter)
            iteration = PythonIterationFact.from_result(
                iterable_result,
                iteration_effects,
                _RELEASE_CALLBACK_EFFECTS
                if iterable_result.release_may_call
                else NO_EFFECTS,
            )
            key = self._node_key(node)
            previous = self.iterations.get(key)
            self.iterations[key] = (
                iteration if previous is None else previous.merge(iteration)
            )
            entry = self._apply_effects(entry, iteration.effects)
            prefix = self._normal_flow(
                entry, iterable.effects | iteration.effects
            ).without_normal()
        entry_epoch = self.states.get(entry).taint_epoch

        def advance(header: int) -> tuple[int | None, PythonCompletionFlow[int]]:
            body_entry = header
            truth: bool | None = None
            if isinstance(node, ast.While):
                test = self._eval_truth_test(node.test, header, scope)
                body_entry = test.state_id
                truth = self._known_expression_result(node.test).truth
                header_flow = self._normal_flow(body_entry, test.effects)
            else:
                assert iteration is not None
                body_entry = self._apply_effects(body_entry, iteration.effects)
                header_flow = self._normal_flow(body_entry, iteration.effects)
                if iteration.empty:
                    truth = False
            exhausted = body_entry if truth is not True else None
            if truth is False:
                return exhausted, header_flow.without_normal()
            if isinstance(node, (ast.For, ast.AsyncFor)):
                assert iteration is not None
                strings = iteration.element_strings
                if (
                    iteration.mutable
                    and self.states.get(body_entry).taint_epoch != entry_epoch
                ):
                    strings = None
                body_entry, target_effects = self.assign_target(
                    node.target,
                    int(PythonIdentity.INERT_VALUE)
                    if strings is not None
                    else OTHER_IDENTITY,
                    body_entry,
                    scope,
                    static_value=strings,
                )
                header_flow = self._merge_flows(
                    header_flow.without_normal(),
                    self._normal_flow(body_entry, target_effects),
                )
            return exhausted, header_flow.sequence(
                lambda normal: self.exec_statements(node.body, normal, scope),
                join_states=self.states.join,
            )

        def widen(header: int) -> int:
            return self.states.invalidate_members(
                self._widen_module_bindings(header), ALL_INVALID_MEMBERS
            )

        def finalize(state: int) -> PythonCompletionFlow[int]:
            assert iteration is not None
            return self._normal_flow(
                self._apply_effects(state, iteration.release_effects),
                iteration.release_effects,
            )

        return self._merge_flows(
            prefix,
            PythonCompletionFlow.loop(
                entry,
                advance,
                lambda exhausted: self.exec_statements(node.orelse, exhausted, scope),
                join_states=self.states.join,
                equivalent_states=self.states.equivalent,
                widen_state=widen,
                finalize=finalize
                if iteration is not None and iteration.release_effects
                else None,
            ),
        )

    def _exec_with(
        self, node: ast.With | ast.AsyncWith, state_id: int, scope: _Scope
    ) -> PythonCompletionFlow[int]:
        callback_effects = INVOKES_CONTEXT_CALLBACK | EXECUTES_ARBITRARY_PYTHON | RAISES
        if isinstance(node, ast.AsyncWith):
            callback_effects |= SUSPENDS

        def enter(index: int, incoming: int) -> PythonCompletionFlow[int]:
            if index == len(node.items):
                return self.exec_statements(node.body, incoming, scope)
            item = node.items[index]
            context = self.eval_expr(item.context_expr, incoming, scope)
            evaluated = self._normal_flow(context.state_id, context.effects)
            entered = self._apply_effects(context.state_id, callback_effects)
            prefix = self._merge_flows(
                evaluated.without_normal(),
                self._normal_flow(entered, callback_effects).without_normal(),
            )
            assigned = PythonCompletionFlow(normal=entered)
            if item.optional_vars is not None:
                assigned_state, assigned_effects = self.assign_target(
                    item.optional_vars, OTHER_IDENTITY, entered, scope
                )
                assigned = self._normal_flow(assigned_state, assigned_effects)
            body = assigned.sequence(
                lambda normal: enter(index + 1, normal), join_states=self.states.join
            )

            def exit_context(incoming: int) -> PythonCompletionFlow[int]:
                released = self._apply_effects(incoming, callback_effects)
                return self._normal_flow(released, callback_effects)

            return self._merge_flows(
                prefix, body.unwind_context(exit_context, join_states=self.states.join)
            )

        return enter(0, state_id)

    def _exec_exception_handler(
        self, handler: ast.ExceptHandler, state_id: int, scope: _Scope
    ) -> PythonCompletionFlow[int]:
        effects = NO_EFFECTS
        exception_name = handler.name
        if exception_name:
            state_id, effects = self._bind_name(
                exception_name, OTHER_IDENTITY, state_id, scope
            )
        flow = self._normal_flow(state_id, effects).sequence(
            lambda normal: self.exec_statements(handler.body, normal, scope),
            join_states=self.states.join,
        )
        if exception_name:

            def clear_exception(incoming: int) -> PythonCompletionFlow[int]:
                # CPython clears an exception target by storing None before
                # deleting it, including when the handler already deleted it.
                rebound, release_effects = self._bind_name(
                    exception_name,
                    exact_identity(PythonIdentity.INERT_VALUE),
                    incoming,
                    scope,
                )
                cleared, cleanup_effects = self.delete_target(
                    ast.Name(id=exception_name, ctx=ast.Del()), rebound, scope
                )
                return self._normal_flow(cleared, release_effects | cleanup_effects)

            flow = flow.apply_finally(clear_exception, join_states=self.states.join)
        return flow

    def _exec_try_star_handlers(
        self, handlers: Sequence[ast.ExceptHandler], state_id: int, scope: _Scope
    ) -> PythonCompletionFlow[int]:
        def evaluate_type(
            handler: ast.ExceptHandler, incoming: int
        ) -> PythonCompletionFlow[int]:
            if handler.type is None:
                return PythonCompletionFlow(normal=incoming)
            kind = self.eval_expr(handler.type, incoming, scope)
            return self._normal_flow(kind.state_id, kind.effects | RAISES)

        def group_protocol(incoming: int) -> PythonCompletionFlow[int]:
            effects = EXECUTES_ARBITRARY_PYTHON | ALLOCATES | RAISES
            successor = self._apply_effects(incoming, effects)
            return self._normal_flow(successor, effects)

        return PythonCompletionFlow.exception_group_handlers(
            state_id,
            handlers,
            evaluate_type=evaluate_type,
            split_group=group_protocol,
            execute_handler=lambda handler, incoming: self._exec_exception_handler(
                handler, incoming, scope
            ),
            merge_group=group_protocol,
            join_states=self.states.join,
        )

    def _exec_try(
        self, node: ast.Try | ast.TryStar, state_id: int, scope: _Scope
    ) -> PythonCompletionFlow[int]:
        body = self.exec_statements(node.body, state_id, scope)
        flow = PythonCompletionFlow(
            returned=body.returned,
            broken=body.broken,
            continued=body.continued,
            effects=body.effects,
        )
        if body.normal is not None:
            flow = self._merge_flows(
                flow, self.exec_statements(node.orelse, body.normal, scope)
            )
        if body.raised is not None:
            if isinstance(node, ast.TryStar):
                handled = self._exec_try_star_handlers(
                    node.handlers, body.raised, scope
                )
                flow = self._merge_flows(flow, handled)
            else:
                unmatched: int | None = body.raised
                for handler in node.handlers:
                    if unmatched is None:
                        break
                    branch = unmatched
                    if handler.type is not None:
                        kind = self.eval_expr(handler.type, branch, scope)
                        branch = kind.state_id
                        unmatched = branch
                        flow = self._merge_flows(
                            flow,
                            self._normal_flow(
                                branch, kind.effects | RAISES
                            ).without_normal(),
                        )
                    else:
                        unmatched = None
                    flow = self._merge_flows(
                        flow, self._exec_exception_handler(handler, branch, scope)
                    )
                if unmatched is not None:
                    flow = self._merge_flows(
                        flow, PythonCompletionFlow(raised=unmatched)
                    )
        if node.finalbody:
            flow = flow.apply_finally(
                lambda incoming: self.exec_statements(node.finalbody, incoming, scope),
                join_states=self.states.join,
            )
        return flow

    def _analyze_function_job(self, job: _FunctionJob, outer_state: int) -> None:
        node = job.node
        arguments = node.args
        parameters = function_parameter_names(arguments)
        body = (
            node.body
            if isinstance(node.body, list)
            else [ast.copy_location(ast.Return(value=node.body), node.body)]
        )
        assert self._dependency_authority is not None
        declarations = self._dependency_authority.declarations(node)
        kind: _ScopeKind = (
            "annotation"
            if job.annotation_scope
            else "lambda"
            if isinstance(node, ast.Lambda)
            else "function"
        )
        name = "<lambda>" if isinstance(node, ast.Lambda) else node.name
        scope = self._new_scope(
            parent=job.parent_scope,
            source_node=node,
            kind=kind,
            name=name,
            declarations=declarations,
        )
        scope.annotation_evaluator = job.annotation_scope
        state_id = outer_state
        parameter_default_identities = dict(job.parameter_default_identities)
        for local_name in scope.locals:
            state_id = self.states.set_binding(
                state_id, scope.slots[local_name], UNBOUND_IDENTITY
            )
        for parameter in parameters:
            slot = scope.slots.get(parameter)
            if slot is not None:
                identities = OTHER_IDENTITY | parameter_default_identities.get(
                    parameter, NO_IDENTITIES
                )
                state_id = self.states.set_binding(
                    state_id,
                    slot,
                    identities,
                    PythonParameterRef(parameter),
                )
        observed: list[int] = [state_id]
        self._observed_stack.append(observed)
        previous_history = self._active_lexical_history
        self._active_lexical_history = observed
        try:
            self.exec_statements(body, state_id, scope)
        finally:
            self._active_lexical_history = previous_history
            self._observed_stack.pop()

    def _overlay_future_states(
        self,
        state_id: int,
        summary_states: Sequence[int],
        summary_start: int,
        base_bindings: Sequence[tuple[int, IdentityMask]],
    ) -> int:
        if summary_start >= len(summary_states):
            return state_id
        history_key = id(summary_states)
        summary = self._history_summaries.get(history_key)
        if summary is None or summary.states is not summary_states:
            summary = _HistorySummary.build(self.states, summary_states)
            self._history_summaries[history_key] = summary
        maybe, definitely, taint_epoch = summary.properties(summary_start)
        state_id = self.states.overlay_summary_properties(
            state_id,
            maybe_invalidated_members=maybe,
            definitely_invalidated_members=definitely,
            taint_epoch=taint_epoch,
        )
        updates: list[tuple[int, IdentityMask, PythonStaticValue]] = []
        for slot, base_binding in base_bindings:
            future_binding = summary.binding(self.states, summary_start, slot)
            updates.append((slot, base_binding | future_binding, None))
        return self.states.set_bindings(state_id, updates)

    def analyze(self, tree: ast.Module) -> PythonBindingIndex:
        self._future_annotations = any(
            isinstance(statement, ast.ImportFrom)
            and statement.module == "__future__"
            and any(alias.name == "annotations" for alias in statement.names)
            for statement in tree.body
        )
        self._dependency_authority = PythonDependencyAuthority(
            eager_annotations=self._eager_annotations,
            future_annotations=self._future_annotations,
        )
        declarations = python_scope_declarations(
            tree.body, eager_annotations=self._eager_annotations
        )
        module = self._new_scope(
            parent=None,
            source_node=tree,
            kind="module",
            name="<module>",
            declarations=declarations,
        )
        self.module_scope = module
        self.module_slots = list(module.slots.values())
        self.module_slot_mask = sum(1 << slot for slot in self.module_slots)
        self.states.set_taint_domain(self.module_slot_mask)
        observed: list[int] = [0]
        self._observed_stack.append(observed)
        self._active_lexical_history = observed
        try:
            module_flow = self.exec_statements(tree.body, 0, module)
            module_exit = self.states.join(
                *(state for _kind, state in module_flow.successors()), *observed
            )
        finally:
            self._observed_stack.pop()
        self._active_lexical_history = None
        cursor = 0 if self.policy.analyze_deferred_bodies else len(self.function_jobs)
        while cursor < len(self.function_jobs):
            job = self.function_jobs[cursor]
            cursor += 1
            # Deferred bodies can run after any observed module state.  Parent
            # function state is retained for closure cells; module globals are
            # widened by the shared summary through the join.
            module_states = job.module_states
            if module_states is None:
                assert job.module_history_start is not None
                module_states = self._module_history
                module_state_start = min(
                    job.module_history_start, len(self._module_history)
                )
            else:
                if not module_states:
                    module_states = (module_exit,)
                module_state_start = 0
            job_outer = self._overlay_future_states(
                job.outer_state_id,
                module_states,
                module_state_start,
                job.module_base_bindings,
            )
            if job.lexical_history is not None:
                job_outer = self._overlay_future_states(
                    job_outer,
                    job.lexical_history,
                    job.lexical_history_start,
                    job.lexical_base_bindings,
                )
            previous_module_states = self._active_module_states
            self._active_module_states = (job_outer,)
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
                        for name in (
                            set(scope.slots) | set(scope.globals) | set(scope.nonlocals)
                        )
                        if (slot := self._slot_for_name(scope, name)) is not None
                    )
                ),
            )
            for scope in self.scopes
        )
        from molt.compiler_analysis.python_imports import (
            ModuleImportContext,
            ModuleImportFlow,
            _analyze_module_import_flow_uncached,
            context_import_state,
        )

        import_context = ModuleImportContext(
            module_name=self.policy.module_name,
            is_package=self.policy.module_is_package,
            spec_name=self.policy.module_spec_name,
            target_python=self.policy.target_python,
            execution_kind=self.policy.module_execution_kind,
        )
        if self._module_import_flow_required:
            module_import_flow = _analyze_module_import_flow_uncached(
                tree,
                import_context,
                statement_facts=self.statements,
                expression_facts=self.expressions,
                assignment_effects=self.assignment_effects,
                call_facts=self.calls,
                metadata_preserving_globals_calls=frozenset(
                    (
                        fact.node.lineno,
                        fact.node.col_offset,
                        fact.node.end_lineno,
                        fact.node.end_col_offset,
                        fact.node.kind,
                    )
                    for fact in self.calls.values()
                    if fact.callee_may_be(PythonIdentity.INTRINSICS_REQUIRE)
                ),
            )
        else:
            import_state = context_import_state(import_context)
            module_import_flow = ModuleImportFlow({}, (import_state,), (import_state,))
        return PythonBindingIndex.create(
            source_digest=self.source_digest,
            target_python=self.policy.target_python,
            target_sys_platform=self.policy.target_sys_platform,
            module_name=self.policy.module_name,
            module_spec_name=self.policy.module_spec_name,
            module_is_package=self.policy.module_is_package,
            module_execution_kind=self.policy.module_execution_kind,
            module_import_flow=module_import_flow,
            expressions=tuple(
                sorted(self.expressions.values(), key=lambda fact: fact.node)
            ),
            statements=tuple(
                sorted(self.statements.values(), key=lambda fact: fact.node)
            ),
            calls=tuple(sorted(self.calls.values(), key=lambda fact: fact.node)),
            scopes=scope_facts,
            class_annotation_namespaces=frozenset(
                source_key
                for (source_key, _parent_id, kind), scope in self._source_scopes.items()
                if kind == "class" and scope.needs_annotation_namespace
            ),
            state_count=len(self.states),
            telemetry=PythonBindingTelemetry(
                binding_lookups=self.states.binding_lookups,
                join_calls=self.states.join_calls,
                join_node_visits=self.states.join_node_visits,
                join_shared_subtrees_skipped=(self.states.join_shared_subtrees_skipped),
                join_chunk_merges=self.states.join_chunk_merges,
                structural_diff_cache_entries=0,
                structural_diff_node_visits=(self.states.structural_diff_node_visits),
                structural_diff_shared_subtrees_skipped=(
                    self.states.structural_diff_shared_skips
                ),
            ),
            slot_names=tuple(self.slot_names),
        )


@dataclass(slots=True)
class _PendingAnalysis:
    ready: Event = field(default_factory=Event)
    result: PythonBindingIndex | None = None
    error: _AnalysisFailure | None = None


@dataclass(frozen=True, slots=True)
class _AnalysisFailure:
    exception_type: type[BaseException]
    args: tuple[object, ...]
    attributes: tuple[tuple[str, object], ...]

    @classmethod
    def capture(cls, error: BaseException) -> _AnalysisFailure:
        return cls(type(error), error.args, tuple(vars(error).items()))

    def instantiate(self) -> BaseException:
        try:
            error = self.exception_type(*self.args)
        except BaseException:
            error = RuntimeError(
                f"{self.exception_type.__module__}."
                f"{self.exception_type.__qualname__}: " + ", ".join(map(str, self.args))
            )
        for name, value in self.attributes:
            setattr(error, name, value)
        return error


class _BindingIndexCache:
    """Free-thread-safe single-flight cache with bounded FIFO completion eviction."""

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
                raise pending.error.instantiate()
            assert pending.result is not None
            return pending.result
        try:
            result = compute()
        except BaseException as exc:
            with self._lock:
                pending.error = _AnalysisFailure.capture(exc)
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
    """Return a filename- and object-identity-independent AST/spans key."""

    # PythonBindingIndex lookup keys include source spans.  Excluding attributes
    # here aliases location-shifted trees to an index whose call/expression keys
    # cannot match the new nodes.
    serialized = ast.dump(tree, annotate_fields=True, include_attributes=True)
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
        tree = ast.parse(
            source, filename=filename, feature_version=policy.target_python
        )
        return _Analyzer(policy, digest).analyze(tree)

    return _INDEX_CACHE.get_or_compute(key, compute)


__all__ = [
    "PythonBindingPolicy",
    "analyze_python_bindings",
    "analyze_python_source_bindings",
    "python_ast_digest",
    "python_source_digest",
]
