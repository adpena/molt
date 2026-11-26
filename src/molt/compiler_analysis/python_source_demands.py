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
    PythonElementProtocol,
    PythonNativeInitializerSource,
    PythonSourceClass,
)

from collections import deque
from dataclasses import dataclass, field
from enum import StrEnum
from collections.abc import Callable, Iterable, Mapping
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
from molt.compiler_analysis.python_source_keys import PythonSourceKey
from molt.compiler_analysis.python_parameter_classes import representative_arguments
from molt.compiler_analysis.python_identity_catalog_generated import (
    PythonIdentity,
    INTRINSIC_ACQUISITION_IDENTITIES,
    INTRINSIC_RESULT_IDENTITIES,
    NATIVE_ITERATOR_PAYLOADS,
    PythonIteratorPayloadKind,
)
from molt.compiler_analysis.python_value_identity import (
    SOURCE_CALLABLE_IDENTITIES,
    SOURCE_ITERATION_CALLABLES,
    possible_runtime_import_call_identities,
)
from molt.compiler_analysis.static_truth import (
    IMPLICIT_INSTANCE_SPECIAL_METHODS,
    context_execution_protocol,
)


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


class SourceCandidate(NamedTuple):
    module: str
    source: PythonSourceKey | None
    kind: Literal[
        "value",
        "instance",
        "resumable",
        "iterator",
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
    def value(self) -> tuple[str, PythonSourceKey | None]:
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


class SourceMember(NamedTuple):
    owner: SourceCandidate
    name: str


class SourceCacheItem(NamedTuple):
    key: str | None
    # None normally means every key. A wildcard write is also a possible
    # provider for each literal-key read without mixing unrelated literal keys.
    wildcard: bool = False


class SourceReturn(NamedTuple):
    callee: SourceValue


class SourceCallArguments(NamedTuple):
    site: PythonSourceKey


class SourceIteratorItems(NamedTuple):
    site: PythonSourceKey
    producer: PythonIdentity


class SourceAwait(NamedTuple):
    value: SourceValue


class SourceGeneratorCompletion(NamedTuple):
    value: SourceValue


SourceValue = tuple[
    str,
    PythonSourceKey
    | SourceMember
    | SourceCacheItem
    | SourceReturn
    | SourceCallArguments
    | SourceIteratorItems
    | SourceAwait
    | SourceGeneratorCompletion
    | None,
]


class SourceDemand(NamedTuple):
    phase: SourceDemandPhase
    arguments: tuple[str, ...]
    argument_count: int | None
    consumer: str


type SourceActivation = tuple[
    SourceCandidate, SourceDemandPhase, tuple[str, ...], int | None, str
]


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
        self.value_candidates: dict[SourceValue, set[SourceCandidate]] = {}
        self.value_users: dict[SourceValue, set[SourceValue]] = {}
        # Operand transfers read candidate membership and callable shape.
        # Outcome facts such as a growing return set have their own users.
        self.candidate_users: dict[SourceValue, set[SourceValue]] = {}
        self.declaration_users: dict[SourceValue, set[SourceValue]] = {}
        self.value_phases: dict[SourceValue, set[SourceDemand]] = {}
        self.pending_phases: dict[SourceValue, set[SourceDemand]] = {}
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
        self.arguments: dict[SourceValue, set[SourceCandidate]] = {}
        self.functions: dict[SourceValue, PythonFunctionResult] = {}
        self.activation_partitions: dict[
            SourceValue, tuple[frozenset[str] | None, ...]
        ] = {}
        self.native_initializers: dict[SourceValue, PythonNativeInitializerSource] = {}
        self.native_initializers_by_receiver: dict[SourceValue, set[SourceValue]] = {}
        self.native_calls: dict[
            SourceValue, set[tuple[SourceValue, SourceCandidate]]
        ] = {}
        self.classes: dict[SourceValue, PythonSourceClass] = {}
        self.pending_modules: dict[str, set[SourceValue]] = {}
        # Per attribute value: the functions it selected through an instance
        # (the receiver fills their first parameter) and through anything else.
        self.receiver_bound_members: dict[SourceValue, set[SourceCandidate]] = {}
        self.unbound_members: dict[SourceValue, set[SourceCandidate]] = {}
        # Per attribute value: the values a property getter returned to it. A
        # call of one passes the call's own arguments, never the instance.
        self._property_results: dict[SourceValue, set[SourceCandidate]] = {}
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
        self.value_jobs: set[SourceValue] = set()
        # Body scans by class representative, and every argument tuple that
        # activated each body, so a changed parameter fact re-maps them.
        self.body_jobs: dict[SourceValue, set[tuple[str, ...]]] = {}
        self.body_activations: dict[SourceValue, set[tuple[str, ...]]] = {}
        self.activated: set[SourceActivation] = set()
        self.native_activations: dict[SourceValue, set[SourceActivation]] = {}
        self.queue: deque[SourceValue] = deque()
        self.exports: dict[str, SourceAllExports] = {}

    def queue_value(self, value: SourceValue) -> None:
        if value not in self.value_jobs:
            self.value_jobs.add(value)
            self.queue.append(value)

    def demand(
        self,
        value: SourceValue,
        phase: SourceDemandPhase,
        arguments: tuple[str, ...] = (),
        *,
        argument_count: int | None = None,
        consumer: str | None = None,
    ) -> None:
        phases = self.value_phases.setdefault(value, set())
        incidence = SourceDemand(
            phase, arguments, argument_count, consumer or self._consumer or value[0]
        )
        if incidence not in phases:
            phases.add(incidence)
            self.pending_phases.setdefault(value, set()).add(incidence)
            self.queue_value(value)

    def watch(self, origin: SourceValue, user: SourceValue) -> set[SourceCandidate]:
        self.value_users.setdefault(origin, set()).add(user)
        return self._ensure_value(origin)

    def _watch_candidates(
        self, origin: SourceValue, user: SourceValue
    ) -> set[SourceCandidate]:
        self.candidate_users.setdefault(origin, set()).add(user)
        return self._ensure_value(origin)

    def _ensure_value(self, origin: SourceValue) -> set[SourceCandidate]:
        if origin not in self.value_candidates:
            self.value_candidates[origin] = set()
            self.queue_value(origin)
        return self.value_candidates[origin]

    def _invalidate_value(self, value: SourceValue) -> None:
        if value in self.value_candidates:
            self.queue_value(value)
        for user in self.value_users.get(value, ()):
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
        if values - known:
            known.update(values)
            self.queue_value(("", item))

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
            self._publish(self.elements, value, element)
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
                )
            if self._publish(self.functions, value, result):
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
            if (
                value.source is not None
                or value.identity is not None
                or value.contained
                or value.kind != "value"
            ):
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
                self.demand((module, source), "expose", consumer=module)
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

    def _select_attribute(
        self,
        owner_value: SourceValue,
        name: str,
        user: SourceValue,
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
        for candidate in tuple(self.watch(owner_value, user)):
            source_class = self._source_class_candidate(candidate)
            if strict:
                members: set[SourceCandidate] = set()
                if source_class:
                    for ancestor in self._strict_ancestors(candidate.value, user):
                        members.update(
                            self.select_member(
                                ancestor._replace(kind=candidate.kind),
                                name,
                                user,
                                binding_owner=candidate,
                            )
                        )
            else:
                members = self.select_member(candidate, name, user)
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
            for dependent in self.candidate_users.get(user, ()):
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
            known = self.arguments.setdefault(formal, set())
            if argument not in known:
                known.add(argument)
                self.queue_value(formal)

    def select_member(
        self,
        owner,
        name,
        user,
        *,
        from_import=False,
        from_star=False,
        visiting=frozenset(),
        member_visiting=frozenset(),
        binding_owner=None,
    ):
        if owner.identity is not None:
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
        if owner.contained or owner in member_visiting:
            return result
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
        member_visiting = member_visiting | {owner}
        # An excluded star member still depends on the export policy. A later
        # writer can widen __all__ without changing the module candidate itself.
        self.watch(owner.value, user)
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
                            self._property_results.setdefault(user, set()).update(
                                returned
                            )
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
            for base in self.bases.get(owner.value, ()):
                for candidate in tuple(self.watch((owner_module, base), user)):
                    inherited = SourceCandidate(*candidate.value, owner.kind)
                    result.update(
                        self.select_member(
                            inherited,
                            name,
                            user,
                            member_visiting=member_visiting,
                            binding_owner=binding_owner,
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
                                candidate, name, user, from_star=True, visiting=visiting
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
        incidence = (origin, phase, arguments, argument_count, consumer)
        if phase in {"read", "factory"}:
            if (
                origin.identity is None
                and origin.source is not None
                and origin.source[-1]
                in {"FunctionDef", "AsyncFunctionDef", "Lambda", "GeneratorExp"}
            ):
                self.read_callables.setdefault(origin.module, set()).add(origin.source)
            return
        activated = (
            self.native_activations.setdefault(origin.value, set())
            if origin.kind in {"native_initializer", "bound_native_initializer"}
            else self.activated
        )
        if incidence in activated:
            return
        activated.add(incidence)
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
        if origin.kind == "iterator":
            assert owner is not None
            if phase in {"escape", "expose", "iterate", "next"}:
                self.demand(
                    self._iterator_contents(origin),
                    "escape" if phase in {"escape", "expose"} else "read",
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
        self, argument: PythonSourceArgument, user: SourceValue, include_contents: bool
    ) -> set[SourceCandidate]:
        name, direct, exposed = argument
        selected: set[SourceCandidate] = set()
        for origin in exposed if include_contents or name is None else direct:
            for candidate in self.watch((user[0], origin), user):
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
        if (
            demand.phase in {"escape", "expose"}
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
            demand.phase,
            demand.arguments,
            argument_count=demand.argument_count,
            consumer=demand.consumer,
        )

    def process_value(self, value: SourceValue) -> None:
        self.value_jobs.discard(value)
        module_name, source = value
        if module_name not in self.modules and not isinstance(source, SourceCacheItem):
            self.pending_modules.setdefault(module_name, set()).add(value)
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
                    for item in self.watch(origin, value)
                )
        elif value in self.cache_reads:
            for keys, defaults, default_contents, contained in self.cache_reads[value]:
                items = (
                    [SourceCacheItem(key) for key in keys]
                    if keys is not None
                    else [SourceCacheItem(None)]
                )
                if keys is not None:
                    items.append(SourceCacheItem(None, True))
                for item in items:
                    candidates.update(
                        item._replace(contained=contained or item.contained)
                        for item in self.watch(("", item), value)
                    )
                for default in defaults:
                    candidates.update(self.watch((module_name, default), value))
                for default in default_contents:
                    candidates.update(
                        item._replace(contained=True)
                        for item in self.watch((module_name, default), value)
                    )
        elif isinstance(source, SourceCallArguments):
            self._route_call_arguments(module_name, source, value)
        elif isinstance(source, SourceIteratorItems):
            candidates.update(self._iterator_items(module_name, source, value))
        elif isinstance(source, SourceReturn):
            for callee in tuple(self.watch(source.callee, value)):
                candidates.update(self._returned_values(callee, value))
        elif isinstance(source, SourceAwait):
            for awaited in tuple(self.watch(source.value, value)):
                candidates.update(self._awaited_values(awaited, value))
        elif isinstance(source, SourceGeneratorCompletion):
            for generator in tuple(self.watch(source.value, value)):
                candidates.update(self._generator_completion_values(generator, value))
        elif isinstance(source, SourceMember):
            candidates.update(self.select_member(source.owner, source.name, value))
            if source.owner.kind == "instance":
                for callee in candidates:
                    self.watch(callee.value, value)
                    if callee.kind not in {"bound_class_method", "bound_static_method"}:
                        self._bind_receiver(callee, source.owner)
        elif source is not None and source[-1] == "arg":
            candidates.update(self.arguments.get(value, ()))
        elif source is None or source[-1] == "Module":
            candidates.add(SourceCandidate(module_name, None))
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
            for candidate, phase, args, count, consumer in tuple(
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
                    elif element.protocol is PythonElementProtocol.MAPPING_WRITE:
                        if candidate.kind == "namespace":
                            self._record_namespace_write(
                                module_name, element.owner, candidate, element.key
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
                            candidate.identity is PythonIdentity.CURRENT_FRAME
                            and not candidate.contained
                        ):
                            candidates.add(_INTERPRETER_BUILTINS)
                        elif not candidate.contained and (
                            candidate.kind
                            in {"native_initializer", "bound_native_initializer"}
                            or candidate.value in self.declarations
                        ):
                            candidates.add(_INTERPRETER_BUILTINS)
                            candidates.update(
                                self.select_member(candidate, "__builtins__", value)
                            )
                    elif candidate.contained:
                        # Exposed provenance is transitive may-containment, not
                        # an exact container depth. A selector can yield the
                        # value itself or another container still holding it.
                        candidates.add(candidate)
                        candidates.add(candidate._replace(contained=False))
                    else:
                        match element.protocol:
                            case PythonElementProtocol.SUBSCRIPT:
                                if candidate.kind == "namespace":
                                    candidates.update(
                                        self._namespace_values(
                                            candidate, value, element.key
                                        )
                                    )
                                elif candidate.kind == "instance":
                                    member = (
                                        candidate.module,
                                        SourceMember(candidate, "__getitem__"),
                                    )
                                    candidates.update(
                                        self.watch(
                                            (candidate.module, SourceReturn(member)),
                                            value,
                                        )
                                    )
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
                                | PythonElementProtocol.REFLECTED_BUILTINS
                                | PythonElementProtocol.REFLECTED_GLOBALS
                                | PythonElementProtocol.MAPPING_WRITE
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
                    self.watch((module_name, operation((module_name, owner))), value)
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
                        candidates.update(self._returned_values(candidate, value))
                        continue
                    if candidate.identity is not None:
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
                            for initializer in self.select_member(
                                candidate._replace(kind="instance"), "__init__", value
                            ):
                                if initializer.kind in {
                                    "native_initializer",
                                    "bound_native_initializer",
                                }:
                                    self._bind_native_call(value, initializer)
                            candidates.add(
                                SourceCandidate(*candidate.value, "instance")
                            )
                            for member in self.select_member(
                                candidate, "__new__", value
                            ):
                                candidates.update(self._returned_values(member, value))
                        else:
                            for member in self.select_member(
                                candidate, "__call__", value
                            ):
                                candidates.update(self._returned_values(member, value))
                    else:
                        candidates.update(self._returned_values(candidate, value))
        elif value in self.attributes:
            for name, owners in self.attributes[value].items():
                if name in {"__builtins__", "f_builtins"}:
                    # Native metadata can be visible without a source callable.
                    candidates.add(_INTERPRETER_BUILTINS)
                for owner in sorted(owners):
                    candidates.update(
                        self._select_attribute((module_name, owner), name, value)
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
        known = self.value_candidates.setdefault(value, set())
        added = candidates - known
        known.update(added)
        if added:
            if writes := self.attribute_writes.get(value):
                self._record_attribute_receivers(module_name, writes, added)
            for users in (self.value_users, self.candidate_users):
                for user in users.get(value, ()):
                    self.queue_value(user)
        # Incremental join: each candidate/demand incidence is emitted once.
        # A namespace dependency wake with neither delta performs no old product.
        new_phases = self.pending_phases.pop(value, set())
        old_phases = (self.value_phases.get(value, set()) - new_phases) if added else ()
        for candidate in added:
            for incidence in old_phases:
                self._activate_demand(value, candidate, incidence)
        if new_phases:
            for candidate in tuple(known):
                for incidence in new_phases:
                    self._activate_demand(value, candidate, incidence)

    def _route_call_arguments(
        self, module: str, inputs: SourceCallArguments, user: SourceValue
    ) -> None:
        # Return values can grow without changing a call's actuals. Watch
        # operand and declaration changes from the argument transfer alone.
        site = (module, inputs.site)
        if site in self.import_bindings:
            return
        call = self.calls[site]
        seen: set[tuple[SourceValue, bool]] = set()
        actuals: dict[tuple[int, bool], set[SourceCandidate]] = {}
        names = tuple(name for name, _, _ in call.arguments)

        def values(index: int, packed: bool) -> set[SourceCandidate]:
            key = (index, packed)
            selected = actuals.get(key)
            if selected is not None:
                return selected
            name, direct, exposed = call.arguments[index]
            if name is None:
                direct = exposed
            selected = {
                candidate._replace(contained=packed or source not in direct)
                for source in exposed
                for candidate in self._watch_candidates((user[0], source), user)
            }
            actuals[key] = selected
            return selected

        for origin in call.callees:
            selection = (module, origin)
            for candidate in self._watch_candidates(selection, user):
                if (
                    candidate.contained
                    or candidate.identity is not None
                    or candidate.source is None
                    or candidate.kind.startswith("namespace")
                ):
                    continue
                bound = candidate in self.receiver_bound_members.get(selection, ())
                unbound = not bound or candidate in self.unbound_members.get(
                    selection, ()
                )
                offsets = (
                    (True,)
                    if candidate.kind == "bound_class_method" or not unbound
                    else (False, True)
                    if bound
                    else (False,)
                )
                for offset in offsets:
                    route = (candidate.value, offset)
                    if route not in seen:
                        seen.add(route)
                        self._bind_actuals(names, values, candidate.value, user, offset)

    def _bind_actuals(
        self,
        names: tuple[int | str | None, ...],
        candidates: Callable[[int, bool], set[SourceCandidate]],
        callee: SourceValue,
        user: SourceValue,
        receiver_bound: bool,
    ) -> None:
        self.declaration_users.setdefault(callee, set()).add(user)
        parameters = self.declarations.get(callee)
        if parameters is None:
            return
        keywords = dict(parameters.keywords)
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
            for target, packed in targets:
                values = candidates(index, packed)
                if not values:
                    continue
                formal = (callee[0], target)
                known = self.arguments.setdefault(formal, set())
                if added := values - known:
                    known.update(added)
                    self.queue_value(formal)

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
                            formal = (method.module, target)
                            arguments = self.arguments.setdefault(formal, set())
                            selected = {
                                candidate._replace(
                                    contained=contained or candidate.contained
                                )
                                for candidate in values
                            }
                            if selected - arguments:
                                arguments.update(selected)
                                self.queue_value(formal)
                self.activate(method, "call")

    def _function_kind(self, callee: SourceCandidate) -> FunctionKind:
        declaration = self.declarations.get(callee.value)
        return FunctionKind.SYNC if declaration is None else declaration.kind

    def _returned_values(
        self, callee: SourceCandidate, user: SourceValue
    ) -> set[SourceCandidate]:
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
        if callee.identity is not None or callee.kind in {
            "namespace_clear",
            "namespace_write_descriptor",
            "namespace_write_method",
            "resumable",
            "class_descriptor",
            "property_descriptor",
        }:
            return set()
        if self._function_kind(callee) != FunctionKind.SYNC:
            value = callee.value
            self.watch(value, user)
            return {SourceCandidate(*value, "resumable")}
        return self._function_values(callee, user)

    def _function_values(
        self, callee: SourceCandidate, user: SourceValue, *, yielded: bool = False
    ) -> set[SourceCandidate]:
        value = callee.value
        self.watch(value, user)
        function = self.functions.get(value)
        if function is None:
            return set()
        returned, exposed = (
            (function.yielded, function.yielded_exposed)
            if yielded
            else (function.returned, function.returned_exposed)
        )
        result: set[SourceCandidate] = set()
        for source in returned:
            result.update(self.watch((callee.module, source), user))
        for source in exposed - returned:
            result.update(
                candidate._replace(contained=True)
                for candidate in self.watch((callee.module, source), user)
            )
        return result

    def _awaited_values(
        self, awaited: SourceCandidate, user: SourceValue
    ) -> set[SourceCandidate]:
        if awaited.contained:
            return set()
        if awaited.kind == "resumable":
            if self._function_kind(awaited) == FunctionKind.ASYNC:
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
        return set()

    def _generator_completion_values(
        self, generator: SourceCandidate, user: SourceValue
    ) -> set[SourceCandidate]:
        if (
            not generator.contained
            and generator.kind == "resumable"
            and self._function_kind(generator) == FunctionKind.GENERATOR
        ):
            return self._function_values(generator, user)
        return set()

    @staticmethod
    def _iteration_arities(call: PythonSourceCall) -> tuple[bool, bool]:
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
        unary, binary = self._iteration_arities(call)
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
            for argument in operands.values():
                for receiver in argument:
                    if identity is PythonIdentity.BUILTIN_REVERSED:
                        if receiver.kind == "instance" and not receiver.contained:
                            for method in ("__reversed__", "__len__"):
                                self.demand(
                                    (receiver.module, SourceMember(receiver, method)),
                                    "call",
                                )
                    else:
                        self.activate(receiver, "get_iter", consumer=module)
            return {
                SourceCandidate(
                    module, call.owner, "iterator", iterator_producer=identity
                )
            }

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
        unary, binary = self._iteration_arities(call)
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
                    result.update(self._returned_values(callback, user))
            if items.producer is PythonIdentity.BUILTIN_MAP:
                return result
            # Filter only returns elements from the original iterable.
        result = set().union(*operands)
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
    ) -> set[SourceCandidate]:
        if iterator.contained:
            return set()
        if iterator.kind == "iterator" and not asynchronous:
            return set(self.watch(self._iterator_contents(iterator), user))
        expected = (
            FunctionKind.ASYNC_GENERATOR if asynchronous else FunctionKind.GENERATOR
        )
        if iterator.kind == "resumable" and self._function_kind(iterator) == expected:
            return self._function_values(iterator, user, yielded=True)
        if iterator.kind == "instance":
            name = "__anext__" if asynchronous else "__next__"
            member = (iterator.module, SourceMember(iterator, name))
            item = (iterator.module, SourceReturn(member))
            if asynchronous:
                item = (iterator.module, SourceAwait(item))
            return set(self.watch(item, user))
        return set()

    def _iterated_values(
        self, iterable: SourceCandidate, user: SourceValue, *, asynchronous: bool
    ) -> set[SourceCandidate]:
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
