from __future__ import annotations

from molt.compiler_analysis.python_function_kind import (
    IMPLICIT_DESCRIPTOR_NAMES,
    FunctionKind,
)
from molt.compiler_analysis.python_binding_facts import (
    PythonFunctionResult,
    PythonSourceArgument,
    PythonSourceCall,
    PythonSourceCallableDeclaration,
    PythonSourceElement,
    PythonNativeInitializerSource,
    PythonSourceClass,
)

from collections import defaultdict, deque
from dataclasses import dataclass, field
from enum import IntFlag, StrEnum
from collections.abc import Callable, Collection, Iterable, Iterator, Mapping, Set
from typing import Literal, NamedTuple, assert_never, cast

from molt.compiler_analysis.python_builtin_shapes import (
    MappingWriteScope,
    builtin_mapping_write_scope,
)
from molt.compiler_analysis.python_imports import (
    SourceAllExports,
    SourceDemandPhase,
    _ImportScanRequests,
    _SourceImportBinding,
    SourceAttributeWrite,
    _SourceValueDemand,
    SourceImportCall,
    normalize_stdlib_module_name,
)
from molt.compiler_analysis.python_namespace_writes import PythonNamespaceWrite
from molt.compiler_analysis.python_source_keys import (
    PythonElementProtocol,
    PythonSourceKey,
    PythonSourceProjection,
)
from molt.compiler_analysis.python_parameter_classes import representative_arguments
from molt.compiler_analysis.python_identity_catalog_generated import (
    PythonIdentity,
    INTRINSIC_ACQUISITION_IDENTITIES,
    INTRINSIC_RESULT_IDENTITIES,
    NATIVE_ITERATOR_PAYLOADS,
    NATIVE_ITERATOR_MEMBERS,
    PythonIteratorPayloadKind,
)
from molt.compiler_analysis.python_value_identity import (
    NATIVE_SELECTOR_CONSTRUCTORS,
    SOURCE_CALLABLE_IDENTITIES,
    SOURCE_ITERATION_CALLABLES,
    possible_runtime_import_call_identities,
)
from molt.compiler_analysis.static_truth import (
    IMPLICIT_INSTANCE_SPECIAL_METHODS,
    context_execution_protocol,
)


_EMPTY_CANDIDATES: frozenset[SourceCandidate] = frozenset()


class ClassLifecycleWrite(IntFlag):
    NONE = 0
    INIT_SUBCLASS = 1
    MRO = 2


_CLASS_LIFECYCLE_WRITES = {
    "__init_subclass__": ClassLifecycleWrite.INIT_SUBCLASS,
    "__bases__": ClassLifecycleWrite.MRO,
    "__class__": ClassLifecycleWrite.MRO,
}


class PythonNamespaceOwnership(StrEnum):
    BORROWED = "borrowed"
    OWNED = "owned"


# `object`'s own methods that hand their receiver to no Python code: a
# receiver that reaches one of them through no class definition stays bound.
_OBJECT_METHODS_WITHOUT_CALLBACKS = frozenset(
    {
        "__init__",
        "__init_subclass__",
        "__new__",
        "__hash__",
        "__eq__",
        "__sizeof__",
    }
)


type _SourceOrigin = tuple[str, PythonSourceKey | None]


class SourceCandidate(NamedTuple):
    module: str
    source: PythonSourceKey | None
    kind: Literal[
        "value",
        "instance",
        "resumable",
        "iterator",
        "iterator_next",
        "iterator_self",
        "native_selector",
        "reflective_result",
        "class_descriptor",
        "static_descriptor",
        "property_descriptor",
        "bound_class_method",
        "bound_static_method",
        "namespace",
        "namespace_get",
        "namespace_remove",
        "namespace_popitem",
        "namespace_clear",
        "namespace_write_descriptor",
        "namespace_write_method",
        "namespace_copy_method",
        "namespace_values_method",
        "namespace_items_method",
        "namespace_values",
        "namespace_items",
        "native_initializer",
        "bound_native_initializer",
    ] = "value"
    contained: bool = False
    identity: PythonIdentity | None = None
    receiver: tuple[str, PythonSourceKey | None] | None = None
    namespace_ownership: PythonNamespaceOwnership | None = None
    namespace_mutation: MappingWriteScope | None = None
    iterator_producer: PythonIdentity | None = None

    @property
    def value(self) -> _SourceOrigin:
        return self.module, self.source


_INTERPRETER_BUILTINS = SourceCandidate(
    "builtins", None, "namespace", namespace_ownership=PythonNamespaceOwnership.BORROWED
)
_INTERPRETER_IMPORT = SourceCandidate(
    "builtins", None, identity=PythonIdentity.BUILTINS_IMPORT
)


def _candidate_order(value: object) -> tuple[object, ...]:
    """A total order over candidates whose optional fields may be None: two
    candidates of one value can differ in a source, receiver or identity that
    one of them lacks, which plain tuple order cannot compare."""
    if value is None:
        return (0,)
    if isinstance(value, tuple):
        return (1, tuple(_candidate_order(item) for item in value))
    if isinstance(value, str):
        return (2, value)
    if isinstance(value, int):
        return (3, int(value))
    raise TypeError(f"unordered candidate field: {value!r}")


@dataclass(frozen=True, slots=True)
class SourceMember:
    owner: SourceCandidate
    name: str


@dataclass(frozen=True, slots=True)
class _ClassMemberLookup:
    owner: SourceCandidate
    name: str
    strict: bool


@dataclass(frozen=True, slots=True)
class _ClassMemberProviders:
    source: PythonSourceKey | None
    name: str


@dataclass(slots=True)
class _ProviderSearchFrame:
    node: SourceValue
    children: Iterator[_SourceOrigin]
    index: int
    component_start: int
    providers: frozenset[_SourceOrigin]

    def join(self, providers: frozenset[_SourceOrigin]) -> None:
        if not self.providers:
            self.providers = providers
        elif providers and self.providers is not providers:
            self.providers = self.providers | providers


@dataclass(frozen=True, slots=True)
class _ClassMemberResult:
    members: frozenset[SourceCandidate]
    property_results: frozenset[SourceCandidate]


@dataclass(slots=True)
class _InheritedSources:
    owners: set[_SourceOrigin]
    users: set[SourceValue] = field(default_factory=set)


@dataclass(frozen=True, slots=True)
class SourceCacheItem:
    key: str | None
    # None normally means every key. A wildcard write is also a possible
    # provider for each literal-key read without mixing unrelated literal keys.
    wildcard: bool = False


def _import_cache_items(keys: tuple[str, ...] | None) -> tuple[SourceCacheItem, ...]:
    if keys is None:
        return (SourceCacheItem(None),)
    # A nonliteral key may name any one of these entries.
    return tuple(SourceCacheItem(key) for key in keys) + (SourceCacheItem(None, True),)


# Each operation is a distinct graph key even when it consumes the same
# source value. Tuple field equality would merge call, await and delegation.
@dataclass(frozen=True, slots=True)
class SourceReturn:
    callee: SourceValue


@dataclass(frozen=True, slots=True)
class SourceCallableOutcome:
    owner: PythonSourceKey
    yielded: bool


@dataclass(frozen=True, slots=True)
class SourceCallArguments:
    site: PythonSourceKey


@dataclass(frozen=True, slots=True)
class SourceIteratorItems:
    site: PythonSourceKey
    producer: PythonIdentity


@dataclass(frozen=True, slots=True)
class SourceAwait:
    value: SourceValue


@dataclass(frozen=True, slots=True)
class SourceSubclassInvocation:
    owner: SourceCandidate


@dataclass(frozen=True, slots=True)
class SourceGeneratorCompletion:
    value: SourceValue


SourceValue = tuple[
    str,
    PythonSourceKey
    | SourceMember
    | _ClassMemberLookup
    | _ClassMemberProviders
    | SourceCacheItem
    | SourceReturn
    | SourceCallArguments
    | SourceCallableOutcome
    | SourceIteratorItems
    | SourceAwait
    | SourceGeneratorCompletion
    | SourceSubclassInvocation
    | None,
]
_OperandDeltas = dict[SourceValue, set[SourceCandidate]]


class SourceDemand(NamedTuple):
    phase: SourceDemandPhase
    arguments: tuple[str, ...]
    argument_count: int | None
    consumer: str


type NativeActivation = tuple[SourceCandidate, SourceDemand]


@dataclass(slots=True)
class _SourceNamespace:
    members: dict[str, frozenset[PythonSourceKey]] = field(default_factory=dict)
    named_origins: set[PythonSourceKey] = field(default_factory=set)
    exposure: frozenset[PythonSourceKey] = frozenset()

    def add(
        self,
        names: tuple[tuple[str, frozenset[PythonSourceKey]], ...],
        exposure: frozenset[PythonSourceKey],
    ) -> tuple[bool, set[PythonSourceKey]]:
        """Publish member deltas and return newly reachable origins once."""
        changed = not exposure.issubset(self.exposure)
        added = set(exposure.difference(self.exposure, self.named_origins))
        if changed:
            self.exposure |= exposure
        for name, origins in names:
            previous = self.members.get(name)
            if previous is not None and origins.issubset(previous):
                continue
            self.members[name] = origins if previous is None else previous | origins
            added.update(origins.difference(self.named_origins, self.exposure))
            self.named_origins.update(origins)
            changed = True
        return changed, added

    def origins(
        self, key: str | None = None
    ) -> set[PythonSourceKey] | frozenset[PythonSourceKey]:
        if key is None:
            return (
                self.named_origins | self.exposure
                if self.exposure
                else self.named_origins
            )
        members = self.members.get(key, frozenset())
        return members | self.exposure if self.exposure else members


_EMPTY_NAMESPACE = _SourceNamespace()


class PythonSourceDemands:
    """One incremental source-value activation worklist for import discovery.

    Source records never contain topology verdicts. The graph owner supplies
    current module admission and body scheduling. Candidate/demand incidences
    are joined once; dependency wakes process only new candidates or demands.
    """

    def __init__(
        self,
        *,
        admit_imports: Callable[[str, tuple[str, ...]], None],
        queue_body: Callable[[str, PythonSourceKey, tuple[str, ...]], None],
        require_runtime_import_protocol: Callable[[str], None],
    ) -> None:
        self.admit_imports = admit_imports
        self.queue_body = queue_body
        self.require_runtime_import_protocol = require_runtime_import_protocol
        self._consumer: str | None = None
        # Reading a callable requires its executable body at publication
        # even when this incidence does not execute it or scan its imports.
        self.read_callables: dict[str, set[PythonSourceKey]] = {}
        # A monotone projection of existing activation transitions. The delta
        # wakes only source scans whose globals-custody proof has changed.
        self._opaque_activation_sources: dict[str, set[PythonSourceKey]] = {}
        self._opaque_activation_changes: set[str] = set()
        self._opaque_activation_producers: dict[str, set[str]] = {}
        self.modules: set[str] = set()
        self.namespace_writes: dict[str, set[PythonNamespaceWrite]] = {}
        self.owner_demands: dict[
            tuple[str, PythonSourceKey],
            set[_SourceValueDemand],
        ] = {}
        self.argument_exposures: dict[
            tuple[str, PythonSourceKey], set[PythonSourceKey]
        ] = {}
        self.imported_modules: set[str] = set()
        self.value_candidates: dict[
            SourceValue, set[SourceCandidate] | frozenset[SourceCandidate]
        ] = {}
        self.value_users: defaultdict[SourceValue, set[SourceValue]] = defaultdict(set)
        self.operand_users: defaultdict[SourceValue, set[SourceValue]] = defaultdict(
            set
        )
        self.pending_operands: dict[SourceValue, _OperandDeltas] = {}
        # A call can publish plain function outcomes without rerunning its
        # callee/effect transfer. A structural refresh replaces these routes.
        self.call_outcome_inputs: dict[SourceValue, set[SourceValue]] = {}
        self.call_outcome_users: dict[SourceValue, set[SourceValue]] = {}
        self.pending_call_outcomes: set[SourceValue] = set()
        # Binding subscribers want membership deltas and receiver-binding
        # changes, without unrelated structural wakes.
        self.binding_users: defaultdict[SourceValue, set[SourceValue]] = defaultdict(
            set
        )
        self.declaration_users: dict[SourceValue, set[SourceValue]] = defaultdict(set)
        self.value_phases: defaultdict[SourceValue, set[SourceDemand]] = defaultdict(
            set
        )
        self.pending_phases: defaultdict[SourceValue, set[SourceDemand]] = defaultdict(
            set
        )
        self.elements: dict[SourceValue, PythonSourceElement] = {}
        self.cache_origins: dict[SourceCacheItem, set[tuple[SourceValue, bool]]] = {}
        self.cache_reads: dict[
            SourceValue,
            frozenset[
                tuple[
                    tuple[str, ...] | None,
                    frozenset[PythonSourceKey],
                    frozenset[PythonSourceKey],
                    bool,
                ]
            ],
        ] = {}
        self.calls: dict[SourceValue, PythonSourceCall] = {}
        self.callables: dict[SourceValue, int] = {}
        self.call_imports: dict[SourceValue, frozenset[SourceImportCall]] = {}
        self.admitted_call_imports: set[tuple[SourceValue, PythonIdentity, str]] = set()
        self.completions: dict[SourceValue, frozenset[PythonSourceKey]] = {}
        self.declarations: dict[SourceValue, PythonSourceCallableDeclaration] = {}
        self.pending_arguments: dict[SourceValue, set[SourceCandidate]] = {}
        self.functions: dict[SourceValue, PythonFunctionResult] = {}
        self._return_projection_users: dict[SourceValue, set[SourceValue]] = {}
        self._formal_bindings: dict[
            tuple[PythonSourceCallableDeclaration, tuple[int | str | None, ...], bool],
            tuple[tuple[PythonSourceKey, int, bool], ...],
        ] = {}
        self.activation_partitions: dict[
            SourceValue, tuple[frozenset[str] | None, ...]
        ] = {}
        self.native_initializers: dict[SourceValue, PythonNativeInitializerSource] = {}
        self.native_initializers_by_receiver: dict[SourceValue, set[SourceValue]] = {}
        self.native_calls: dict[
            SourceValue, set[tuple[SourceValue, SourceCandidate]]
        ] = {}
        self.classes: dict[SourceValue, PythonSourceClass] = {}
        self.class_lifecycle_writes: dict[SourceValue, ClassLifecycleWrite] = {}
        self.pending_modules: dict[str, set[SourceValue]] = {}
        # Per attribute value: the functions it selected through an instance
        # (the receiver fills their first parameter) and through anything else.
        self.receiver_bound_members: dict[SourceValue, set[SourceCandidate]] = {}
        self.unbound_members: dict[SourceValue, set[SourceCandidate]] = {}
        # Per attribute value: the values a property getter returned to it. A
        # call of one passes the call's own arguments, never the instance.
        self._property_results: dict[SourceValue, set[SourceCandidate]] = {}
        self._class_members: dict[_ClassMemberLookup, _ClassMemberResult] = {}
        self._member_providers: dict[SourceValue, frozenset[_SourceOrigin]] = {}
        self._inherited_sources: dict[SourceValue, _InheritedSources] = {}
        # Attribute values whose receiver-binding proof read a candidate set
        # still queued; `drain` decides them once the queue empties.
        self._deferred_receiver_proofs: set[SourceValue] = set()
        # Per function value: the class statement that built each receiver a
        # selection bound to its first parameter (None for any other
        # receiver), and the functions some call entered with no receiver.
        self._receiver_classes: dict[
            tuple[str, PythonSourceKey], set[tuple[str, PythonSourceKey] | None]
        ] = {}
        self._direct_entries: set[SourceValue] = set()
        self.star_aliases: dict[str, set[PythonSourceKey]] = {}
        self.escaped_modules: dict[str, set[SourceDemand]] = {}
        self.attribute_writes: dict[SourceValue, set[SourceAttributeWrite]] = {}
        self.owner_attribute_writes: dict[SourceValue, set[SourceAttributeWrite]] = {}
        self.pending_attribute_writes: set[tuple[str, str, SourceAttributeWrite]] = (
            set()
        )
        self.reached_attribute_writes: set[tuple[str, str, SourceAttributeWrite]] = (
            set()
        )
        self.namespaces: dict[SourceValue, _SourceNamespace] = {}
        self.bases: dict[SourceValue, frozenset[PythonSourceKey]] = {}
        self.attributes: dict[SourceValue, dict[str, frozenset[PythonSourceKey]]] = {}
        self.import_bindings: dict[SourceValue, _SourceImportBinding] = {}
        # Pending values carry whether their candidate transfer must run.
        # Additive operand work and demand-only work need no full refresh.
        self.value_jobs: dict[SourceValue, bool] = {}
        # Body scans by class representative, and every argument tuple that
        # activated each body, so a changed parameter fact re-maps them.
        self.body_jobs: dict[SourceValue, set[tuple[str, ...]]] = {}
        self.body_activations: dict[SourceValue, set[tuple[str, ...]]] = {}
        # Incidences with the same demand share their activation frontier,
        # even when a candidate arrives through multiple source expressions.
        self.activated: dict[SourceDemand, set[SourceCandidate]] = {}
        # An escaped namespace also writes at the particular use site.
        # Those incidences still visit the site after candidate activation.
        self._site_sensitive_activated: set[tuple[SourceCandidate, SourceDemand]] = (
            set()
        )
        self.native_activations: dict[SourceValue, set[NativeActivation]] = {}
        self.queue: deque[SourceValue] = deque()
        self.exports: dict[str, SourceAllExports] = {}

    def queue_value(self, value: SourceValue, *, refresh: bool = True) -> None:
        if isinstance(value[1], _ClassMemberProviders):
            self._invalidate_member_providers(value)
            return
        pending = self.value_jobs.get(value)
        if pending is None:
            self.queue.append(value)
            self.value_jobs[value] = refresh
        elif refresh and not pending:
            self.value_jobs[value] = True

    def demand(
        self,
        value: SourceValue,
        phase: SourceDemandPhase,
        arguments: tuple[str, ...] = (),
        *,
        argument_count: int | None = None,
        consumer: str | None = None,
    ) -> None:
        phases = self.value_phases[value]
        incidence = SourceDemand(
            phase, arguments, argument_count, consumer or self._consumer or value[0]
        )
        if incidence not in phases:
            phases.add(incidence)
            self.pending_phases[value].add(incidence)
            # Method and store demands also determine receiver binding
            # and escape in the attribute transfer.
            needs_candidates = (
                phase in {"method", "mutate"} or value not in self.value_candidates
            )
            self.queue_value(value, refresh=needs_candidates)

    def watch(
        self, origin: SourceValue, user: SourceValue
    ) -> set[SourceCandidate] | frozenset[SourceCandidate]:
        self.value_users[origin].add(user)
        return self._ensure_value(origin)

    def _operand_candidates(
        self, origin: SourceValue, user: SourceValue, deltas: _OperandDeltas | None
    ) -> Collection[SourceCandidate]:
        if deltas is not None:
            return deltas.get(origin, ())
        self.operand_users[origin].add(user)
        return tuple(self._ensure_value(origin))

    def _watch_inherited_sources(
        self, origin: SourceValue, user: SourceValue
    ) -> set[_SourceOrigin]:
        # An inherited selection takes only the source identity from the base
        # operand. The original class access supplies the receiver.
        sources = self._inherited_sources.get(origin)
        if sources is None:
            sources = self._inherited_sources[origin] = _InheritedSources(
                {candidate.value for candidate in self._ensure_value(origin)}
            )
        sources.users.add(user)
        return sources.owners

    def _add_operand_candidates(
        self, origin: SourceValue, added: Collection[SourceCandidate]
    ) -> None:
        for subscribers in (self.operand_users, self.binding_users):
            for user in subscribers.get(origin, ()):
                if self.value_jobs.get(user, False):
                    continue
                pending = self.pending_operands.setdefault(user, {})
                pending.setdefault(origin, set()).update(added)
                self.queue_value(user, refresh=False)

    def _ensure_value(
        self, origin: SourceValue
    ) -> set[SourceCandidate] | frozenset[SourceCandidate]:
        candidates = self.value_candidates.get(origin)
        if candidates is None:
            candidates = self.value_candidates[origin] = _EMPTY_CANDIDATES
            self.queue_value(origin)
        return candidates

    def _queue_arguments(
        self, formal: SourceValue, candidates: set[SourceCandidate]
    ) -> None:
        added = candidates.difference(self.value_candidates.get(formal, ()))
        if not added:
            return
        # Only the unprocessed wave is staged. The ordinary value publication
        # owns committed candidates and the module admission fence.
        pending = self.pending_arguments.setdefault(formal, set())
        before = len(pending)
        pending.update(added)
        if len(pending) != before:
            self.queue_value(formal)

    def _invalidate_value(self, value: SourceValue) -> None:
        if value in self.value_candidates:
            self.queue_value(value)
        for subscribers in (self.value_users, self.operand_users):
            for user in subscribers.get(value, ()):
                self.queue_value(user)

    def _publish[T](
        self, table: dict[SourceValue, T], value: SourceValue, fact: T
    ) -> bool:
        if table.get(value) == fact:
            return False
        table[value] = fact
        self._invalidate_value(value)
        return True

    def hold_cache_origins(
        self, item: SourceCacheItem, values: set[tuple[SourceValue, bool]]
    ) -> None:
        known = self.cache_origins.setdefault(item, set())
        additions = values - known
        if additions:
            known.update(additions)
            value = ("", item)
            # Publication alone does not read a cache entry. The first
            # demand or dependent initializes the entry from all its origins.
            if value in self.value_candidates or value in self.value_phases:
                self.queue_value(value)

    def accept(self, module_name: str, scan: _ImportScanRequests) -> None:
        self.modules.add(module_name)
        # Ordinary initialization can publish its admitted source namespace too.
        # This is may-provenance, not proof of cache presence or execution order.
        publication = {((module_name, None), False)}
        self.hold_cache_origins(SourceCacheItem(module_name), publication)
        self.hold_cache_origins(SourceCacheItem(None), publication)
        if self.exports.get(module_name) != scan.all_exports:
            self.exports[module_name] = scan.all_exports
            self._invalidate_value((module_name, None))
        self.imported_modules.update(
            (*scan.imports, *scan.dynamic_relative_import_candidates)
        )
        for owner, names, exposure in scan.source_values.namespaces:
            value = (module_name, None if owner[-1] == "Module" else owner)
            namespace = self.namespaces.get(value)
            new = namespace is None
            if namespace is None:
                namespace = self.namespaces[value] = _SourceNamespace()
            changed, added = namespace.add(names, exposure)
            if new or changed:
                self._invalidate_value(value)
                for incidence in tuple(self.value_phases.get(value, ())):
                    if incidence.phase in {
                        "escape",
                        "expose",
                        "descriptor",
                        "release_contents",
                    }:
                        for source in added:
                            self.demand(
                                (module_name, source),
                                "release"
                                if incidence.phase == "release_contents"
                                else incidence.phase,
                                incidence.arguments,
                                consumer=incidence.consumer,
                            )
        for info in scan.source_values.classes:
            value = (module_name, info.owner)
            prior = self.classes.get(value)
            if prior is not None:
                info = info._replace(
                    plain_metaclass=prior.plain_metaclass and info.plain_metaclass,
                    plain_attributes=tuple(
                        name
                        for name in info.plain_attributes
                        if name in prior.plain_attributes
                    ),
                )
            self._publish(self.classes, value, info)
        for owner, bases in scan.source_values.bases:
            value = (module_name, owner)
            previous_bases = self.bases.get(value, frozenset())
            if self._publish(self.bases, value, previous_bases | bases):
                for incidence in tuple(self.value_phases.get(value, ())):
                    if incidence.phase in {"descriptor", "escape", "expose"}:
                        for base in bases - previous_bases:
                            self.demand(
                                (module_name, base),
                                incidence.phase,
                                incidence.arguments,
                                consumer=incidence.consumer,
                            )
        for owner, name, origins in scan.source_values.attributes:
            value = (module_name, owner)
            selections = self.attributes.setdefault(value, {})
            previous = selections.get(name)
            joined = origins if previous is None else previous | origins
            if previous != joined:
                selections[name] = joined
                self._invalidate_value(value)
        for element in scan.source_values.elements:
            value = (module_name, element.owner)
            previous = self.elements.get(value)
            if previous is not None:
                if previous.protocol != element.protocol:
                    raise ValueError("source element changed protocol")
                element = element._replace(origins=previous.origins | element.origins)
                if element.key != previous.key:
                    element = element._replace(key=None)
            if self._publish(self.elements, value, element):
                for user in self._return_projection_users.get(value, ()):
                    self.queue_value(user)
        for transfer in scan.source_values.call_imports:
            value = (module_name, transfer.site.source)
            self._publish(
                self.call_imports,
                value,
                self.call_imports.get(value, frozenset()) | {transfer},
            )
        for source, identities in scan.source_values.callables:
            value = (module_name, source)
            self._publish(
                self.callables, value, self.callables.get(value, 0) | identities
            )
        for call in scan.source_values.calls:
            value = (module_name, call.owner)
            previous = self.calls.get(value)
            if previous is not None:
                call = call.join(previous)
            if self._publish(self.calls, value, call) and (
                previous is None
                or call.callees != previous.callees
                or call.arguments != previous.arguments
            ):
                operands = (module_name, SourceCallArguments(call.owner))
                if operands in self.value_candidates:
                    self.queue_value(operands)
        for initializer in scan.source_values.native_initializers:
            value = (module_name, initializer.owner)
            previous = self.native_initializers.get(value)
            if previous is not None:
                initializer = initializer._replace(
                    receiver=initializer.receiver | previous.receiver,
                    captures=initializer.captures | previous.captures,
                    attributes=tuple(
                        (name, origins | prior[1])
                        for (name, origins), prior in zip(
                            initializer.attributes, previous.attributes, strict=True
                        )
                    ),
                    parameters=initializer.parameters
                    if initializer.parameters == previous.parameters
                    else None,
                    capture_values=initializer.capture_values
                    if initializer.capture_values == previous.capture_values
                    else None,
                    stores=initializer.stores
                    if initializer.stores == previous.stores
                    else None,
                    post_inputs=initializer.post_inputs
                    if initializer.post_inputs == previous.post_inputs
                    else None,
                )
            self._publish(self.native_initializers, value, initializer)
        for source, origins in scan.source_values.completions:
            value = (module_name, source)
            self._publish(
                self.completions,
                value,
                self.completions.get(value, frozenset()) | origins,
            )
        for declaration in scan.source_values.declarations:
            owner = (module_name, declaration.owner)
            if self._publish(self.declarations, owner, declaration):
                for user in self.declaration_users.get(owner, ()):
                    self.queue_value(user)
                for user in self._return_projection_users.get(owner, ()):
                    self.queue_value(user)
        for owner, classes in scan.source_values.activation_partitions:
            value = (module_name, owner)
            previous_classes = self.activation_partitions.get(value)
            # Combine proofs across observed activations. An exact argument
            # required in one context must remain distinct in all of them.
            if previous_classes is not None:
                classes = tuple(
                    old | new if old is not None and new is not None else None
                    for old, new in zip(previous_classes, classes, strict=True)
                )
            if classes != previous_classes:
                self.activation_partitions[value] = classes
                for arguments in self.body_activations.get(value, ()):
                    self._schedule_body(value, arguments)
        for result in scan.source_values.functions:
            value = (module_name, result.owner)
            previous = self.functions.get(value)
            if previous is not None:
                result = result._replace(
                    returned=previous.returned | result.returned,
                    returned_exposed=previous.returned_exposed
                    | result.returned_exposed,
                    yielded=previous.yielded | result.yielded,
                    yielded_exposed=previous.yielded_exposed | result.yielded_exposed,
                    returns_calls_only=(
                        previous.returns_calls_only and result.returns_calls_only
                    ),
                )
            if previous != result:
                self.functions[value] = result
                if previous is None or (
                    previous.returned != result.returned
                    or previous.returned_exposed != result.returned_exposed
                ):
                    for user in self._return_projection_users.get(value, ()):
                        self.queue_value(user)
                    outcome = (module_name, SourceCallableOutcome(result.owner, False))
                    if outcome in self.value_candidates:
                        self.queue_value(outcome)
                if previous is None or (
                    previous.yielded != result.yielded
                    or previous.yielded_exposed != result.yielded_exposed
                ):
                    outcome = (module_name, SourceCallableOutcome(result.owner, True))
                    if outcome in self.value_candidates:
                        self.queue_value(outcome)
                old = (
                    frozenset()
                    if previous is None
                    else (
                        previous.returned
                        | previous.returned_exposed
                        | previous.yielded
                        | previous.yielded_exposed
                    )
                )
                added = (
                    result.returned
                    | result.returned_exposed
                    | result.yielded
                    | result.yielded_exposed
                ) - old
                for incidence in tuple(self.value_phases.get(value, ())):
                    if incidence.phase in {"escape", "expose"}:
                        for source in added:
                            self.demand(
                                (module_name, source),
                                "escape",
                                consumer=incidence.consumer,
                            )
        for binding in scan.source_bindings:
            key = (module_name, binding.source)
            previous = self.import_bindings.get(key)
            if previous is not None:
                binding = binding._replace(
                    modules=tuple(dict.fromkeys((*previous.modules, *binding.modules)))
                )
            if not self._publish(self.import_bindings, key, binding):
                continue
            operands = (module_name, SourceCallArguments(binding.source))
            if operands in self.value_candidates:
                self.queue_value(operands)
            if binding.attribute == "*":
                self.star_aliases.setdefault(module_name, set()).add(binding.source)
                for incidence in self.escaped_modules.get(module_name, ()):
                    self.demand(
                        key,
                        incidence.phase,
                        incidence.arguments,
                        consumer=incidence.consumer,
                    )
        for _source, keys, origins, contents in scan.source_values.cache_writes:
            values: set[tuple[SourceValue, bool]] = {
                ((module_name, origin), False) for origin in origins
            } | {((module_name, origin), True) for origin in contents}
            items = (
                [SourceCacheItem(key) for key in keys]
                if keys is not None
                else [SourceCacheItem(None, True)]
            )
            items.append(SourceCacheItem(None))
            for item in items:
                self.hold_cache_origins(item, values)
            # Removing or replacing a literal key can retire values from
            # that entry and computed-key stores. The aggregate is needed
            # only for an operation whose key is unknown or removes all keys.
            for item in _import_cache_items(keys):
                self.demand(("", item), "release", consumer=module_name)
        for (
            source,
            keys,
            defaults,
            default_contents,
            contained,
        ) in scan.source_values.cache_reads:
            value = (module_name, source)
            self._publish(
                self.cache_reads,
                value,
                self.cache_reads.get(value, frozenset())
                | {(keys, defaults, default_contents, contained)},
            )
        for write in scan.source_values.attribute_writes:
            owner = (module_name, write.owner)
            if write.owner is None or owner in self.body_jobs:
                self._watch_attribute_write(module_name, write)
            else:
                self.owner_attribute_writes.setdefault(owner, set()).add(write)
        for request in scan.source_demands:
            if request.owner is not None:
                self.owner_demands.setdefault((module_name, request.owner), set()).add(
                    request
                )
                if (module_name, request.owner) not in self.body_jobs:
                    continue
            self._request(module_name, request)
        for value in self.pending_modules.pop(module_name, ()):
            self.queue_value(value)

    def _watch_attribute_write(self, module: str, write: SourceAttributeWrite) -> None:
        for receiver in write.receivers:
            value = (module, receiver)
            writes = self.attribute_writes.setdefault(value, set())
            if write not in writes:
                writes.add(write)
                self._record_attribute_receivers(
                    module, (write,), self._ensure_value(value)
                )

    def _record_attribute_receivers(
        self,
        writer: str,
        writes: Iterable[SourceAttributeWrite],
        receivers: Iterable[SourceCandidate],
    ) -> None:
        for value in receivers:
            if value.identity is not None or value.contained:
                continue
            if self._source_class_candidate(value):
                prior = self.class_lifecycle_writes.get(
                    value.value, ClassLifecycleWrite.NONE
                )
                reached = prior
                for write in writes:
                    names = (
                        _CLASS_LIFECYCLE_WRITES if write.names is None else write.names
                    )
                    for name in names:
                        if value.kind == "value" or name == "__class__":
                            reached |= _CLASS_LIFECYCLE_WRITES.get(
                                name, ClassLifecycleWrite.NONE
                            )
                if reached != prior:
                    self.class_lifecycle_writes[value.value] = reached
                    for user in self.value_users.get(value.value, ()):
                        if isinstance(user[1], SourceSubclassInvocation):
                            self.queue_value(user)
                continue
            if value.source is not None or value.kind != "value":
                continue
            for write in writes:
                incidence = (writer, value.module, write)
                if incidence not in self.reached_attribute_writes:
                    self.reached_attribute_writes.add(incidence)
                    self.pending_attribute_writes.add(incidence)

    def drain_attribute_writes(self) -> set[tuple[str, str, SourceAttributeWrite]]:
        rows = self.pending_attribute_writes
        self.pending_attribute_writes = set()
        return rows

    def _request(self, module: str, request: _SourceValueDemand) -> None:
        if request.argument_call is not None:
            self.argument_exposures.setdefault(
                (module, request.argument_call), set()
            ).add(request.source)
            return
        self.demand(
            (module, request.source),
            request.phase,
            argument_count=request.argument_count,
            consumer=module,
        )

    def binds_positional_arguments(
        self,
        module: str,
        site: PythonSourceKey,
        provider: str,
        definition: PythonSourceKey,
    ) -> bool:
        """The existing call/formal transfer entered exactly the witnessed definition."""
        target = (provider, definition)
        call = self.calls.get((module, site))
        formals = self.declarations.get(target)
        if (
            call is None
            or formals is None
            or target not in self.body_jobs
            or (module, site) in self.import_bindings
            or len(call.arguments) > len(formals.positional)
            or definition in self._opaque_activation_sources.get(provider, ())
        ):
            return False
        if not self.argument_exposures[module, site].issubset(
            origin for _, _, exposed in call.arguments for origin in exposed
        ):
            return False
        selected = set().union(
            *(
                self.value_candidates.get((module, callee), ())
                for callee in call.callees
            )
        )
        return selected == {SourceCandidate(provider, definition)}

    def resolve_argument_exposures(
        self, closed: Callable[[str, PythonSourceKey], bool]
    ) -> bool:
        """Release fallback demands at graph quiescence and recheck held sites on extension."""
        released = False
        for (module, site), origins in tuple(self.argument_exposures.items()):
            if closed(module, site):
                continue
            for source in origins:
                self.demand((module, source), "opaque_argument", consumer=module)
            del self.argument_exposures[module, site]
            released = True
        return released

    def opaque_activation_sources(
        self, module_name: str
    ) -> tuple[PythonSourceKey, ...]:
        return tuple(sorted(self._opaque_activation_sources.get(module_name, ())))

    def opaque_activation_producers(self) -> Mapping[str, set[str]]:
        """Map the defining module to consumers known to escape its sources."""
        return self._opaque_activation_producers

    def exact_receivers(
        self,
    ) -> dict[str, tuple[tuple[PythonSourceKey, PythonSourceKey], ...]]:
        """Per module, each function whose every entry binds its first
        parameter to an instance one class statement of the module built, as
        (function source, class statement source), sorted (design 78, 3.8:
        exact `self`).

        Every entry means: some selection on an instance bound the receiver
        (`_bind_receiver`), no call entered the function without a receiver,
        no attribute selected it through anything but an instance, and no
        code the analysis cannot follow activates it. Read once the graph is
        closed: the rows only grow less precise as entries are found.
        """
        unbound = {
            member.value
            for members in self.unbound_members.values()
            for member in members
        }
        rows: dict[str, list[tuple[PythonSourceKey, PythonSourceKey]]] = {}
        for function, owners in self._receiver_classes.items():
            module, source = function
            if (
                source in self._opaque_activation_sources.get(module, ())
                or function in self._direct_entries
                or function in unbound
                or len(owners) != 1
            ):
                continue
            (owner,) = owners
            if owner is None or owner[0] != module:
                continue
            rows.setdefault(module, []).append((source, owner[1]))
        return {module: tuple(sorted(found)) for module, found in rows.items()}

    def drain_opaque_activation_changes(self) -> frozenset[str]:
        changed = frozenset(self._opaque_activation_changes)
        self._opaque_activation_changes.clear()
        return changed

    def drain(self) -> None:
        while True:
            while self.queue:
                self.process_value(self.queue.popleft())
            if not self._deferred_receiver_proofs:
                return
            deferred, self._deferred_receiver_proofs = (
                self._deferred_receiver_proofs,
                set(),
            )
            for value in sorted(deferred, key=repr):
                self.queue_value(value)

    @staticmethod
    def _source_class_candidate(candidate: SourceCandidate) -> bool:
        return (
            candidate.kind in {"instance", "value"}
            and not candidate.contained
            and candidate.identity is None
            and candidate.source is not None
            and candidate.source[-1] == "ClassDef"
        )

    def _strict_ancestors(
        self, owner: SourceValue, user: SourceValue
    ) -> list[SourceCandidate]:
        """Every source class statement above `owner`, each once."""
        ancestors: list[SourceCandidate] = []
        seen = {owner}
        pending = [owner]
        while pending:
            current = pending.pop()
            self.watch(current, user)
            for base in sorted(self.bases.get(current, ())):
                for candidate in sorted(
                    self.watch((current[0], base), user), key=_candidate_order
                ):
                    if (
                        candidate.kind == "value"
                        and self._source_class_candidate(candidate)
                        and candidate.value not in seen
                    ):
                        seen.add(candidate.value)
                        ancestors.append(candidate)
                        pending.append(candidate.value)
        return ancestors

    def _receiver_binding_complete(
        self,
        receiver: SourceCandidate,
        name: str,
        user: SourceValue,
        *,
        strict: bool,
    ) -> bool:
        """Whether selecting `name` on `receiver` bound it to every function
        the load can enter (design 78, 3.8).

        The proof needs the whole MRO: each class a plain class statement
        (metaclass `type`, no `__init_subclass__`, every base such a class),
        and each definition of `name` a plain function, a `classmethod`, a
        `staticmethod` or a `property`. A name no class defines is an instance
        attribute, or one of `object`'s methods that hand the receiver to no
        Python code. `strict` selects above the receiver's class (`super()`).
        A class mutation exposes the class, which escapes its instances.
        """
        found = False
        seen: set[SourceValue] = set()
        pending = [receiver.value]
        while pending:
            owner = pending.pop()
            if owner in seen:
                continue
            seen.add(owner)
            self.watch(owner, user)
            if owner[0] not in self.modules:
                self.pending_modules.setdefault(owner[0], set()).add(user)
                return True
            info = self.classes.get(owner)
            if info is None or not info.plain_metaclass:
                return False
            if not (strict and owner == receiver.value):
                namespace = self.namespaces.get(owner, _EMPTY_NAMESPACE).members
                for origin in namespace.get(name, ()):
                    member_value = (owner[0], origin)
                    members = self.watch(member_value, user)
                    if member_value in self.value_jobs:
                        self._deferred_receiver_proofs.add(user)
                        return True
                    if not members:
                        return False
                    for member in members:
                        found = True
                        if member.contained or member.identity is not None:
                            return False
                        if member.kind in {
                            "class_descriptor",
                            "static_descriptor",
                            "property_descriptor",
                        }:
                            continue
                        if (
                            member.kind != "value"
                            or member.source is None
                            or member.source[-1]
                            not in {"FunctionDef", "AsyncFunctionDef", "Lambda"}
                        ):
                            return False
            for base in self.bases.get(owner, ()):
                base_value = (owner[0], base)
                bases = self.watch(base_value, user)
                if base_value in self.value_jobs:
                    self._deferred_receiver_proofs.add(user)
                    return True
                if not bases:
                    return False
                for candidate in bases:
                    if candidate.kind != "value" or not self._source_class_candidate(
                        candidate
                    ):
                        return False
                    pending.append(candidate.value)
        return (
            found
            or not (name.startswith("__") and name.endswith("__"))
            or name in _OBJECT_METHODS_WITHOUT_CALLBACKS
        )

    def _refresh_class_members(
        self, lookup: _ClassMemberLookup, value: SourceValue
    ) -> _ClassMemberResult:
        owner, name = lookup.owner, lookup.name
        property_results: set[SourceCandidate] = set()
        if lookup.strict:
            seen: set[tuple[str, PythonSourceKey | None]] = set()
            members: set[SourceCandidate] = set()
            for ancestor in self._strict_ancestors(owner.value, value):
                members.update(
                    self.select_member(
                        ancestor._replace(kind=owner.kind),
                        name,
                        value,
                        member_seen=seen,
                        binding_owner=owner,
                        property_results=property_results,
                    )
                )
        else:
            members = set()
            providers = (
                self._class_member_providers(owner.value, name, value)
                if self.bases.get(owner.value)
                and name not in {"__class__", "__dict__", "__init__"}
                else None
            )
            # Special metadata and native initialization retain their full
            # selection rules.
            for source in (owner.value,) if providers is None else providers:
                members.update(
                    self.select_member(
                        owner
                        if source == owner.value
                        else SourceCandidate(*source, owner.kind),
                        name,
                        value,
                        binding_owner=owner,
                        property_results=property_results,
                    )
                )
        result = _ClassMemberResult(frozenset(members), frozenset(property_results))
        previous = self._class_members.get(lookup)
        if result != previous:
            self._class_members[lookup] = result
            for user in self.value_users.get(value, ()):
                self.queue_value(user)
            if (
                previous is not None
                and previous.property_results == result.property_results
                and previous.members <= result.members
            ):
                self._add_operand_candidates(value, result.members - previous.members)
            else:
                for user in self.operand_users.get(value, ()):
                    self.queue_value(user)
        return result

    def _invalidate_member_providers(self, value: SourceValue) -> None:
        pending = [value]
        while pending:
            current = pending.pop()
            if self._member_providers.pop(current, None) is None:
                continue
            for user in self.value_users.get(current, ()):
                if isinstance(user[1], _ClassMemberProviders):
                    pending.append(user)
                else:
                    self.queue_value(user)

    def _class_member_providers(
        self, owner: _SourceOrigin, name: str, user: SourceValue
    ) -> frozenset[_SourceOrigin]:
        """Share the source frontier before any descriptor binds a receiver.

        Tarjan's walk closes each mutually recursive group once. An abstract
        base graph can cycle even though actual Python inheritance cannot.
        Every node in the component sees the same terminal providers.
        """

        def key(origin: _SourceOrigin) -> SourceValue:
            return origin[0], _ClassMemberProviders(origin[1], name)

        root = key(owner)
        self.value_users[root].add(user)
        if root in self._member_providers:
            return self._member_providers[root]

        unresolved: dict[SourceValue, _ProviderSearchFrame] = {}
        components: list[_ProviderSearchFrame] = []
        stack: list[_ProviderSearchFrame] = []

        def push(origin: _SourceOrigin, node: SourceValue) -> None:
            module, source = origin
            self.watch(origin, node)
            if (
                source is None
                or source[-1] != "ClassDef"
                or self.namespaces.get(origin, _EMPTY_NAMESPACE).members.get(name)
            ):
                children = iter(())
                providers = frozenset({origin})
            else:
                children = (
                    parent
                    for base in self.bases.get(origin, ())
                    for parent in self._watch_inherited_sources((module, base), node)
                )
                providers = frozenset()
            index = len(components)
            frame = _ProviderSearchFrame(node, children, index, index, providers)
            unresolved[node] = frame
            components.append(frame)
            stack.append(frame)

        push(owner, root)
        while stack:
            frame = stack[-1]
            if (origin := next(frame.children, None)) is not None:
                child = key(origin)
                self.value_users[child].add(frame.node)
                if child in self._member_providers:
                    frame.join(self._member_providers[child])
                elif (unfinished := unresolved.get(child)) is not None:
                    frame.component_start = min(frame.component_start, unfinished.index)
                else:
                    push(origin, child)
                continue

            stack.pop()
            if frame.component_start == frame.index:
                component = components[frame.index :]
                del components[frame.index :]
                for member in component[1:]:
                    frame.join(member.providers)
                for member in component:
                    self._member_providers[member.node] = frame.providers
                    del unresolved[member.node]
                if stack:
                    stack[-1].join(frame.providers)
            else:
                parent = stack[-1]
                parent.component_start = min(
                    parent.component_start, frame.component_start
                )
        return self._member_providers[root]

    def _select_class_members(
        self,
        owner: SourceCandidate,
        name: str,
        user: SourceValue,
        *,
        strict: bool,
        operand: bool = False,
        deltas: _OperandDeltas | None = None,
    ) -> frozenset[SourceCandidate]:
        lookup = _ClassMemberLookup(owner, name, strict)
        value = (owner.module, lookup)
        result = self._class_members.get(lookup)
        if result is None or self.value_jobs.get(value, False):
            # An attribute transfer needs the current snapshot to decide whether
            # its receiver escapes. Leave a queued slot in place when forcing it.
            if value in self.value_jobs:
                self.value_jobs[value] = False
            result = self._refresh_class_members(lookup, value)
        subscribers = self.operand_users if operand else self.value_users
        subscribers[value].add(user)
        if result.property_results:
            self._property_results.setdefault(user, set()).update(
                result.property_results
            )
        if deltas is not None:
            return result.members.intersection(deltas.get(value, ()))
        return result.members

    def _select_attribute(
        self,
        owner_value: SourceValue,
        name: str,
        user: SourceValue,
        deltas: _OperandDeltas | None,
    ) -> set[SourceCandidate]:
        """Select `name` on each candidate of one attribute owner, bind an
        instance receiver, and escape a method receiver it cannot bind."""
        element = self.elements.get(owner_value)
        strict = element is not None and element.protocol is PythonElementProtocol.SUPER
        methods = tuple(
            incidence
            for incidence in self.value_phases.get(user, ())
            if incidence.phase == "method"
        )
        # The attribute is a store target (`x.name = v`, `setattr`).
        stored = any(
            incidence.phase == "mutate" for incidence in self.value_phases.get(user, ())
        )
        selected: set[SourceCandidate] = set()
        binding_changed = False
        for candidate in self._operand_candidates(owner_value, user, deltas):
            source_class = self._source_class_candidate(candidate)
            if source_class:
                members = self._select_class_members(
                    candidate, name, user, strict=strict
                )
            elif not strict:
                members = self.select_member(candidate, name, user)
            else:
                members = frozenset()
            bound = self.receiver_bound_members.setdefault(user, set())
            unbound = self.unbound_members.setdefault(user, set())
            property_results = self._property_results.get(user, ())
            for member in members:
                if (
                    source_class
                    and candidate.kind == "instance"
                    and name not in IMPLICIT_DESCRIPTOR_NAMES
                    and member not in property_results
                    and member.kind == "value"
                    and member.identity is None
                    and not member.contained
                    and member.source is not None
                    and member.source[-1]
                    in {"FunctionDef", "AsyncFunctionDef", "Lambda"}
                ):
                    self.watch(member.value, user)
                    self._bind_receiver(member, candidate)
                    if member not in bound:
                        bound.add(member)
                        binding_changed = True
                elif member not in unbound:
                    unbound.add(member)
                    binding_changed = True
            selected.update(members)
            if source_class and (
                # The instance mapping is a writer no member store names.
                (candidate.kind == "instance" and name == "__dict__")
                # An instance attribute that shadows a class attribute.
                or (
                    stored
                    and candidate.kind == "instance"
                    and (
                        bool(members)
                        or not self._receiver_binding_complete(
                            candidate, name, user, strict=False
                        )
                    )
                )
            ):
                self.activate(candidate, "escape")
            # A modeled mapping method carries the receiver through its
            # selected key or view. An opaque method may expose the mapping.
            if methods and not (
                (candidate.kind == "namespace" and members)
                or (
                    source_class
                    and self._receiver_binding_complete(
                        candidate, name, user, strict=strict
                    )
                )
            ):
                for incidence in methods:
                    self.activate(candidate, "escape", consumer=incidence.consumer)
        # The callable can remain the same while it gains a receiver.
        if binding_changed:
            for dependent in self.binding_users.get(user, ()):
                self.queue_value(dependent)
            for dependent in self._return_projection_users.get(user, ()):
                self.queue_value(dependent)
        return selected

    def _bind_receiver(
        self, callee: SourceCandidate, receiver: SourceCandidate
    ) -> None:
        if callee.source is not None:
            self._receiver_classes.setdefault(
                (callee.module, callee.source), set()
            ).add(
                (receiver.module, receiver.source)
                if receiver.kind == "instance"
                and receiver.source is not None
                and self._source_class_candidate(receiver)
                else None
            )
        parameters = self.declarations.get(callee.value)
        positional = parameters.positional if parameters is not None else ()
        vararg = parameters.vararg if parameters is not None else None
        parameter = positional[0] if positional else vararg
        if parameter is not None:
            formal = (callee.module, parameter)
            argument = receiver._replace(contained=receiver.contained or not positional)
            self._queue_arguments(formal, {argument})

    def select_member(
        self,
        owner,
        name,
        user,
        *,
        from_import=False,
        from_star=False,
        visiting=frozenset(),
        member_seen: set[tuple[str, PythonSourceKey | None]] | None = None,
        binding_owner=None,
        property_results: set[SourceCandidate] | None = None,
    ):
        if owner.kind == "reflective_result" and not owner.contained:
            if name in {"__builtins__", "f_builtins"}:
                return {_INTERPRETER_BUILTINS}
            return {owner}
        if owner.identity is not None:
            if owner.kind == "native_selector" and name == "__call__":
                return {owner}
            if member := NATIVE_ITERATOR_MEMBERS.get((owner.identity, name)):
                return {owner._replace(identity=member)}
            if (
                owner.identity is PythonIdentity.BUILTIN_DICT
                and (mutation := builtin_mapping_write_scope(name)) is not None
            ):
                return {
                    owner._replace(
                        kind="namespace_write_descriptor",
                        identity=None,
                        namespace_mutation=mutation,
                    )
                }
            return set()
        if owner.kind == "iterator":
            if name == "__next__":
                return {owner._replace(kind="iterator_next")}
            if name == "__iter__":
                return {owner._replace(kind="iterator_self")}
            return set()
        owner_module, owner_source = owner.value
        result = set()
        if owner.kind in {"native_initializer", "bound_native_initializer"}:
            if name == "__call__":
                return {owner}
            plan = self.native_initializers.get(owner.value)
            self.watch(owner.value, user)
            if plan is not None:
                for attribute, origins in plan.attributes:
                    if attribute == name:
                        for origin in origins:
                            result.update(
                                candidate._replace(
                                    contained=attribute
                                    not in {"__globals__", "__builtins__"}
                                )
                                for candidate in self.watch(
                                    (owner_module, origin), user
                                )
                            )
            return result
        if owner.kind == "namespace":
            mutation = builtin_mapping_write_scope(name)
            kind: Literal[
                "namespace_get",
                "namespace_remove",
                "namespace_popitem",
                "namespace_clear",
                "namespace_copy_method",
                "namespace_values_method",
                "namespace_items_method",
                "namespace_write_method",
            ]
            if name in {"get", "__getitem__", "setdefault"}:
                kind = "namespace_get"
            elif name == "pop":
                kind = "namespace_remove"
            elif name == "popitem":
                kind = "namespace_popitem"
            elif name in {"clear", "__delitem__"}:
                kind = "namespace_clear"
            elif name == "copy":
                kind = "namespace_copy_method"
            elif name == "values":
                kind = "namespace_values_method"
            elif name == "items":
                kind = "namespace_items_method"
            elif mutation is not None:
                kind = "namespace_write_method"
            else:
                return result
            return {owner._replace(kind=kind, namespace_mutation=mutation)}
        binding_owner = owner if binding_owner is None else binding_owner
        if owner.kind == "property_descriptor":
            if name == "fget":
                return {owner._replace(kind="value")}
            self.demand(owner.value, "expose")
            return result
        if owner.kind in {
            "class_descriptor",
            "static_descriptor",
            "bound_class_method",
        }:
            if name == "__func__" or (
                name == "__wrapped__" and owner.kind != "bound_class_method"
            ):
                return {owner._replace(kind="value", receiver=None)}
            if (
                owner.kind == "bound_class_method"
                and name == "__self__"
                and owner.receiver is not None
            ):
                return set(self.watch(owner.receiver, user))
            self.demand(owner.value, "expose")
            return result
        if owner.contained:
            return result
        if member_seen is not None:
            if owner.value in member_seen:
                return result
            member_seen.add(owner.value)
        if (
            owner.kind == "instance"
            and owner_source is not None
            and owner_source[-1] == "ClassDef"
            and name == "__class__"
        ):
            # `x.__class__` is the class of the statement that built `x`
            # (an assignment to it exposes the class).
            return {owner._replace(kind="value")}
        if owner_source is None and name == "__dict__":
            # A module's data descriptor wins even while its body initializes.
            # Keep the live mapping, rather than invoking PEP 562 or its values.
            return {
                owner._replace(
                    kind="namespace",
                    namespace_ownership=PythonNamespaceOwnership.BORROWED,
                )
            }
        # Fast class metadata reads also depend on later publication.
        self.watch(owner.value, user)
        if (
            owner_source is not None
            and owner_source[-1] == "ClassDef"
            and name == "__dict__"
            and self.classes.get(
                owner.value, PythonSourceClass(owner_source, False, ())
            ).plain_metaclass
        ):
            return {
                owner._replace(
                    kind="namespace",
                    namespace_ownership=PythonNamespaceOwnership.BORROWED,
                )
            }
        # An excluded star member still depends on the export policy. A later
        # writer can widen __all__ without changing the module candidate itself.
        exports = self.exports.get(owner_module)
        if (
            from_star
            and exports is not None
            and (
                exports.status == "known"
                and name not in exports.names
                or exports.status == "absent"
                and name.startswith("_")
            )
        ):
            return result
        namespace = self.namespaces.get(owner.value, _EMPTY_NAMESPACE).members
        if (
            owner_source is not None
            and owner_source[-1] == "ClassDef"
            and name == "__init__"
            and not namespace.get(name)
        ):
            for initializer in self.native_initializers_by_receiver.get(
                owner.value, ()
            ):
                self.watch(initializer, user)
                result.add(
                    SourceCandidate(
                        initializer[0],
                        self.native_initializers[initializer].owner,
                        "bound_native_initializer"
                        if binding_owner.kind == "instance"
                        else "native_initializer",
                        receiver=binding_owner.value,
                    )
                )
        if owner_source is None:
            # Final namespace facts supply candidates, not initialization-order
            # proof. A cycle may read a later export while the hook is installed.
            for origin in namespace.get("__getattr__", ()):
                self.demand((owner_module, origin), "call", (name,))
                result.update(
                    self.watch(
                        (owner_module, SourceReturn((owner_module, origin))), user
                    )
                )
        if namespace.get(name):
            for origin in namespace[name]:
                for candidate in tuple(self.watch((owner_module, origin), user)):
                    if candidate.kind == "class_descriptor":
                        self._bind_receiver(
                            candidate, binding_owner._replace(kind="value")
                        )
                        result.add(
                            candidate._replace(
                                kind="bound_class_method", receiver=binding_owner.value
                            )
                        )
                    elif candidate.kind == "static_descriptor":
                        result.add(candidate._replace(kind="bound_static_method"))
                    elif candidate.kind == "property_descriptor":
                        if binding_owner.kind == "instance":
                            # The load is the getter's call; its activation
                            # comes from the class's instance hooks.
                            returned = self.watch(
                                (
                                    candidate.module,
                                    SourceReturn((candidate.module, candidate.source)),
                                ),
                                user,
                            )
                            if property_results is None:
                                self._property_results.setdefault(user, set()).update(
                                    returned
                                )
                            else:
                                property_results.update(returned)
                            result.update(returned)
                        else:
                            result.add(candidate)
                    else:
                        result.add(candidate)
        elif result:
            pass
        elif owner_source is None:
            child = f"{owner_module}.{name}"
            if from_import:
                self.imported_modules.add(child)
                self.admit_imports(user[0], (child,))
            if child in self.modules or child in self.imported_modules:
                result.update(self.watch((child, None), user))
        elif owner_source[-1] == "ClassDef":
            # Unknown instance/descriptor use is bounded by this class family.
            bases = self.bases.get(owner.value, ())
            if not bases:
                return result
            if member_seen is None:
                # All paths in this query have the same bound receiver and
                # contribute a union. A shared ancestor contributes once.
                member_seen = {owner.value}
            for base in bases:
                for source in self._watch_inherited_sources((owner_module, base), user):
                    if source in member_seen:
                        continue
                    inherited = SourceCandidate(*source, owner.kind)
                    result.update(
                        self.select_member(
                            inherited,
                            name,
                            user,
                            member_seen=member_seen,
                            binding_owner=binding_owner,
                            property_results=property_results,
                        )
                    )
        else:
            self.demand(owner.value, "expose")
        if owner_source is None:
            # No namespace summary spells a star-bound name; select it from each
            # star-imported module. Star-import cycles stop at a visited module.
            visiting = visiting | {owner_module}
            for alias in tuple(self.star_aliases.get(owner_module, ())):
                for candidate in tuple(self.watch((owner_module, alias), user)):
                    if candidate[0] not in visiting:
                        result.update(
                            self.select_member(
                                candidate,
                                name,
                                user,
                                from_star=True,
                                visiting=visiting,
                                property_results=property_results,
                            )
                        )
        return result

    def _namespace_values(
        self,
        owner: SourceCandidate,
        user: SourceValue,
        key: str | None = None,
        visiting=frozenset(),
    ) -> set[SourceCandidate]:
        if owner.value in visiting:
            return set()
        visiting = visiting | {owner.value}
        self.watch(owner.value, user)
        namespace = self.namespaces.get(owner.value, _EMPTY_NAMESPACE)
        sources = namespace.origins(key)
        result = set()
        if owner.value == _INTERPRETER_BUILTINS.value and key in {None, "__import__"}:
            result.add(_INTERPRETER_IMPORT)
        for source in sources:
            result.update(self.watch((owner.module, source), user))
        if (
            owner.source is not None
            and owner.source[-1] == "ClassDef"
            and (key is None or key == "__init__")
            and not namespace.members.get("__init__")
        ):
            result.update(
                self.select_member(owner._replace(kind="value"), "__init__", user)
            )
        if owner.source is None:
            for alias in self.star_aliases.get(owner.module, ()):
                for candidate in tuple(self.watch((owner.module, alias), user)):
                    if key is not None:
                        result.update(
                            self.select_member(candidate, key, user, from_star=True)
                        )
                    else:
                        exports = self.exports.get(candidate.module)
                        if exports is not None and exports.status == "known":
                            for name in exports.names:
                                result.update(
                                    self.select_member(
                                        candidate, name, user, from_star=True
                                    )
                                )
                        else:
                            result.update(
                                self._namespace_values(
                                    candidate, user, visiting=visiting
                                )
                            )
        return result

    def activate(
        self,
        origin: SourceCandidate,
        phase: SourceDemandPhase,
        arguments: tuple[str, ...] = (),
        *,
        argument_count: int | None = None,
        consumer: str | None = None,
    ) -> None:
        consumer = consumer or self._consumer or origin.module
        if phase in {"read", "factory"}:
            if (
                origin.identity is None
                and origin.source is not None
                and origin.source[-1]
                in {"FunctionDef", "AsyncFunctionDef", "Lambda", "GeneratorExp"}
            ):
                self.read_callables.setdefault(origin.module, set()).add(origin.source)
            return
        demand = SourceDemand(phase, arguments, argument_count, consumer)
        if origin.kind in {"native_initializer", "bound_native_initializer"}:
            native = self.native_activations.setdefault(origin.value, set())
            incidence = (origin, demand)
            if incidence in native:
                return
            native.add(incidence)
        elif phase in {"escape", "expose"} and (
            origin.kind == "namespace" or origin.namespace_mutation is not None
        ):
            incidence = (origin, demand)
            if incidence in self._site_sensitive_activated:
                return
            self._site_sensitive_activated.add(incidence)
        else:
            activated = self.activated.get(demand)
            if activated is None:
                self.activated[demand] = {origin}
            elif origin in activated:
                return
            else:
                activated.add(origin)
        if (
            phase in {"escape", "expose"}
            and origin.identity is None
            and origin.source is not None
            and origin.source[-1]
            in {
                "FunctionDef",
                "AsyncFunctionDef",
                "Lambda",
                "GeneratorExp",
                "TypeAlias",
                "ClassDef",
            }
        ):
            if consumer != origin.module:
                self._opaque_activation_producers.setdefault(origin.module, set()).add(
                    consumer
                )
            known = self._opaque_activation_sources.setdefault(origin.module, set())
            if origin.source not in known:
                known.add(origin.source)
                self._opaque_activation_changes.add(origin.module)
        previous = self._consumer
        self._consumer = consumer
        try:
            self._activate(origin, phase, arguments, argument_count)
        finally:
            self._consumer = previous

    def _activate(
        self,
        origin: SourceCandidate,
        phase: SourceDemandPhase,
        arguments: tuple[str, ...],
        argument_count: int | None,
    ) -> None:
        module_name, owner = origin.value
        if phase == "method":
            # The attribute projection owns the receiver obligation.
            return
        if origin.kind.startswith("namespace"):
            if phase in {"escape", "expose"}:
                # Retained mappings remain live as their owners publish values.
                self.demand(origin.value, phase)
                if origin.value == _INTERPRETER_BUILTINS.value:
                    self.activate(_INTERPRETER_IMPORT, phase)
            elif (
                phase == "call"
                and origin.namespace_mutation is not None
                and origin.kind not in {"namespace_get", "namespace_write_descriptor"}
            ):
                self.demand(origin.value, "release_contents")
            elif (
                phase == "release"
                and origin.namespace_ownership is PythonNamespaceOwnership.OWNED
            ):
                self.demand(origin.value, "release_contents")
            return
        if origin.kind in {"native_initializer", "bound_native_initializer"}:
            if phase in {"call", "escape", "expose"}:
                self._activate_native_initializer(origin)
            return
        if phase == "release_contents":
            self.demand(origin.value, phase)
            for candidate in self._namespace_values(origin, origin.value):
                self.activate(candidate, "release")
            return
        if phase == "mutate":
            # A member store may change a source callable's execution target
            # (__code__, defaults, __init__, ...). Resolve the actual receiver
            # through ordinary source/formal provenance before exposing it.
            # Mutating a module member does not expose its entire namespace.
            if (
                not origin.contained
                and owner is not None
                and owner[-1]
                in {"FunctionDef", "AsyncFunctionDef", "Lambda", "ClassDef"}
            ):
                self.activate(
                    origin,
                    "descriptor" if origin.kind == "instance" else "expose",
                    arguments,
                    argument_count=argument_count,
                )
            return
        if origin.kind == "property_descriptor":
            # A property object runs its getter only through its `__get__`
            # hook; calling the object itself raises TypeError. Any other
            # use hands the getter to that use.
            if phase == "call":
                return
            self.activate(
                origin._replace(kind="value"),
                "call" if phase == "descriptor" else phase,
                arguments,
            )
            return
        if origin.kind in {"class_descriptor", "static_descriptor"}:
            if phase == "call" and origin.kind == "class_descriptor":
                return
            self.activate(
                origin._replace(kind="value"),
                phase,
                arguments,
                argument_count=argument_count,
            )
            return
        if (
            origin.kind == "bound_class_method"
            and phase in {"escape", "expose"}
            and origin.receiver is not None
        ):
            self.demand(origin.receiver, "expose")
        if origin.contained:
            if phase in {
                "escape",
                "release",
                "expose",
                "protocol",
                "truth",
                "descriptor",
            }:
                self.activate(origin._replace(contained=False), phase, arguments)
            return
        if origin.identity is not None:
            if phase in {
                "call",
                "escape",
                "expose",
            } and possible_runtime_import_call_identities(
                int(origin.identity), argument_count if phase == "call" else None
            ):
                assert self._consumer is not None
                self.require_runtime_import_protocol(self._consumer)
            return
        if phase in {
            "context_enter",
            "context_exit",
            "context_unpack",
            "async_context_enter",
            "async_context_exit",
            "async_context_unpack",
        }:
            method, awaited, unpack = context_execution_protocol(phase)
            member = (module_name, SourceMember(origin, method))
            returned = (module_name, SourceReturn(member))
            self.demand(member, "call")
            if awaited:
                self.demand(returned, "await")
            if unpack:
                self.demand(
                    (module_name, SourceAwait(returned)) if awaited else returned,
                    "iterate",
                )
            return
        if origin.kind in {"iterator", "iterator_next", "iterator_self"}:
            assert owner is not None
            escapes = phase in {"escape", "expose"}
            advances = (
                origin.kind == "iterator"
                and phase in {"iterate", "next"}
                or origin.kind == "iterator_next"
                and phase == "call"
            )
            if escapes or advances:
                self.demand(
                    self._iterator_contents(origin), "escape" if escapes else "read"
                )
            return
        if phase in {"iterate", "async_iterate", "get_iter", "get_async_iter"}:
            asynchronous = phase in {"async_iterate", "get_async_iter"}
            advances = phase in {"iterate", "async_iterate"}
            if origin.kind == "resumable":
                factory = self._function_kind(origin)
                expected = (
                    FunctionKind.ASYNC_GENERATOR
                    if asynchronous
                    else FunctionKind.GENERATOR
                )
                if advances and factory == expected:
                    self.activate(origin, "resume", arguments)
            elif origin.kind == "instance":
                method = "__aiter__" if asynchronous else "__iter__"
                member = (module_name, SourceMember(origin, method))
                self.demand(member, "call")
                if advances:
                    self.demand(
                        (module_name, SourceReturn(member)),
                        "async_next" if asynchronous else "next",
                    )
                    if not asynchronous:
                        self.demand(
                            (module_name, SourceMember(origin, "__getitem__")), "call"
                        )
            return
        if phase in {"next", "async_next"}:
            asynchronous = phase == "async_next"
            if origin.kind == "resumable":
                expected = (
                    FunctionKind.ASYNC_GENERATOR
                    if asynchronous
                    else FunctionKind.GENERATOR
                )
                if self._function_kind(origin) == expected:
                    self.activate(origin, "resume", arguments)
            elif origin.kind == "instance":
                member = (
                    module_name,
                    SourceMember(origin, "__anext__" if asynchronous else "__next__"),
                )
                self.demand(member, "call")
                if asynchronous:
                    self.demand((module_name, SourceReturn(member)), "await")
            return
        if phase == "await":
            if origin.kind == "resumable":
                if self._function_kind(origin) == FunctionKind.ASYNC:
                    self.activate(origin, "resume", arguments)
            elif origin.kind == "instance":
                member = (module_name, SourceMember(origin, "__await__"))
                self.demand(member, "call")
                self.demand((module_name, SourceReturn(member)), "next")
            return
        if phase == "init_subclass":
            if origin.kind == "value" and self._source_class_candidate(origin):
                self.demand((module_name, SourceSubclassInvocation(origin)), "read")
            return
        if phase == "class_subscription":
            if origin.kind == "value" and self._source_class_candidate(origin):
                # Class subscription uses type.__getitem__ or the class's
                # inherited __class_getitem__. Class creation already exposes
                # custom metaclasses through its preparation demands.
                self.demand(
                    (module_name, SourceMember(origin, "__class_getitem__")),
                    "escape",
                    arguments,
                )
            return
        if phase == "descriptor":
            if origin.kind == "instance":
                # Keep the phase on the canonical class value so namespace
                # publications wake only their newly added descriptor origins.
                self.demand(origin.value, phase, arguments)
                for method in (
                    "__getattribute__",
                    "__getattr__",
                    "__setattr__",
                    "__delattr__",
                    "__get__",
                    "__set__",
                    "__delete__",
                ):
                    self.demand((module_name, SourceMember(origin, method)), "escape")
            if owner is not None and owner[-1] == "ClassDef":
                # Descriptor-valued attributes can execute their own hooks;
                # ordinary methods stored here are values, not implicit calls.
                namespace = self.namespaces.get(origin.value, _EMPTY_NAMESPACE)
                for source in namespace.origins() | self.bases.get(
                    origin.value, frozenset()
                ):
                    self.demand((module_name, source), phase, arguments)
            return
        if phase in {"protocol", "truth"}:
            if origin.kind != "instance":
                return
            if owner is not None and owner[-1] == "ClassDef":
                # Use the receiver's inherited slots and preserve descriptor
                # effects. A class body can also publish an unnamed slot.
                self.activate(origin, "descriptor", arguments)
                methods = (
                    ("__bool__", "__len__")
                    if phase == "truth"
                    else sorted(IMPLICIT_INSTANCE_SPECIAL_METHODS)
                )
                for method in methods:
                    # Without a proof of slot presence at the use, either
                    # truth slot may run. Escape also retains effects on the
                    # result, such as length conversion through __index__.
                    self.demand((module_name, SourceMember(origin, method)), "escape")
                namespace = self.namespaces.get(origin.value, _EMPTY_NAMESPACE)
                for value in sorted(namespace.exposure - namespace.named_origins):
                    self.demand((module_name, value), "escape")
                return
            phase = "protocol"
        if phase == "release":
            if origin.kind == "instance":
                self.demand((module_name, SourceMember(origin, "__del__")), "call")
            return
        if phase == "call" and (
            origin.kind == "resumable"
            or self._function_kind(origin) != FunctionKind.SYNC
        ):
            # Calling a generator/coroutine factory creates a suspended value;
            # only resume/escape demand executes that value's body.
            return
        if module_name not in self.modules:
            return
        if owner is None or owner[-1] in {"Module", "ClassDef"}:
            namespace = self.namespaces.get(origin.value, _EMPTY_NAMESPACE)
            if owner is not None and owner[-1] == "ClassDef" and phase == "call":
                names = (
                    ("__call__",)
                    if origin.kind == "instance"
                    else ("__new__", "__init__")
                )
                for name in names:
                    self.demand(
                        (module_name, SourceMember(origin, name)),
                        "call",
                    )
                return
            namespace_phase: SourceDemandPhase = (
                "escape" if phase in {"escape", "protocol"} else "expose"
            )
            self.demand(origin.value, namespace_phase, arguments)
            for value in namespace.origins() | self.bases.get(
                origin.value, frozenset()
            ):
                self.demand((module_name, value), namespace_phase)
            if owner is None:
                # An escaped namespace exposes its star-bound names as well.
                self.escaped_modules.setdefault(module_name, set()).add(
                    SourceDemand(
                        "escape" if phase in {"escape", "protocol"} else "expose",
                        arguments,
                        None,
                        self._consumer or module_name,
                    )
                )
                for alias in tuple(self.star_aliases.get(module_name, ())):
                    self.demand(
                        (module_name, alias),
                        "escape" if phase in {"escape", "protocol"} else "expose",
                    )
        elif owner[-1] in {
            "FunctionDef",
            "AsyncFunctionDef",
            "Lambda",
            "TypeAlias",
            "GeneratorExp",
        }:
            if phase in {"escape", "expose"}:
                # A returned/yielded source value is availability until an
                # actual consumer uses it. Opaque consumers of the callable
                # may use its results too; propagate that escape explicitly.
                self.demand(origin.value, phase, arguments)
                result = self.functions.get(origin.value)
                if result is not None:
                    for source in (
                        result.returned
                        | result.returned_exposed
                        | result.yielded
                        | result.yielded_exposed
                    ):
                        self.demand((module_name, source), "escape")
            self.body_activations.setdefault(origin.value, set()).add(arguments)
            self._schedule_body(origin.value, arguments)

    def _schedule_body(self, value: SourceValue, arguments: tuple[str, ...]) -> None:
        """Scan a body once per argument class (``python_parameter_classes``)."""
        module_name, owner = value
        if (classes := self.activation_partitions.get(value)) is not None:
            arguments = representative_arguments(arguments, classes)
        body_arguments = self.body_jobs.setdefault(value, set())
        if arguments in body_arguments:
            return
        body_arguments.add(arguments)
        for request in self.owner_demands.get((module_name, owner), ()):
            self._request(module_name, request)
        for write in self.owner_attribute_writes.pop(value, ()):
            self._watch_attribute_write(module_name, write)
        self.queue_body(module_name, cast(PythonSourceKey, owner), arguments)

    def _record_namespace_write(
        self,
        module: str,
        site: PythonSourceKey,
        receiver: SourceCandidate,
        key: str | None,
    ) -> None:
        if (
            receiver.source is None
            and receiver.namespace_ownership is PythonNamespaceOwnership.BORROWED
            and not receiver.contained
        ):
            self.namespace_writes.setdefault(module, set()).add(
                PythonNamespaceWrite(
                    receiver.module, key, "namespace", site[0], source_site=site
                )
            )

    def _record_mapping_call(
        self, value: SourceValue, call: PythonSourceCall, method: SourceCandidate
    ) -> None:
        scope = method.namespace_mutation
        if scope is None:
            return
        module = value[0]
        unbound = method.kind == "namespace_write_descriptor"
        key_position = int(unbound)
        keys: tuple[str | None, ...] = (None,)
        if scope is MappingWriteScope.UPDATE:
            update = call.unbound_update_keys if unbound else call.bound_update_keys
            if update is not None:
                keys = update
        elif (
            scope is MappingWriteScope.KEY
            and len(call.argument_strings) > key_position
            and call.arguments[key_position][0] == key_position
        ):
            keys = (call.argument_strings[key_position],)
        if not unbound:
            for key in keys:
                self._record_namespace_write(module, call.owner, method, key)
            return
        for receiver in self._positional_candidates(call, value, 0):
            if receiver.kind != "namespace":
                continue
            for key in keys:
                self._record_namespace_write(module, call.owner, receiver, key)
            if keys:
                self.demand(receiver.value, "release_contents", consumer=module)

    def _positional_candidates(
        self,
        call: PythonSourceCall,
        user: SourceValue,
        position: int | None,
        *,
        include_contents: bool = False,
        keyword: str | None = None,
    ) -> set[SourceCandidate]:
        selected: set[SourceCandidate] = set()
        for argument in call.arguments:
            name = argument[0]
            if (
                name is None
                or name == position
                or name == keyword
                or (position is None and isinstance(name, int))
            ):
                selected.update(
                    self._argument_candidates(argument, user, include_contents)
                )
        return selected

    def _argument_candidates(
        self,
        argument: PythonSourceArgument,
        user: SourceValue,
        include_contents: bool,
        *,
        module: str | None = None,
    ) -> set[SourceCandidate]:
        name, direct, exposed = argument
        defining_module = module or user[0]
        selected: set[SourceCandidate] = set()
        for origin in exposed if include_contents or name is None else direct:
            for candidate in self.watch((defining_module, origin), user):
                # An expansion can move a contained value to an actual
                # argument. Until its arity is known, keep that alternative.
                contained = name is not None and (
                    candidate.contained or origin not in direct
                )
                selected.add(candidate._replace(contained=contained))
                if name is None and include_contents:
                    selected.add(candidate._replace(contained=True))
        return selected

    def _activate_demand(
        self, value: SourceValue, candidate: SourceCandidate, demand: SourceDemand
    ) -> None:
        module, source = value
        if isinstance(source, SourceIteratorItems):
            source = source.site
        phase = demand.phase
        if phase == "opaque_argument":
            if (
                type(source) is tuple
                and len(source) == 5
                and candidate.kind
                in {
                    "native_initializer",
                    "bound_native_initializer",
                    "reflective_result",
                }
            ):
                self._record_namespace_write(
                    module, source, _INTERPRETER_BUILTINS, None
                )
            # The ordinary exposure transfer owns the body and retained values.
            phase = "expose"
        if (
            phase in {"escape", "expose"}
            and (
                candidate.kind == "namespace"
                or candidate.namespace_mutation is not None
            )
            and type(source) is tuple
            and len(source) == 5
        ):
            # The escaped capability can be exercised by code without a bound
            # call site. Use the publication site, without inventing a key.
            self._record_namespace_write(
                module, source, candidate._replace(contained=False), None
            )
        self.activate(
            candidate,
            phase,
            demand.arguments,
            argument_count=demand.argument_count,
            consumer=demand.consumer,
        )

    def process_value(self, value: SourceValue) -> None:
        refresh = self.value_jobs.pop(value)
        deltas = self.pending_operands.pop(value, None)
        if refresh:
            deltas = None
        module_name, source = value
        assert not isinstance(source, _ClassMemberProviders), (
            "provider invalidation never enters the value queue"
        )
        if isinstance(source, _ClassMemberLookup):
            if refresh:
                self._refresh_class_members(source, value)
            return
        outcomes_pending = value in self.pending_call_outcomes
        if outcomes_pending:
            self.pending_call_outcomes.remove(value)
        if not refresh and deltas is None and not outcomes_pending:
            self._activate_pending(
                value, self.value_candidates[value], _EMPTY_CANDIDATES
            )
            return
        if module_name not in self.modules and not isinstance(source, SourceCacheItem):
            self.pending_modules.setdefault(module_name, set()).add(value)
            return
        if refresh:
            for origin in self.call_outcome_inputs.pop(value, ()):
                self.call_outcome_users[origin].discard(value)
        elif outcomes_pending:
            assert deltas is None
            outcomes = self.call_outcome_inputs[value]
            # Each producer already owns the current answer. Coalesce wakeups
            # without copying candidate waves into every caller's inbox.
            candidates = (
                self.value_candidates[next(iter(outcomes))]
                if len(outcomes) == 1
                else set().union(
                    *(self.value_candidates[origin] for origin in outcomes)
                )
            )
            self._publish_candidates(value, candidates)
            return
        candidates: set[SourceCandidate] = set()
        if isinstance(source, tuple) and (identities := self.callables.get(value)):
            candidates.update(
                SourceCandidate(module_name, source, identity=identity)
                for identity in PythonIdentity(identities)
            )
        if isinstance(source, SourceCacheItem):
            for origin, contained in tuple(self.cache_origins.get(source, ())):
                candidates.update(
                    item._replace(contained=item.contained or contained)
                    for item in self._operand_candidates(origin, value, deltas)
                )
        elif value in self.cache_reads:
            for keys, defaults, default_contents, contained in self.cache_reads[value]:
                for item in _import_cache_items(keys):
                    candidates.update(
                        item._replace(contained=contained or item.contained)
                        for item in self._operand_candidates(("", item), value, deltas)
                    )
                for default in defaults:
                    candidates.update(
                        self._operand_candidates((module_name, default), value, deltas)
                    )
                for default in default_contents:
                    candidates.update(
                        item._replace(contained=True)
                        for item in self._operand_candidates(
                            (module_name, default), value, deltas
                        )
                    )
        elif isinstance(source, SourceCallableOutcome):
            function = self.functions.get((module_name, source.owner))
            if function is not None:
                direct, exposed = (
                    (function.yielded, function.yielded_exposed)
                    if source.yielded
                    else (function.returned, function.returned_exposed)
                )
                for origin in direct:
                    candidates.update(
                        self._operand_candidates((module_name, origin), value, deltas)
                    )
                for origin in exposed - direct:
                    candidates.update(
                        candidate._replace(contained=True)
                        for candidate in self._operand_candidates(
                            (module_name, origin), value, deltas
                        )
                    )
        elif isinstance(source, SourceCallArguments):
            self._route_call_arguments(module_name, source, value, deltas)
        elif isinstance(source, SourceIteratorItems):
            candidates.update(self._iterator_items(module_name, source, value))
        elif isinstance(source, SourceReturn):
            for callee in self._operand_candidates(source.callee, value, deltas):
                candidates.update(self._returned_values(callee, value))
        elif isinstance(source, SourceAwait):
            for awaited in self._operand_candidates(source.value, value, deltas):
                candidates.update(self._awaited_values(awaited, value))
        elif isinstance(source, SourceGeneratorCompletion):
            for generator in self._operand_candidates(source.value, value, deltas):
                candidates.update(self._generator_completion_values(generator, value))
        elif isinstance(source, SourceSubclassInvocation):
            self._call_init_subclass(source.owner, value)
        elif isinstance(source, SourceMember):
            if self._source_class_candidate(source.owner):
                candidates.update(
                    self._select_class_members(
                        source.owner,
                        source.name,
                        value,
                        strict=False,
                        operand=True,
                        deltas=deltas,
                    )
                )
            else:
                candidates.update(self.select_member(source.owner, source.name, value))
            if source.owner.kind == "instance":
                for callee in candidates:
                    self.watch(callee.value, value)
                    if callee.kind not in {"bound_class_method", "bound_static_method"}:
                        self._bind_receiver(callee, source.owner)
        elif source is not None and source[-1] == "arg":
            candidates.update(self.pending_arguments.pop(value, ()))
        elif source is None or source[-1] == "Module":
            candidates.add(SourceCandidate(module_name, None))
        elif source[-1] == PythonSourceProjection.CALL_NATIVE_BUILTINS:
            candidates.add(_INTERPRETER_BUILTINS)
        elif value in self.native_initializers:
            candidates.add(SourceCandidate(module_name, source, "native_initializer"))
            plan = self.native_initializers[value]
            for receiver_source in plan.receiver:
                for receiver in tuple(
                    self.watch((module_name, receiver_source), value)
                ):
                    if (
                        receiver.source is not None
                        and receiver.source[-1] == "ClassDef"
                    ):
                        entries = self.native_initializers_by_receiver.setdefault(
                            receiver.value, set()
                        )
                        if value not in entries:
                            entries.add(value)
                            self._invalidate_value(receiver.value)
            # Member aliases retain receiver and consumer contexts. A late
            # publication replays the actual incidences, even if the alias's
            # candidate set did not change.
            for candidate, (phase, args, count, consumer) in tuple(
                self.native_activations.get(value, ())
            ):
                if phase in {"call", "escape", "expose"}:
                    previous_consumer = self._consumer
                    self._consumer = consumer
                    try:
                        self._activate(candidate, phase, args, count)
                    finally:
                        self._consumer = previous_consumer
        elif source[-1] in {
            "FunctionDef",
            "AsyncFunctionDef",
            "Lambda",
            "GeneratorExp",
        }:
            # A forward value demand can precede its producer's scan. Publish
            # the candidate only when its calling protocol is known.
            if value in self.declarations:
                candidates.add(
                    SourceCandidate(
                        module_name,
                        source,
                        "resumable" if source[-1] == "GeneratorExp" else "value",
                    )
                )
        elif source[-1] in {"ClassDef", "TypeAlias"}:
            candidates.add(SourceCandidate(module_name, source))
        elif value in self.elements:
            element = self.elements[value]
            for owner in element.origins:
                for candidate in tuple(self.watch((module_name, owner), value)):
                    if element.protocol in {
                        PythonElementProtocol.CLASS_METHOD,
                        PythonElementProtocol.STATIC_METHOD,
                    }:
                        candidates.add(
                            candidate._replace(
                                kind="class_descriptor"
                                if element.protocol
                                == PythonElementProtocol.CLASS_METHOD
                                else "static_descriptor"
                            )
                        )
                    elif element.protocol is PythonElementProtocol.PROPERTY:
                        candidates.add(candidate._replace(kind="property_descriptor"))
                    elif element.protocol is PythonElementProtocol.SUPER:
                        # The proxy stands for its receiver in every phase;
                        # an attribute of it selects above the receiver's class.
                        if not candidate.contained:
                            candidates.add(candidate)
                    elif element.protocol in {
                        PythonElementProtocol.MAPPING_WRITE,
                        PythonElementProtocol.ENSURE_BUILTINS,
                    }:
                        if candidate.kind != "namespace":
                            continue
                        if (
                            element.protocol is PythonElementProtocol.ENSURE_BUILTINS
                            and candidate.module in self.modules
                            and candidate != _INTERPRETER_BUILTINS
                        ):
                            # Source publication prepares __builtins__ before
                            # exposing this exact namespace. Any later removal
                            # already invalidates that key through its own write.
                            continue
                        key = (
                            "__builtins__"
                            if element.protocol is PythonElementProtocol.ENSURE_BUILTINS
                            else element.key
                        )
                        self._record_namespace_write(
                            module_name, element.owner, candidate, key
                        )
                    elif element.protocol is PythonElementProtocol.REFLECTED_GLOBALS:
                        candidates.add(
                            candidate._replace(
                                kind="namespace",
                                namespace_ownership=PythonNamespaceOwnership.BORROWED,
                            )
                        )
                    elif element.protocol is PythonElementProtocol.REFLECTED_BUILTINS:
                        if (
                            candidate.source is None
                            and candidate.identity is None
                            and candidate.kind == "value"
                            and not candidate.contained
                        ):
                            # With no finite name, a module lookup can select
                            # any export or call __getattr__ with an unknown key.
                            self.activate(candidate, "expose", consumer=module_name)
                        elif (
                            candidate.identity is PythonIdentity.CURRENT_FRAME
                            and not candidate.contained
                        ):
                            candidates.add(_INTERPRETER_BUILTINS)
                        elif not candidate.contained and (
                            candidate.kind
                            in {
                                "native_initializer",
                                "bound_native_initializer",
                                "reflective_result",
                            }
                            or candidate.value in self.declarations
                        ):
                            candidates.add(_INTERPRETER_BUILTINS)
                            candidates.update(
                                self.select_member(candidate, "__builtins__", value)
                            )
                    elif element.protocol is PythonElementProtocol.SUBSCRIPT:
                        candidates.update(
                            self._subscript_values(candidate, value, element.key)
                        )
                    elif candidate.contained:
                        # Exposed provenance is transitive may-containment, not
                        # an exact container depth. A selector can yield the
                        # value itself or another container still holding it.
                        candidates.add(candidate)
                        candidates.add(candidate._replace(contained=False))
                    else:
                        match element.protocol:
                            case (
                                PythonElementProtocol.ITERATE
                                | PythonElementProtocol.ASYNC_ITERATE
                            ):
                                candidates.update(
                                    self._iterated_values(
                                        candidate,
                                        value,
                                        asynchronous=element.protocol
                                        == PythonElementProtocol.ASYNC_ITERATE,
                                    )
                                )
                            case (
                                PythonElementProtocol.CLASS_METHOD
                                | PythonElementProtocol.STATIC_METHOD
                                | PythonElementProtocol.PROPERTY
                                | PythonElementProtocol.SUPER
                                | PythonElementProtocol.SUBSCRIPT
                                | PythonElementProtocol.REFLECTED_BUILTINS
                                | PythonElementProtocol.REFLECTED_GLOBALS
                                | PythonElementProtocol.MAPPING_WRITE
                                | PythonElementProtocol.ENSURE_BUILTINS
                            ):
                                raise AssertionError(
                                    "descriptor construction is handled before selection"
                                )
                            case _ as impossible:
                                assert_never(impossible)
        elif value in self.completions:
            for owner in self.completions[value]:
                operation = (
                    SourceAwait if source[-1] == "Await" else SourceGeneratorCompletion
                )
                candidates.update(
                    self._operand_candidates(
                        (module_name, operation((module_name, owner))), value, deltas
                    )
                )
        elif value in self.calls and value not in self.import_bindings:
            call = self.calls[value]
            if any(exposed for _, _, exposed in call.arguments):
                self._ensure_value((module_name, SourceCallArguments(call.owner)))
            for callee in call.callees:
                for candidate in tuple(self.watch((module_name, callee), value)):
                    if candidate.contained:
                        continue
                    if candidate.kind.startswith("namespace"):
                        self._record_mapping_call(value, call, candidate)
                        candidates.update(
                            self._returned_values(
                                candidate, value, forward_outcome=True
                            )
                        )
                        continue
                    if candidate.identity is not None:
                        if candidate.kind == "native_selector":
                            if self._positional_arities(call)[0]:
                                candidates.update(
                                    self._native_selector_values(
                                        candidate,
                                        self._positional_candidates(
                                            call, value, 0, include_contents=True
                                        ),
                                        value,
                                    )
                                )
                        elif candidate.identity in NATIVE_SELECTOR_CONSTRUCTORS:
                            if call.arguments and (
                                candidate.identity
                                is PythonIdentity.OPERATOR_METHODCALLER
                                or all(
                                    not isinstance(name, str)
                                    for name, _, _ in call.arguments
                                )
                            ):
                                candidates.add(
                                    SourceCandidate(
                                        module_name,
                                        call.owner,
                                        "native_selector",
                                        identity=candidate.identity,
                                    )
                                )
                        if candidate.identity in SOURCE_ITERATION_CALLABLES:
                            candidates.update(
                                self._iteration_call(candidate.identity, call, value)
                            )
                        if result_identity := INTRINSIC_RESULT_IDENTITIES.get(
                            candidate.identity
                        ):
                            candidates.add(
                                SourceCandidate(
                                    module_name, call.owner, identity=result_identity
                                )
                            )
                        argument_count = (
                            None
                            if any(name is None for name, _, _ in call.arguments)
                            else len(call.arguments)
                        )
                        import_identity = possible_runtime_import_call_identities(
                            int(candidate.identity), argument_count
                        )
                        for transfer in self.call_imports.get(value, ()):
                            if not import_identity & transfer.identities:
                                continue
                            added_imports = tuple(
                                imported
                                for imported in transfer.site.plan.modules
                                if (value, candidate.identity, imported)
                                not in self.admitted_call_imports
                            )
                            if added_imports:
                                self.admitted_call_imports.update(
                                    (value, candidate.identity, imported)
                                    for imported in added_imports
                                )
                                self.admit_imports(module_name, added_imports)
                            for imported in transfer.returned_modules:
                                candidates.update(self.watch((imported, None), value))
                            for source in transfer.returned_exposed:
                                candidates.update(
                                    result._replace(contained=True)
                                    if source not in transfer.returned_sources
                                    else result
                                    for result in self.watch(
                                        (module_name, source), value
                                    )
                                )
                        if int(candidate.identity) & INTRINSIC_ACQUISITION_IDENTITIES:
                            candidates.update(
                                SourceCandidate(
                                    module_name, call.owner, identity=identity
                                )
                                for identity in PythonIdentity(
                                    call.intrinsic_result & SOURCE_CALLABLE_IDENTITIES
                                )
                            )
                        continue
                    if candidate.source is None:
                        continue
                    attribute = (module_name, callee)
                    receiver_bound = candidate in self.receiver_bound_members.get(
                        attribute, ()
                    )
                    if not receiver_bound or candidate in self.unbound_members.get(
                        attribute, ()
                    ):
                        self._direct_entries.add(candidate.value)
                    if candidate.kind in {
                        "native_initializer",
                        "bound_native_initializer",
                    }:
                        self._bind_native_call(value, candidate)
                    if candidate.source[-1] == "ClassDef":
                        if candidate.kind == "value":
                            for initializer in self._select_class_members(
                                candidate._replace(kind="instance"),
                                "__init__",
                                value,
                                strict=False,
                            ):
                                if initializer.kind in {
                                    "native_initializer",
                                    "bound_native_initializer",
                                }:
                                    self._bind_native_call(value, initializer)
                            candidates.add(
                                SourceCandidate(*candidate.value, "instance")
                            )
                            for member in self._select_class_members(
                                candidate, "__new__", value, strict=False
                            ):
                                candidates.update(
                                    self._returned_values(
                                        member, value, forward_outcome=True
                                    )
                                )
                        elif candidate.kind == "instance":
                            for member in self._select_class_members(
                                candidate, "__call__", value, strict=False
                            ):
                                candidates.update(
                                    self._returned_values(
                                        member, value, forward_outcome=True
                                    )
                                )
                        else:
                            for member in self.select_member(
                                candidate, "__call__", value
                            ):
                                candidates.update(
                                    self._returned_values(
                                        member, value, forward_outcome=True
                                    )
                                )
                    else:
                        candidates.update(
                            self._returned_values(
                                candidate,
                                value,
                                forward_outcome=True,
                                selection=attribute,
                            )
                        )
        elif value in self.attributes:
            for name, owners in self.attributes[value].items():
                if name in {"__builtins__", "f_builtins"}:
                    # Native metadata can be visible without a source callable.
                    candidates.add(_INTERPRETER_BUILTINS)
                for owner in sorted(owners):
                    candidates.update(
                        self._select_attribute(
                            (module_name, owner), name, value, deltas
                        )
                    )
        elif value in self.import_bindings:
            binding = self.import_bindings[value]
            for imported in binding.modules:
                normalized = normalize_stdlib_module_name(imported)
                if imported not in self.modules and normalized in self.modules:
                    imported = normalized
                imported_candidates = (
                    self.watch((imported, None), value)
                    | self.watch(("", SourceCacheItem(imported)), value)
                    | self.watch(("", SourceCacheItem(None, True)), value)
                )
                for candidate in tuple(imported_candidates):
                    if binding.attribute in {None, "*"}:
                        candidates.add(candidate)
                    else:
                        candidates.update(
                            self.select_member(
                                candidate, binding.attribute, value, from_import=True
                            )
                        )
        self._publish_candidates(value, candidates)

    def _publish_candidates(
        self,
        value: SourceValue,
        candidates: set[SourceCandidate] | frozenset[SourceCandidate],
    ) -> None:
        known = self.value_candidates.setdefault(value, _EMPTY_CANDIDATES)
        if not candidates:
            self._activate_pending(value, known, _EMPTY_CANDIDATES)
            return
        if isinstance(known, frozenset):
            # Demand creates a graph node, but its lattice stays empty until
            # a producer publishes the first candidate.
            known = set(candidates)
            self.value_candidates[value] = known
            added = candidates
        else:
            added = candidates - known
            known.update(added)
        if added:
            if inherited := self._inherited_sources.get(value):
                before = len(inherited.owners)
                inherited.owners.update(candidate.value for candidate in added)
                if len(inherited.owners) != before:
                    for user in inherited.users:
                        self.queue_value(user)
            if writes := self.attribute_writes.get(value):
                self._record_attribute_receivers(value[0], writes, added)
            for user in self.value_users.get(value, ()):
                self.queue_value(user)
            self._add_operand_candidates(value, added)
            for user in self.call_outcome_users.get(value, ()):
                if not self.value_jobs.get(user, False):
                    self.pending_call_outcomes.add(user)
                    self.queue_value(user, refresh=False)
        self._activate_pending(value, known, added)

    def _activate_pending(
        self,
        value: SourceValue,
        known: set[SourceCandidate] | frozenset[SourceCandidate],
        added: set[SourceCandidate] | frozenset[SourceCandidate],
    ) -> None:
        # A demand-only wake joins existing candidates without evaluating their
        # producer. A changed publication visits only its new incidences.
        new_phases = self.pending_phases.pop(value, None)
        if added and (phases := self.value_phases.get(value)):
            for incidence in phases.difference(new_phases or ()):
                self._activate_new_candidates(value, added, incidence)
        for incidence in new_phases or ():
            self._activate_new_candidates(value, known, incidence)

    def _activate_new_candidates(
        self,
        value: SourceValue,
        candidates: set[SourceCandidate] | frozenset[SourceCandidate],
        demand: SourceDemand,
    ) -> None:
        # Subtract effects already executed through aliases before traversing
        # candidates in Python. Namespace writers and initializer replay keep
        # their own incidence state. Opaque arguments record effects at a site
        # before normalizing to an exposure.
        if activated := self.activated.get(demand):
            candidates = candidates.difference(activated)
        for candidate in tuple(candidates):
            self._activate_demand(value, candidate, demand)

    def _receiver_offsets(
        self, candidate: SourceCandidate, selection: SourceValue
    ) -> tuple[bool, ...]:
        bound = candidate in self.receiver_bound_members.get(selection, ())
        if candidate.kind == "bound_class_method" or (
            bound and candidate not in self.unbound_members.get(selection, ())
        ):
            return (True,)
        return (False, True) if bound else (False,)

    def _project_call_result(
        self,
        candidate: SourceCandidate,
        selection: SourceValue,
        user: SourceValue,
        call: PythonSourceCall,
        parameters: PythonSourceCallableDeclaration,
    ) -> set[SourceCandidate] | None:
        if not call.arguments:
            return None
        callee = candidate.value
        result = self.functions.get(callee)
        if result is None:
            # Bodies can publish after their call is admitted.
            self._return_projection_users.setdefault(callee, set()).add(user)
            return None
        if not result.returned and not result.returned_exposed:
            return None
        names = tuple(name for name, _, _ in call.arguments)
        if None in names:
            return None
        # Each operand path is expressed in formals until this invocation binds
        # it. The same transfer still sends actuals to the body for its effects.
        roots = []
        for source in result.returned | result.returned_exposed:
            element = self.elements.get((candidate.module, source))
            if (
                element is not None
                and element.protocol is not PythonElementProtocol.SUBSCRIPT
            ):
                return None
            roots.append((source, element, source not in result.returned))
        actuals = []
        for offset in self._receiver_offsets(candidate, selection):
            bindings = self._argument_bindings(parameters, names, offset)
            explicit = {}
            for formal, index, packed in bindings:
                if not packed:
                    if formal in explicit:
                        return None
                    explicit[formal] = index
            for source, element, exposed_only in roots:
                origins = (source,) if element is None else element.origins
                for formal in origins:
                    index = explicit.get(formal)
                    if index is None or not call.arguments[index][2]:
                        return None
                    actuals.append((index, element, exposed_only))
        for dependency in (callee, selection):
            self._return_projection_users.setdefault(dependency, set()).add(user)
        selected: set[SourceCandidate] = set()
        for index, element, exposed_only in actuals:
            if element is not None:
                self._return_projection_users.setdefault(
                    (candidate.module, element.owner), set()
                ).add(user)
            _, direct, exposed = call.arguments[index]
            for source in exposed:
                for value in tuple(self.watch((user[0], source), user)):
                    contained = source not in direct
                    if value.contained != contained:
                        value = value._replace(contained=contained)
                    if element is None:
                        selected.add(
                            value._replace(contained=True)
                            if exposed_only and not value.contained
                            else value
                        )
                        continue
                    results = self._subscript_values(
                        value, user, element.key, consumer=candidate.module
                    )
                    selected.update(
                        (
                            item if item.contained else item._replace(contained=True)
                            for item in results
                        )
                        if exposed_only
                        else results
                    )
        return selected

    def _route_call_arguments(
        self,
        module: str,
        inputs: SourceCallArguments,
        user: SourceValue,
        deltas: _OperandDeltas | None,
    ) -> None:
        # Return values can grow without changing a call's actuals. Watch
        # operand and declaration changes from the argument transfer alone.
        site = (module, inputs.site)
        if site in self.import_bindings:
            return
        call = self.calls[site]
        routes: dict[tuple[SourceValue, bool], bool] = {}
        actuals: dict[tuple[int, bool, bool], set[SourceCandidate]] = {}
        names = tuple(name for name, _, _ in call.arguments)
        actuals_changed = deltas is not None and any(
            (module, source) in deltas
            for _, _, origins in call.arguments
            for source in origins
        )

        def values(index: int, packed: bool, full: bool) -> set[SourceCandidate]:
            key = (index, packed, full)
            selected = actuals.get(key)
            if selected is not None:
                return selected
            name, direct, exposed = call.arguments[index]
            if name is None:
                direct = exposed
            selected = {
                candidate._replace(contained=packed or source not in direct)
                for source in exposed
                for candidate in self._operand_candidates(
                    (module, source), user, None if full else deltas
                )
            }
            actuals[key] = selected
            return selected

        for origin in call.callees:
            selection = (module, origin)
            added = () if deltas is None else deltas.get(selection, ())
            callees: Collection[SourceCandidate]
            if deltas is None:
                self.binding_users[selection].add(user)
                callees = self._ensure_value(selection)
            elif actuals_changed:
                callees = self.value_candidates[selection]
            else:
                callees = added
            for candidate in callees:
                if (
                    candidate.contained
                    or candidate.identity is not None
                    or candidate.source is None
                    or candidate.kind.startswith("namespace")
                ):
                    continue
                for offset in self._receiver_offsets(candidate, selection):
                    route = (candidate.value, offset)
                    # A new route joins prior actuals as well. Existing routes
                    # receive only the actuals that arrived in this wave.
                    routes[route] = routes.get(route, False) or (
                        deltas is None or candidate in added
                    )

        def full_values(index: int, packed: bool) -> set[SourceCandidate]:
            return values(index, packed, True)

        def delta_values(index: int, packed: bool) -> set[SourceCandidate]:
            return values(index, packed, False)

        for (callee, offset), full in routes.items():
            self._bind_actuals(
                names, full_values if full else delta_values, callee, user, offset
            )

    def _bind_actuals(
        self,
        names: tuple[int | str | None, ...],
        candidates: Callable[[int, bool], set[SourceCandidate]],
        callee: SourceValue,
        user: SourceValue,
        receiver_bound: bool,
    ) -> None:
        parameters = self._read_declaration(callee, user)
        if parameters is None:
            return
        for target, index, packed in self._argument_bindings(
            parameters, names, receiver_bound
        ):
            self._queue_arguments((callee[0], target), candidates(index, packed))

    def _argument_bindings(
        self,
        parameters: PythonSourceCallableDeclaration,
        names: tuple[int | str | None, ...],
        receiver_bound: bool,
    ) -> tuple[tuple[PythonSourceKey, int, bool], ...]:
        key = (parameters, names, receiver_bound)
        previous = self._formal_bindings.get(key)
        if previous is not None:
            return previous
        keywords = dict(parameters.keywords)
        result = []
        for index, name in enumerate(names):
            if isinstance(name, int):
                if receiver_bound:
                    name += 1
                targets = (
                    ((parameters.positional[name], False),)
                    if name < len(parameters.positional)
                    else (
                        ((parameters.vararg, True),)
                        if parameters.vararg is not None
                        else ()
                    )
                )
            elif isinstance(name, str):
                target = keywords.get(name)
                targets = (
                    ((target, False),)
                    if target is not None
                    else (
                        ((parameters.kwarg, True),)
                        if parameters.kwarg is not None
                        else ()
                    )
                )
            else:
                # Unknown expansions can supply any formal; their contained
                # candidates become direct parameter values after unpacking.
                targets = tuple(
                    (target, target in {parameters.vararg, parameters.kwarg})
                    for target in dict.fromkeys(
                        (
                            *parameters.positional,
                            *keywords.values(),
                            *(
                                (parameters.vararg,)
                                if parameters.vararg is not None
                                else ()
                            ),
                            *(
                                (parameters.kwarg,)
                                if parameters.kwarg is not None
                                else ()
                            ),
                        )
                    )
                )
            result.extend((target, index, packed) for target, packed in targets)
        bindings = tuple(result)
        self._formal_bindings[key] = bindings
        return bindings

    def _bind_native_call(self, site: SourceValue, callee: SourceCandidate) -> None:
        uses = self.native_calls.setdefault(callee.value, set())
        if (site, callee) not in uses:
            uses.add((site, callee))
            self.queue_value(callee.value)
        # Actual values can arrive after the class or function alias did.
        for _, direct, exposed in self.calls[site].arguments:
            for source in direct | exposed:
                self.watch((site[0], source), callee.value)

    def _native_sources(self, module, direct, exposed, user):
        values = set()
        for source in direct | exposed:
            values.update(
                candidate._replace(
                    contained=source not in direct or candidate.contained
                )
                for candidate in self.watch((module, source), user)
            )
        return values

    def _native_metadata(self, origin, names, key=None):
        plan = self.native_initializers[origin.value]
        values = set()
        for name, sources in plan.attributes:
            if name in names:
                for source in sources:
                    for candidate in tuple(
                        self.watch((origin.module, source), origin.value)
                    ):
                        if candidate.kind == "namespace":
                            values.update(
                                self._namespace_values(candidate, origin.value, key)
                            )
                        else:
                            values.add(candidate._replace(contained=False))
        return values

    def _activate_native_initializer(self, origin: SourceCandidate) -> None:
        plan = self.native_initializers.get(origin.value)
        if plan is None:
            return
        captures = self._native_sources(
            origin.module, frozenset(), plan.captures, origin.value
        )
        captures = {candidate._replace(contained=False) for candidate in captures}
        defaults = self._native_metadata(origin, {"__defaults__", "__kwdefaults__"})
        slots: dict[int, set[SourceCandidate]] = {}
        actuals = set()
        unknown_arguments = False
        for site, callee in tuple(self.native_calls.get(origin.value, ())):
            if callee.receiver != origin.receiver and origin.receiver is not None:
                continue
            for name, direct, exposed in self.calls[site].arguments:
                values = self._native_sources(site[0], direct, exposed, origin.value)
                if name is None:
                    unknown_arguments = True
                    values = {
                        candidate._replace(contained=False) for candidate in values
                    }
                actuals.update(values)
                if isinstance(name, int):
                    slot = name + (callee.kind == "bound_native_initializer")
                elif (
                    isinstance(name, str)
                    and plan.parameters is not None
                    and name in plan.parameters
                ):
                    slot = plan.parameters.index(name)
                else:
                    continue
                slots.setdefault(slot, set()).update(values)

        def argument(slot):
            if slot is None or unknown_arguments:
                return actuals | defaults
            return slots.get(slot, set()) | defaults

        def capture(slot):
            if slot is None or plan.capture_values is None:
                return captures
            if slot < 0 or slot >= len(plan.capture_values):
                return set()
            direct, exposed = plan.capture_values[slot]
            return self._native_sources(origin.module, direct, exposed, origin.value)

        # Only factory-tagged bindings are invoked. Ordinary captured defaults
        # remain values until a store descriptor or a post-init consumer uses them.
        stored = set()
        rows = plan.stores
        if rows is None:
            factories = captures | actuals
            stored.update(factories | defaults)
        else:
            factories = set()
            for _, supplied, binding_arg, binding_capture, factory in rows:
                binding = (
                    argument(binding_arg)
                    if binding_arg is None or binding_arg >= 0
                    else set()
                )
                if binding_capture is None or binding_capture >= 0:
                    binding |= capture(binding_capture)
                if factory is not False:
                    factories.update(binding)
                if factory is not True:
                    stored.update(
                        argument(supplied)
                        if supplied is None or supplied >= 0
                        else binding
                    )
        for factory in factories:
            self.activate(factory, "call", argument_count=0)
            stored.update(self._returned_values(factory, origin.value))
        receivers = (
            self.watch(origin.receiver, origin.value)
            if origin.receiver is not None
            else set().union(
                *(
                    self.watch((origin.module, source), origin.value)
                    for source in plan.receiver
                )
            )
        )
        for receiver in tuple(receivers):
            info = self.classes.get(receiver.value)
            namespace = self.namespaces.get(receiver.value, _EMPTY_NAMESPACE)
            store_names = None if rows is None else {row[0] for row in rows}
            custom_store = (
                info is None
                or not info.plain_metaclass
                or bool(self.bases.get(receiver.value))
                or bool(namespace.exposure)
                or bool(namespace.members.get("__setattr__"))
                or store_names is None
                or None in store_names
                or any(
                    name in namespace.members and name not in info.plain_attributes
                    for name in store_names
                )
            )
            self.activate(receiver._replace(kind="instance"), "descriptor")
            if custom_store:
                for value in stored:
                    self.activate(value, "escape")
            for method in self.select_member(receiver, "__post_init__", origin.value):
                self._bind_receiver(method, receiver._replace(kind="instance"))
                parameters = self.declarations.get(method.value)
                post_rows = plan.post_inputs
                inputs = (
                    [
                        argument(slot)
                        if slot is None or slot >= 0
                        else self._native_metadata(
                            origin, {"__globals__", "__builtins__"}, name
                        )
                        for name, slot in post_rows
                    ]
                    if post_rows is not None
                    else [
                        actuals
                        | defaults
                        | captures
                        | self._native_metadata(origin, {"__globals__", "__builtins__"})
                    ]
                )
                if parameters is not None:
                    for index, values in enumerate(inputs, 1):
                        if post_rows is None:
                            targets = [
                                (target, False) for target in parameters.positional[1:]
                            ]
                            if parameters.vararg is not None:
                                targets.append((parameters.vararg, True))
                        elif index < len(parameters.positional):
                            targets = [(parameters.positional[index], False)]
                        elif parameters.vararg is not None:
                            targets = [(parameters.vararg, True)]
                        else:
                            targets = []
                        for target, contained in targets:
                            selected = {
                                candidate._replace(
                                    contained=contained or candidate.contained
                                )
                                for candidate in values
                            }
                            self._queue_arguments((method.module, target), selected)
                self.activate(method, "call")

    def _read_declaration(
        self, callee: SourceValue, user: SourceValue
    ) -> PythonSourceCallableDeclaration | None:
        self.declaration_users[callee].add(user)
        return self.declarations.get(callee)

    def _function_kind(
        self, callee: SourceCandidate, *, user: SourceValue | None = None
    ) -> FunctionKind:
        value = callee.value
        declaration = (
            self.declarations.get(value)
            if user is None
            else self._read_declaration(value, user)
        )
        return FunctionKind.SYNC if declaration is None else declaration.kind

    def _subscript_values(
        self,
        owner: SourceCandidate,
        user: SourceValue,
        key: str | None,
        *,
        key_values: Collection[SourceCandidate] = (),
        consumer: str | None = None,
    ) -> set[SourceCandidate]:
        if owner.contained:
            # Exposure is transitive, so one selection can still leave a container.
            return {owner, owner._replace(contained=False)}
        if owner.kind == "namespace":
            return self._namespace_values(owner, user, key)
        if owner.kind == "instance":
            member = (owner.module, SourceMember(owner, "__getitem__"))
            self.demand(member, "call", argument_count=1, consumer=consumer or user[0])
            if key_values:
                for getter in self.watch(member, user):
                    self._bind_actuals(
                        (0,),
                        lambda _, packed: {
                            value._replace(contained=packed or value.contained)
                            for value in key_values
                        },
                        getter.value,
                        user,
                        getter.kind != "bound_static_method",
                    )
            return set(self.watch((owner.module, SourceReturn(member)), user))
        return set()

    def _attribute_path_values(
        self, owner: SourceCandidate, path: str, user: SourceValue
    ) -> set[SourceCandidate]:
        selected = {owner}
        for name in path.split("."):
            selected = {
                member
                for candidate in selected
                for member in self.watch(
                    (candidate.module, SourceMember(candidate, name)), user
                )
            }
        return selected

    def _methodcaller_values(
        self,
        selector: SourceCandidate,
        constructor: PythonSourceCall,
        owners: Collection[SourceCandidate],
        user: SourceValue,
    ) -> set[SourceCandidate]:
        names = constructor.argument_strings
        member = names[0] if names else None
        # Invocation looks up the method on the receiver before passing its
        # captured arguments. A custom lookup may have effects of its own.
        for owner in owners:
            self.activate(owner, "descriptor", consumer=user[0])
        if member is None or constructor.arguments[0][0] != 0:
            return {
                _INTERPRETER_BUILTINS,
                SourceCandidate(*selector.value, "reflective_result"),
            }
        actuals = constructor.arguments[1:]
        selectors = tuple(
            name - 1 if isinstance(name, int) else name for name, _, _ in actuals
        )
        fixed_key = (
            names[1] if len(names) > 1 and actuals and actuals[0][0] == 1 else None
        )

        def argument(index: int, packed: bool) -> set[SourceCandidate]:
            return {
                candidate._replace(contained=packed or candidate.contained)
                for candidate in self._argument_candidates(
                    actuals[index], user, True, module=selector.module
                )
            }

        result: set[SourceCandidate] = set()
        for owner in owners:
            if member == "__getattribute__":
                # The default slot reads the named attribute directly. A
                # source override is also an ordinary callback below.
                if fixed_key is None:
                    result.update(
                        {
                            _INTERPRETER_BUILTINS,
                            SourceCandidate(*selector.value, "reflective_result"),
                        }
                    )
                else:
                    selected = (owner.module, SourceMember(owner, fixed_key))
                    result.update(self.watch(selected, user))
            selection = (owner.module, SourceMember(owner, member))
            for callee in self.watch(selection, user):
                self._bind_actuals(
                    selectors,
                    argument,
                    callee.value,
                    user,
                    owner.kind == "instance"
                    and callee.kind != "bound_static_method"
                    or callee.kind == "bound_class_method",
                )
                self.activate(callee, "call", consumer=user[0])
                result.update(self._returned_values(callee, user))
                if callee.namespace_mutation is not None:
                    site = user[1]
                    if isinstance(site, SourceIteratorItems):
                        site = site.site
                    if type(site) is tuple and len(site) == 5:
                        key = (
                            fixed_key
                            if callee.namespace_mutation is MappingWriteScope.KEY
                            else None
                        )
                        self._record_namespace_write(user[0], site, callee, key)
        return result

    def _native_selector_values(
        self,
        selector: SourceCandidate,
        owners: Collection[SourceCandidate],
        user: SourceValue,
    ) -> set[SourceCandidate]:
        self.watch(selector.value, user)
        constructor = self.calls[selector.value]
        if selector.identity is PythonIdentity.OPERATOR_METHODCALLER:
            return self._methodcaller_values(selector, constructor, owners, user)
        expanded = any(name is None for name, _, _ in constructor.arguments)
        keys: dict[str | None, set[SourceCandidate]] = {}
        for index, (name, origins, exposed) in enumerate(constructor.arguments):
            key = (
                constructor.argument_strings[index]
                if name is not None and index < len(constructor.argument_strings)
                else None
            )
            values = keys.setdefault(key, set())
            for source in exposed:
                for value in self.watch((selector.module, source), user):
                    values.add(
                        value._replace(
                            contained=(value.contained or source not in origins)
                        )
                    )
                    if name is None:
                        # Expansion may supply the inner value as a key itself.
                        values.add(value._replace(contained=False))
        result: set[SourceCandidate] = set()
        if selector.identity is PythonIdentity.OPERATOR_ATTRGETTER and None in keys:
            # A computed, possibly dotted selection may expose the live table
            # directly or return an object through which metadata is accessible.
            # Keep the capability on the result for its actual consumer.
            result.add(_INTERPRETER_BUILTINS)
            result.add(SourceCandidate(*selector.value, "reflective_result"))
        for owner in owners:
            for key, values in keys.items():
                if selector.identity is PythonIdentity.OPERATOR_ITEMGETTER:
                    result.update(
                        self._subscript_values(owner, user, key, key_values=values)
                    )
                elif key is not None:
                    result.update(self._attribute_path_values(owner, key, user))
        if not expanded and len(constructor.arguments) == 1:
            return result
        contents = {item._replace(contained=True) for item in result}
        return result | contents if expanded else contents

    def _subclass_selection_written(
        self, owner: SourceValue, user: SourceValue
    ) -> bool:
        mask = ClassLifecycleWrite.INIT_SUBCLASS | ClassLifecycleWrite.MRO
        if (
            self.class_lifecycle_writes.get(owner, ClassLifecycleWrite.NONE)
            & ClassLifecycleWrite.MRO
        ):
            return True
        for base in self._strict_ancestors(owner, user):
            self.watch(base.value, user)
            if (
                self.class_lifecycle_writes.get(base.value, ClassLifecycleWrite.NONE)
                & mask
            ):
                return True
        return False

    def _call_init_subclass(self, owner: SourceCandidate, user: SourceValue) -> None:
        # Type selection starts above the new class. Namespace construction
        # has already run __set_name__, so reached writes can change this edge.
        self.watch(owner.value, user)
        member_seen: set[tuple[str, PythonSourceKey | None]] = set()
        for origin in self.bases.get(owner.value, ()):
            for base in self.watch((owner.module, origin), user):
                if not self._source_class_candidate(base) or base.kind != "value":
                    continue
                for callback in self.select_member(
                    base,
                    "__init_subclass__",
                    user,
                    member_seen=member_seen,
                    binding_owner=owner,
                ):
                    if callback.kind == "value":
                        self._bind_actuals(
                            (0,),
                            lambda _, packed: {owner._replace(contained=packed)},
                            callback.value,
                            user,
                            False,
                        )
                    self.demand(
                        (callback.module, SourceReturn(callback.value)), "release"
                    )
                    self.activate(
                        callback, "call", argument_count=0, consumer=owner.module
                    )
        if self._subclass_selection_written(owner.value, user):
            self.activate(owner, "escape", consumer=owner.module)

    def _returned_values(
        self,
        callee: SourceCandidate,
        user: SourceValue,
        *,
        forward_outcome: bool = False,
        selection: SourceValue | None = None,
    ) -> Set[SourceCandidate]:
        if callee.kind == "iterator_next":
            return self.watch(self._iterator_contents(callee), user)
        if callee.kind == "iterator_self":
            return {callee._replace(kind="iterator")}
        if callee.kind == "namespace_popitem":
            # Removal returns (key, value), not the value returned by pop().
            return {
                candidate._replace(contained=True)
                for candidate in self._namespace_values(callee, user)
            }
        if callee.kind in {"namespace_get", "namespace_remove"}:
            call = self.calls.get(user)
            key = (
                call.argument_strings[0]
                if call is not None and call.argument_strings
                else None
            )
            result = self._namespace_values(callee, user, key)
            if call is not None:
                for name, origins, exposed in call.arguments[1:2]:
                    for source in exposed:
                        result.update(
                            candidate._replace(contained=source not in origins)
                            for candidate in self.watch((user[0], source), user)
                        )
            return result
        if callee.kind == "namespace_values_method":
            return {callee._replace(kind="namespace_values")}
        if callee.kind == "namespace_copy_method":
            return {
                callee._replace(
                    kind="namespace", namespace_ownership=PythonNamespaceOwnership.OWNED
                )
            }
        if callee.kind == "namespace_items_method":
            return {callee._replace(kind="namespace_items")}
        if (
            callee.identity is not None
            or callee.kind
            in {
                "namespace_clear",
                "namespace_write_descriptor",
                "namespace_write_method",
                "resumable",
                "class_descriptor",
                "property_descriptor",
            }
            or callee.source is None
        ):
            return _EMPTY_CANDIDATES
        # A candidate's declaration selects its calling protocol. Reads of
        # its returned values subscribe to the outcome, not the callee value.
        value = callee.value
        declaration = self._read_declaration(value, user)
        if declaration is not None and declaration.kind != FunctionKind.SYNC:
            return {SourceCandidate(*value, "resumable")}
        if (
            selection is not None
            and declaration is not None
            and callee.kind in {"value", "bound_class_method", "bound_static_method"}
        ):
            projection = self._project_call_result(
                callee, selection, user, self.calls[user], declaration
            )
            if projection is not None:
                return projection
        return self._function_values(callee, user, forward_outcome=forward_outcome)

    def _function_values(
        self,
        callee: SourceCandidate,
        user: SourceValue,
        *,
        yielded: bool = False,
        forward_outcome: bool = False,
    ) -> Set[SourceCandidate]:
        if callee.source is None:
            return _EMPTY_CANDIDATES
        outcome = (callee.module, SourceCallableOutcome(callee.source, yielded))
        if forward_outcome:
            self.call_outcome_inputs.setdefault(user, set()).add(outcome)
            self.call_outcome_users.setdefault(outcome, set()).add(user)
            return self._ensure_value(outcome)
        return self.watch(outcome, user)

    def _awaited_values(
        self, awaited: SourceCandidate, user: SourceValue
    ) -> Set[SourceCandidate]:
        if awaited.contained:
            return _EMPTY_CANDIDATES
        if awaited.kind == "resumable":
            if self._function_kind(awaited, user=user) == FunctionKind.ASYNC:
                return self._function_values(awaited, user)
        elif awaited.kind == "instance":
            member = (awaited.module, SourceMember(awaited, "__await__"))
            self.demand(member, "call")
            returned = (awaited.module, SourceReturn(member))
            self.demand(returned, "next")
            result: set[SourceCandidate] = set()
            for iterator in tuple(self.watch(returned, user)):
                result.update(self._generator_completion_values(iterator, user))
            return result
        return _EMPTY_CANDIDATES

    def _generator_completion_values(
        self, generator: SourceCandidate, user: SourceValue
    ) -> Set[SourceCandidate]:
        if (
            not generator.contained
            and generator.kind == "resumable"
            and self._function_kind(generator, user=user) == FunctionKind.GENERATOR
        ):
            return self._function_values(generator, user)
        return _EMPTY_CANDIDATES

    @staticmethod
    def _positional_arities(call: PythonSourceCall) -> tuple[bool, bool]:
        names = tuple(name for name, _, _ in call.arguments)
        if any(isinstance(name, str) for name in names):
            return False, False
        if None in names:
            return True, True
        return len(names) == 1, len(names) == 2

    def _iteration_operands(
        self,
        identity: PythonIdentity,
        call: PythonSourceCall,
        user: SourceValue,
        unary: bool,
        *,
        include_contents: bool,
    ) -> dict[int | None, set[SourceCandidate]]:
        if identity is PythonIdentity.BUILTIN_MAP:
            # Build payload slots in one traversal. Expansions can supply any
            # still-unbound formal and share a single unknown-position slot.
            operands: dict[int | None, set[SourceCandidate]] = {}
            for argument in call.arguments:
                name = argument[0]
                if name is None or isinstance(name, int) and name > 0:
                    position = None if name is None else name - 1
                    operands.setdefault(position, set()).update(
                        self._argument_candidates(argument, user, include_contents)
                    )
            return operands
        native_payload = NATIVE_ITERATOR_PAYLOADS.get(identity)
        keyword = None
        if native_payload is not None:
            position, keyword, _kind = native_payload
        elif identity is PythonIdentity.BUILTIN_ZIP:
            position = None
        elif identity is PythonIdentity.BUILTIN_FILTER:
            position = 1
        elif unary or identity is PythonIdentity.BUILTIN_ENUMERATE:
            position = 0
            if identity is PythonIdentity.BUILTIN_ENUMERATE:
                keyword = "iterable"
        else:
            return {}
        return {
            0: self._positional_candidates(
                call,
                user,
                position,
                include_contents=include_contents,
                keyword=keyword,
            )
        }

    def _iteration_call(
        self, identity: PythonIdentity, call: PythonSourceCall, user: SourceValue
    ) -> set[SourceCandidate]:
        unary, binary = self._positional_arities(call)
        module = user[0]
        if identity is not PythonIdentity.BUILTIN_NEXT:
            native_payload = NATIVE_ITERATOR_PAYLOADS.get(identity)
            # Variadic chain acquires each input on advancement. A captured
            # object (repeat) never uses the iteration protocol on that object.
            captures_without_acquiring = native_payload is not None and (
                native_payload.position is None
                or native_payload.kind is PythonIteratorPayloadKind.VALUE
            )
            operands = (
                {}
                if captures_without_acquiring
                else self._iteration_operands(
                    identity, call, user, unary, include_contents=False
                )
            )
            if not operands and not (
                captures_without_acquiring
                or binary
                and identity is PythonIdentity.BUILTIN_ITER
            ):
                return set()
            acquired: set[SourceCandidate] = set()
            for argument in operands.values():
                for receiver in argument:
                    if identity is PythonIdentity.BUILTIN_ITER and unary:
                        if receiver.kind == "instance" and not receiver.contained:
                            member = (
                                receiver.module,
                                SourceMember(receiver, "__iter__"),
                            )
                            acquired.update(
                                self.watch(
                                    (receiver.module, SourceReturn(member)), user
                                )
                            )
                        elif receiver.kind == "iterator" or (
                            receiver.kind == "resumable"
                            and self._function_kind(receiver, user=user)
                            == FunctionKind.GENERATOR
                        ):
                            acquired.add(receiver)
                        elif self._source_class_candidate(receiver):
                            # A custom metaclass can supply the iteration slot.
                            # Keep its callbacks until the metaclass is sealed.
                            self.activate(receiver, "escape", consumer=module)
                    if identity is PythonIdentity.BUILTIN_REVERSED:
                        if receiver.kind == "instance" and not receiver.contained:
                            for method in ("__reversed__", "__len__"):
                                self.demand(
                                    (receiver.module, SourceMember(receiver, method)),
                                    "call",
                                )
                    else:
                        self.activate(receiver, "get_iter", consumer=module)
            # The native sequence fallback supplies an iterator as well;
            # an explicit __iter__ can return an object with additional methods.
            acquired.add(
                SourceCandidate(
                    module, call.owner, "iterator", iterator_producer=identity
                )
            )
            return acquired

        if not (unary or binary):
            return set()
        result: set[SourceCandidate] = set()
        for iterator in self._positional_candidates(call, user, 0):
            self.activate(iterator, "next", consumer=module)
            result.update(self._advanced_values(iterator, user, asynchronous=False))
        if binary:
            result.update(
                self._positional_candidates(call, user, 1, include_contents=True)
            )
        return result

    @staticmethod
    def _iterator_contents(iterator: SourceCandidate) -> SourceValue:
        assert iterator.source is not None and iterator.iterator_producer is not None
        return (
            iterator.module,
            SourceIteratorItems(iterator.source, iterator.iterator_producer),
        )

    def _iterator_operand_values(
        self,
        identity: PythonIdentity,
        owners: set[SourceCandidate],
        user: SourceValue,
    ) -> set[SourceCandidate]:
        result: set[SourceCandidate] = set()
        for owner in owners:
            if owner.contained:
                # An aggregate can hold or transitively contain this value.
                result.add(owner)
                result.add(owner._replace(contained=False))
            elif identity is PythonIdentity.BUILTIN_REVERSED:
                if owner.kind == "instance":
                    # A custom reverse slot supplies an iterator. The fallback
                    # reads sequence indices even when __iter__ exists.
                    reverse = (owner.module, SourceMember(owner, "__reversed__"))
                    returned = (owner.module, SourceReturn(reverse))
                    self.demand(returned, "next")
                    for iterator in self.watch(returned, user):
                        result.update(
                            self._advanced_values(iterator, user, asynchronous=False)
                        )
                    index = (owner.module, SourceMember(owner, "__getitem__"))
                    self.demand(index, "call")
                    result.update(self.watch((owner.module, SourceReturn(index)), user))
            else:
                self.activate(owner, "iterate", consumer=user[0])
                result.update(self._iterated_values(owner, user, asynchronous=False))
        return result

    def _iterator_items(
        self, module: str, items: SourceIteratorItems, user: SourceValue
    ) -> set[SourceCandidate]:
        origin = (module, items.site)
        # Revisit a consumed iterator if its constructor learns new arguments.
        self.watch(origin, user)
        call = self.calls[origin]
        unary, binary = self._positional_arities(call)
        inputs = self._iteration_operands(
            items.producer, call, user, unary, include_contents=True
        )
        native_payload = NATIVE_ITERATOR_PAYLOADS.get(items.producer)
        if (
            native_payload is not None
            and native_payload.kind is PythonIteratorPayloadKind.VALUE
        ):
            return set().union(*inputs.values())
        operands = tuple(
            self._iterator_operand_values(items.producer, argument, user)
            for argument in inputs.values()
        )
        if items.producer in {
            PythonIdentity.BUILTIN_MAP,
            PythonIdentity.BUILTIN_FILTER,
        }:
            # These adapters invoke their callbacks at advancement. Reuse the formal
            # binder as direct source calls and feed it the yielded operands.
            def payload(index: int, packed: bool) -> set[SourceCandidate]:
                return {
                    candidate._replace(contained=packed or candidate.contained)
                    for candidate in operands[index]
                }

            names = tuple(inputs)
            count = len(names) if None not in names else None
            result: set[SourceCandidate] = set()
            for callback in self._positional_candidates(call, user, 0):
                self._bind_actuals(
                    names,
                    payload,
                    callback.value,
                    user,
                    callback.kind == "bound_class_method",
                )
                if callback.namespace_mutation is not None:
                    if callback.kind == "namespace_write_descriptor":
                        for receiver in operands[0] if operands else ():
                            if receiver.kind == "namespace":
                                self._record_namespace_write(
                                    module, items.site, receiver, None
                                )
                    else:
                        self._record_namespace_write(module, items.site, callback, None)
                self.activate(callback, "call", argument_count=count, consumer=module)
                if items.producer is PythonIdentity.BUILTIN_MAP:
                    if callback.kind == "native_selector" and (
                        count is None or count == 1
                    ):
                        result.update(
                            self._native_selector_values(
                                callback, set().union(*operands), user
                            )
                        )
                    else:
                        result.update(self._returned_values(callback, user))
            if items.producer is PythonIdentity.BUILTIN_MAP:
                return result
            # Filter only returns elements from the original iterable.
        result = set().union(*operands)
        if (
            native_payload is not None
            and native_payload.kind is PythonIteratorPayloadKind.FLATTEN
        ):
            return self._iterator_operand_values(items.producer, result, user)
        if (
            native_payload is not None
            and native_payload.kind is PythonIteratorPayloadKind.TUPLE
            or items.producer
            in {PythonIdentity.BUILTIN_ENUMERATE, PythonIdentity.BUILTIN_ZIP}
        ):
            return {item._replace(contained=True) for item in result}
        if binary and items.producer is PythonIdentity.BUILTIN_ITER:
            # iter(callable, sentinel) calls its producer only on advancement.
            for callback in self._positional_candidates(call, user, 0):
                self.activate(callback, "call", argument_count=0, consumer=module)
                result.update(self._returned_values(callback, user))
        return result

    def _advanced_values(
        self, iterator: SourceCandidate, user: SourceValue, *, asynchronous: bool
    ) -> Set[SourceCandidate]:
        if iterator.contained:
            return _EMPTY_CANDIDATES
        if iterator.kind == "iterator" and not asynchronous:
            return self.watch(self._iterator_contents(iterator), user)
        expected = (
            FunctionKind.ASYNC_GENERATOR if asynchronous else FunctionKind.GENERATOR
        )
        if (
            iterator.kind == "resumable"
            and self._function_kind(iterator, user=user) == expected
        ):
            return self._function_values(iterator, user, yielded=True)
        if iterator.kind == "instance":
            name = "__anext__" if asynchronous else "__next__"
            member = (iterator.module, SourceMember(iterator, name))
            item = (iterator.module, SourceReturn(member))
            if asynchronous:
                item = (iterator.module, SourceAwait(item))
            return self.watch(item, user)
        return _EMPTY_CANDIDATES

    def _iterated_values(
        self, iterable: SourceCandidate, user: SourceValue, *, asynchronous: bool
    ) -> Set[SourceCandidate]:
        if iterable.kind in {"namespace_values", "namespace_items"}:
            return {
                candidate._replace(contained=iterable.kind == "namespace_items")
                for candidate in self._namespace_values(iterable, user)
            }
        if iterable.kind != "instance":
            return self._advanced_values(iterable, user, asynchronous=asynchronous)
        member = (
            iterable.module,
            SourceMember(iterable, "__aiter__" if asynchronous else "__iter__"),
        )
        returned = (iterable.module, SourceReturn(member))
        result: set[SourceCandidate] = set()
        for iterator in tuple(self.watch(returned, user)):
            result.update(
                self._advanced_values(iterator, user, asynchronous=asynchronous)
            )
        if not asynchronous:
            sequence = (iterable.module, SourceMember(iterable, "__getitem__"))
            result.update(self.watch((iterable.module, SourceReturn(sequence)), user))
        return result
