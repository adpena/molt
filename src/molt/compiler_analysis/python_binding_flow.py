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
    PythonBindingTelemetry,
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
from molt.compiler_analysis.python_imports import import_metadata_target_name
from molt.compiler_analysis.python_source_keys import python_pattern_capture_names


_ANALYSIS_SCHEMA: Final = 4
_MAX_LOOP_FIXPOINT_STEPS: Final = 8
_METADATA_NAMES: Final = frozenset({"__name__", "__package__", "__spec__", "__path__"})
_RELEASE_CALLBACK_EFFECTS: Final[EffectMask] = (
    RELEASES_REFERENCE | RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK
)
_IMPORT_EXECUTION_INVALID_MEMBERS: Final[MemberMask] = ALL_INVALID_MEMBERS & ~int(
    PythonMember.TYPING_TYPE_CHECKING
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
            resolution = _UNBOUND_BINDING_RESOLUTION
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
                    clean = bool(left.clean_mask & slot_bit) and bool(
                        right.clean_mask & slot_bit
                    )
                    if clean and taint_mask & slot_bit:
                        clean = (
                            left_epoch == taint_epoch
                            and right_epoch == taint_epoch
                            and left.clean_epochs[chunk_offset] == left_epoch
                            and right.clean_epochs[chunk_offset] == right_epoch
                        )
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
                    if parent_clean and taint_mask & slot_bit:
                        parent_clean = chunk.clean_epochs[chunk_offset] == parent_epoch
                    clean = clean and parent_clean
                    if clean and taint_mask & slot_bit:
                        clean = parent_epoch == taint_epoch
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
        if value != UNBOUND_IDENTITY and not resolution.clean:
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
        current_value, current_static_value, clean = self._binding_details(
            state_id, slot
        )
        if current_value == value and current_static_value == static_value and clean:
            return state_id
        state = self._states[state_id]
        return self.intern(
            _BindingState(
                parents=(state_id,),
                updated_slot=slot,
                updated_value=value,
                updated_static_value=static_value,
                updated_clean=True,
                taint_epoch=state.taint_epoch,
                maybe_invalidated_members=state.maybe_invalidated_members,
                definitely_invalidated_members=state.definitely_invalidated_members,
            )
        )

    def set_bindings(
        self,
        state_id: int,
        bindings: Sequence[tuple[int, IdentityMask]],
    ) -> int:
        updates: list[tuple[int, IdentityMask, PythonStaticValue, bool]] = []
        for slot, value in bindings:
            current_value, current_static_value, clean = self._binding_details(
                state_id, slot
            )
            if current_value == value and current_static_value is None and clean:
                continue
            updates.append((slot, value, None, True))
        if not updates:
            return state_id
        state = self._states[state_id]
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

    def taint_slots(self, state_id: int, slots: int) -> int:
        if slots & self._taint_domain_mask:
            state = self._states[state_id]
            state_id = self.intern(
                _BindingState(
                    parents=(state_id,),
                    taint_epoch=state.taint_epoch + 1,
                    maybe_invalidated_members=state.maybe_invalidated_members,
                    definitely_invalidated_members=(
                        state.definitely_invalidated_members
                    ),
                )
            )
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
    if isinstance(target, ast.pattern):
        return set(python_pattern_capture_names(target))
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
        self.bound.update(
            alias.asname or alias.name.split(".", 1)[0] for alias in node.names
        )

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        self.bound.update(
            alias.asname or alias.name for alias in node.names if alias.name != "*"
        )

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


class _DeferredLoadCollector(ast.NodeVisitor):
    """Collect names loaded by one deferred body without per-job class cycles."""

    def __init__(self) -> None:
        self.loaded: set[str] = set()

    def visit_Name(self, node: ast.Name) -> None:
        if isinstance(node.ctx, ast.Load):
            self.loaded.add(node.id)

    def _visit_function_definition(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> None:
        for decorator in node.decorator_list:
            self.visit(decorator)
        for default in (*node.args.defaults, *node.args.kw_defaults):
            if default is not None:
                self.visit(default)

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self._visit_function_definition(node)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self._visit_function_definition(node)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        for expression in (
            *node.decorator_list,
            *node.bases,
            *(keyword.value for keyword in node.keywords),
        ):
            self.visit(expression)
        for statement in node.body:
            self.visit(statement)

    def visit_Lambda(self, node: ast.Lambda) -> None:
        for default in (*node.args.defaults, *node.args.kw_defaults):
            if default is not None:
                self.visit(default)


def _scope_declarations(
    body: Sequence[ast.stmt],
    parameters: Iterable[str] = (),
) -> _ScopeDeclarations:
    collector = _DeclarationCollector()
    for statement in body:
        collector.visit(statement)
    bound = (
        (collector.bound | set(parameters)) - collector.globals - collector.nonlocals
    )
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


def _literal_string(node: ast.AST | None) -> str | None:
    return (
        node.value
        if isinstance(node, ast.Constant) and isinstance(node.value, str)
        else None
    )


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
    module_base_bindings: tuple[tuple[int, IdentityMask], ...]
    lexical_history: list[int] | None
    lexical_history_start: int
    lexical_base_bindings: tuple[tuple[int, IdentityMask], ...]
    parameter_default_identities: tuple[tuple[str, IdentityMask], ...]


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
                last_taint = self.taint_indices[-1]
                bound_before_taint = base_value != UNBOUND_IDENTITY
                if not bound_before_taint and rows is not None:
                    indices, event_values, _suffix_values = rows
                    event_index = bisect_left(indices, start)
                    while (
                        event_index < len(indices)
                        and indices[event_index] <= last_taint
                    ):
                        if event_values[event_index] != UNBOUND_IDENTITY:
                            bound_before_taint = True
                            break
                        event_index += 1
                if bound_before_taint:
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
        self.calls: dict[PythonNodeKey, PythonCallSiteFact] = {}
        self.function_jobs: list[_FunctionJob] = []
        self.module_scope: _Scope | None = None
        self.module_slots: list[int] = []
        self.module_slot_mask = 0
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
    ) -> None:
        module_slots, lexical_slots = self._deferred_slot_dependencies(node, scope)
        lexical_history = (
            self._observed_stack[-1] if lexical_slots and self._observed_stack else None
        )
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
                tuple(
                    (slot, self.states.binding(state_id, slot)) for slot in module_slots
                ),
                lexical_history,
                len(lexical_history) if lexical_history is not None else 0,
                tuple(
                    (slot, self.states.binding(state_id, slot))
                    for slot in lexical_slots
                ),
                parameter_default_identities,
            )
        )

    def _deferred_slot_dependencies(
        self,
        node: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda,
        parent_scope: _Scope,
    ) -> tuple[tuple[int, ...], tuple[int, ...]]:
        parameters = _argument_names(node.args)
        body = (
            node.body if isinstance(node.body, list) else [ast.Return(value=node.body)]
        )
        declarations = _scope_declarations(body, parameters)
        collector = _DeferredLoadCollector()
        for statement in body:
            collector.visit(statement)
        loaded = collector.loaded
        module_slots: set[int] = set()
        lexical_slots: set[int] = set()
        assert self.module_scope is not None
        for name in loaded:
            if name in declarations.bound:
                continue
            if name in declarations.globals:
                slot = self.module_scope.slots.get(name)
            else:
                slot = None
                visible: _Scope | None = parent_scope
                while visible is not None:
                    candidate = visible.slots.get(name)
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
        self._queue_function(deferred, scope, state_id)

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
                    self.states.set_taint_domain(self.module_slot_mask)
                    self._scope_slot_cache.clear()
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
        cache_key = (scope.scope_id, name)
        if cache_key in self._scope_slot_cache:
            return self._scope_slot_cache[cache_key]
        if name in scope.globals:
            slot = self._module_slot(name)
        elif name in scope.nonlocals:
            slot = self._nonlocal_slot(scope, name)
        else:
            slot = scope.slots.get(name)
            parent = scope.parent
            while slot is None and parent is not None:
                slot = parent.slots.get(name)
                parent = parent.parent
        self._scope_slot_cache[cache_key] = slot
        return slot

    def _lookup_name(self, state_id: int, scope: _Scope, name: str) -> IdentityMask:
        slot = self._slot_for_name(scope, name)
        if slot is not None:
            value = self.states.binding(state_id, slot)
            if (
                value != UNBOUND_IDENTITY
                or scope.kind in {"function", "lambda", "comprehension"}
                and name in scope.locals
            ):
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
        identities: IdentityMask,
        effects: EffectMask,
        static_value: PythonStaticValue,
    ) -> None:
        key = self._node_key(node)
        self.expressions[key] = PythonExpressionFact(
            key,
            scope.scope_id,
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
        return result, effects, state_id

    def _expression_identity(self, node: ast.AST) -> IdentityMask:
        fact = self.expressions.get(self._node_key(node))
        return fact.identities if fact is not None else OTHER_IDENTITY

    def eval_expr(
        self, node: ast.expr, state_id: int, scope: _Scope
    ) -> _ExpressionResult:
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
            if (
                owner.identities & int(PythonIdentity.SYS_MODULES)
                and isinstance(node.slice, ast.Name)
                and node.slice.id == "__name__"
            ):
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
            state_id = self._apply_effects(state_id, effects)
            key = self._node_key(node)
            self.calls[key] = PythonCallSiteFact(
                key,
                scope.scope_id,
                callee_result.identities,
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
            state_id, defaults_effect, defaults = self._eval_arguments(
                node.args, state_id, scope
            )
            effects |= defaults_effect | ALLOCATES
            identities = exact_identity(PythonIdentity.USER_FUNCTION)
            self._queue_function(node, scope, state_id, defaults)
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
                    current = self._apply_effects(result.state_id, callback_effects)
                else:
                    current = result.state_id
            state_id = self.states.join(*possible_exits)
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
                static_value = tuple(
                    cast(str, value) for value in element_static_values
                )
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
                    effects |= (
                        INVOKES_ITERATION_CALLBACK | EXECUTES_ARBITRARY_PYTHON | RAISES
                    )
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
        self._record_expression(node, scope, identities, effects, static_value)
        self._record_state(state_id)
        return _ExpressionResult(state_id, identities, effects, static_value)

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
        declarations = _ScopeDeclarations(frozenset(names), frozenset(), frozenset())
        scope = self._new_scope(
            parent=parent,
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
        current = first_iterable.state_id
        for index, generator in enumerate(generators):
            if index:
                iterable = self.eval_expr(generator.iter, current, scope)
                current = iterable.state_id
                deferred_effects |= iterable.effects
            deferred_effects |= (
                INVOKES_ITERATION_CALLBACK | EXECUTES_ARBITRARY_PYTHON | RAISES
            )
            current, target_effects = self.assign_target(
                generator.target, OTHER_IDENTITY, current, scope
            )
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
            state_id = self.states.set_binding(state_id, slot, value, static_value)
            return state_id, effects
        if isinstance(target, (ast.Tuple, ast.List)):
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
        if isinstance(target, ast.Attribute):
            owner = self.eval_expr(target.value, state_id, scope)
            state_id = self._invalidate_member_target(
                owner.state_id, owner.identities, target.attr
            )
            effects |= (
                owner.effects
                | WRITES_OBJECT_STATE
                | INVOKES_DESCRIPTOR
                | EXECUTES_ARBITRARY_PYTHON
                | _RELEASE_CALLBACK_EFFECTS
                | RAISES
            )
            if (
                owner.identities & int(PythonIdentity.CURRENT_MODULE)
                and target.attr in _METADATA_NAMES
            ):
                effects |= WRITES_MODULE_METADATA | WRITES_GLOBAL_NAMESPACE
            return self._apply_effects(state_id, effects), effects
        if isinstance(target, ast.Subscript):
            owner = self.eval_expr(target.value, state_id, scope)
            index = self.eval_expr(target.slice, owner.state_id, scope)
            effects |= (
                owner.effects
                | index.effects
                | WRITES_OBJECT_STATE
                | EXECUTES_ARBITRARY_PYTHON
                | _RELEASE_CALLBACK_EFFECTS
                | RAISES
            )
            if owner.identities & int(PythonIdentity.CURRENT_GLOBALS):
                effects |= WRITES_GLOBAL_NAMESPACE
                if _literal_string(target.slice) in _METADATA_NAMES:
                    effects |= WRITES_MODULE_METADATA
            return self._apply_effects(index.state_id, effects), effects
        return self._apply_effects(state_id, UNKNOWN_EFFECTS), UNKNOWN_EFFECTS

    def delete_target(
        self, target: ast.AST, state_id: int, scope: _Scope
    ) -> tuple[int, EffectMask]:
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
        synthetic = ast.Name(id=name, ctx=ast.Store())
        return self.assign_target(synthetic, value, state_id, scope)

    def exec_statements(
        self, body: Sequence[ast.stmt], state_id: int, scope: _Scope
    ) -> tuple[int, EffectMask]:
        effects = NO_EFFECTS
        for statement in body:
            state_id, statement_effects = self.exec_statement(
                statement, state_id, scope
            )
            effects |= statement_effects
            self._record_state(state_id)
            if scope.kind == "module":
                self._module_history.append(state_id)
        return state_id, effects

    def exec_statement(
        self, node: ast.stmt, state_id: int, scope: _Scope
    ) -> tuple[int, EffectMask]:
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
            target_read = (
                self.eval_expr(node.target, state_id, scope)
                if isinstance(node.target, ast.expr)
                else _ExpressionResult(state_id, OTHER_IDENTITY, NO_EFFECTS)
            )
            value = self.eval_expr(node.value, target_read.state_id, scope)
            effects |= (
                target_read.effects | value.effects | EXECUTES_ARBITRARY_PYTHON | RAISES
            )
            state_id, target_effects = self.assign_target(
                node.target, OTHER_IDENTITY, value.state_id, scope
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
                effects |= (
                    context.effects
                    | INVOKES_CONTEXT_CALLBACK
                    | EXECUTES_ARBITRARY_PYTHON
                    | RAISES
                )
                if item.optional_vars is not None:
                    state_id, target_effects = self.assign_target(
                        item.optional_vars, OTHER_IDENTITY, state_id, scope
                    )
                    effects |= target_effects
            state_id, body_effects = self.exec_statements(node.body, state_id, scope)
            effects |= (
                body_effects
                | INVOKES_CONTEXT_CALLBACK
                | EXECUTES_ARBITRARY_PYTHON
                | RAISES
            )
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
                    branch, target_effects = self._bind_name(
                        name, OTHER_IDENTITY, branch, scope
                    )
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
            state_id, default_effects, defaults = self._eval_arguments(
                node.args, state_id, scope
            )
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
            self._queue_function(node, scope, state_id, defaults)
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
            class_scope = self._new_scope(
                parent=scope, kind="class", name=node.name, declarations=declarations
            )
            class_state, class_effects = self.exec_statements(
                node.body, state_id, class_scope
            )
            effects |= class_effects | EXECUTES_ARBITRARY_PYTHON | ALLOCATES | RAISES
            state_id = self._apply_effects(state_id, effects)
            class_identity = possible_identity(PythonIdentity.USER_CLASS)
            state_id, bind_effects = self._bind_name(
                node.name, class_identity, state_id, scope
            )
            effects |= bind_effects
        elif isinstance(node, getattr(ast, "TypeAlias", ())):
            for expression in (
                node.value,
                *(
                    value
                    for type_param in node.type_params
                    for attribute in ("bound", "default_value")
                    if isinstance(
                        (value := getattr(type_param, attribute, None)), ast.expr
                    )
                ),
            ):
                self._queue_annotation_expression(
                    expression,
                    scope,
                    state_id,
                    node.type_params,
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
        elif isinstance(
            node, (ast.Global, ast.Nonlocal, ast.Pass, ast.Break, ast.Continue)
        ):
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

    def _exec_loop(
        self, node: ast.For | ast.AsyncFor | ast.While, state_id: int, scope: _Scope
    ) -> tuple[int, EffectMask]:
        effects = NO_EFFECTS
        entry = state_id
        if isinstance(node, (ast.For, ast.AsyncFor)):
            iterable = self.eval_expr(node.iter, entry, scope)
            entry = iterable.state_id
            effects |= (
                iterable.effects
                | INVOKES_ITERATION_CALLBACK
                | EXECUTES_ARBITRARY_PYTHON
                | RAISES
            )
        else:
            test = self.eval_expr(node.test, entry, scope)
            entry = test.state_id
            effects |= test.effects | INVOKES_COMPARISON_CALLBACK | RAISES
        header = entry
        for _step in range(_MAX_LOOP_FIXPOINT_STEPS):
            body_entry = header
            if isinstance(node, (ast.For, ast.AsyncFor)):
                body_entry, target_effects = self.assign_target(
                    node.target, OTHER_IDENTITY, body_entry, scope
                )
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

    def _exec_try(
        self, node: ast.Try | ast.TryStar, state_id: int, scope: _Scope
    ) -> tuple[int, EffectMask]:
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
                branch, bind_effects = self._bind_name(
                    handler.name, OTHER_IDENTITY, branch, scope
                )
                effects |= bind_effects
            branch, handler_effects = self.exec_statements(handler.body, branch, scope)
            effects |= handler_effects
            if handler.name:
                branch, delete_effects = self.delete_target(
                    ast.Name(id=handler.name, ctx=ast.Del()), branch, scope
                )
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
        body = (
            node.body if isinstance(node.body, list) else [ast.Return(value=node.body)]
        )
        declarations = _scope_declarations(body, parameters)
        kind: _ScopeKind = "lambda" if isinstance(node, ast.Lambda) else "function"
        name = "<lambda>" if isinstance(node, ast.Lambda) else node.name
        scope = self._new_scope(
            parent=job.parent_scope, kind=kind, name=name, declarations=declarations
        )
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
        try:
            exit_state, _effects = self.exec_statements(body, state_id, scope)
        finally:
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
        updates: list[tuple[int, IdentityMask]] = []
        for slot, base_binding in base_bindings:
            future_binding = summary.binding(self.states, summary_start, slot)
            updates.append((slot, base_binding | future_binding))
        return self.states.set_bindings(state_id, updates)

    def analyze(self, tree: ast.Module) -> PythonBindingIndex:
        declarations = _scope_declarations(tree.body)
        module = self._new_scope(
            parent=None, kind="module", name="<module>", declarations=declarations
        )
        self.module_scope = module
        self.module_slots = list(module.slots.values())
        self.module_slot_mask = sum(1 << slot for slot in self.module_slots)
        self.states.set_taint_domain(self.module_slot_mask)
        observed: list[int] = [0]
        self._observed_stack.append(observed)
        try:
            module_exit, _effects = self.exec_statements(tree.body, 0, module)
        finally:
            self._observed_stack.pop()
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
            calls=tuple(sorted(self.calls.values(), key=lambda fact: fact.node)),
            scopes=scope_facts,
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
