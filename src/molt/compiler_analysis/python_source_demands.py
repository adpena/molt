from __future__ import annotations

from molt.compiler_analysis.python_function_kind import FunctionKind
from molt.compiler_analysis.python_binding_facts import (
    PythonFunctionResult,
    PythonSourceCall,
    PythonSourceParameters,
    PythonSourceElement,
    PythonElementProtocol,
)

from collections import deque
from collections.abc import Callable
from typing import Literal, NamedTuple, assert_never, cast

from molt.compiler_analysis.python_imports import (
    SourceAllExports,
    SourceDemandPhase,
    _ImportScanRequests,
    _SourceImportBinding,
    SourceImportCall,
    normalize_stdlib_module_name,
)
from molt.compiler_analysis.python_source_keys import PythonSourceKey
from molt.compiler_analysis.python_identity_catalog_generated import (
    PythonIdentity,
    INTRINSIC_ACQUISITION_IDENTITIES,
)
from molt.compiler_analysis.python_value_identity import (
    SOURCE_CALLABLE_IDENTITIES,
    possible_runtime_import_call_identities,
)
from molt.compiler_analysis.static_truth import (
    context_execution_protocol,
)


class SourceCandidate(NamedTuple):
    module: str
    source: PythonSourceKey | None
    kind: Literal["value", "instance", "resumable"] = "value"
    contained: bool = False
    identity: PythonIdentity | None = None

    @property
    def value(self) -> tuple[str, PythonSourceKey | None]:
        return self.module, self.source


class SourceMember(NamedTuple):
    owner: SourceCandidate
    name: str


class SourceReturn(NamedTuple):
    callee: SourceValue


class SourceAwait(NamedTuple):
    value: SourceValue


class SourceGeneratorCompletion(NamedTuple):
    value: SourceValue


SourceValue = tuple[
    str,
    PythonSourceKey
    | SourceMember
    | SourceReturn
    | SourceAwait
    | SourceGeneratorCompletion
    | None,
]


class SourceDemand(NamedTuple):
    phase: SourceDemandPhase
    arguments: tuple[str, ...]
    argument_count: int | None
    consumer: str


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
        self.modules: set[str] = set()
        self.full_modules: set[str] = set()
        self.imported_modules: set[str] = set()
        self.value_candidates: dict[SourceValue, set[SourceCandidate]] = {}
        self.value_users: dict[SourceValue, set[SourceValue]] = {}
        self.value_phases: dict[SourceValue, set[SourceDemand]] = {}
        self.pending_phases: dict[SourceValue, set[SourceDemand]] = {}
        self.elements: dict[SourceValue, PythonSourceElement] = {}
        self.calls: dict[SourceValue, PythonSourceCall] = {}
        self.callables: dict[SourceValue, int] = {}
        self.call_imports: dict[SourceValue, frozenset[SourceImportCall]] = {}
        self.admitted_call_imports: set[tuple[SourceValue, PythonIdentity, str]] = set()
        self.completions: dict[SourceValue, frozenset[PythonSourceKey]] = {}
        self.parameters: dict[SourceValue, PythonSourceParameters] = {}
        self.arguments: dict[SourceValue, set[SourceCandidate]] = {}
        self.functions: dict[SourceValue, PythonFunctionResult] = {}
        self.pending_modules: dict[str, set[SourceValue]] = {}
        self.star_aliases: dict[str, set[PythonSourceKey]] = {}
        self.escaped_modules: dict[str, set[SourceDemand]] = {}
        self.namespaces: dict[
            SourceValue,
            tuple[dict[str, frozenset[PythonSourceKey]], frozenset[PythonSourceKey]],
        ] = {}
        self.bases: dict[SourceValue, frozenset[PythonSourceKey]] = {}
        self.attributes: dict[SourceValue, tuple[str, frozenset[PythonSourceKey]]] = {}
        self.import_bindings: dict[SourceValue, _SourceImportBinding] = {}
        self.value_jobs: set[SourceValue] = set()
        self.body_jobs: set[tuple[str, PythonSourceKey, tuple[str, ...]]] = set()
        self.activated: set[
            tuple[
                SourceCandidate,
                SourceDemandPhase,
                tuple[str, ...],
                int | None,
                str,
            ]
        ] = set()
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

    def accept(
        self, module_name: str, scan: _ImportScanRequests, *, full: bool = False
    ) -> None:
        self.modules.add(module_name)
        if self.exports.get(module_name) != scan.all_exports:
            self.exports[module_name] = scan.all_exports
            self._invalidate_value((module_name, None))
        if full:
            self.full_modules.add(module_name)
        self.imported_modules.update(
            (*scan.imports, *scan.dynamic_relative_import_candidates)
        )
        for owner, names, exposure in scan.source_values.namespaces:
            value = (module_name, None if owner[-1] == "Module" else owner)
            previous, previous_exposure = self.namespaces.get(value, ({}, frozenset()))
            namespace = dict(previous)
            for name, origins in names:
                namespace[name] = namespace.get(name, frozenset()) | origins
            if self._publish(
                self.namespaces, value, (namespace, previous_exposure | exposure)
            ):
                old_origins = previous_exposure.union(*previous.values())
                added = exposure.union(*namespace.values()) - old_origins
                for incidence in tuple(self.value_phases.get(value, ())):
                    if incidence.phase in {"escape", "expose", "descriptor"}:
                        for source in added:
                            self.demand(
                                (module_name, source),
                                incidence.phase,
                                incidence.arguments,
                                consumer=incidence.consumer,
                            )
        for owner, bases in scan.source_values.bases:
            value = (module_name, owner)
            self._publish(self.bases, value, self.bases.get(value, frozenset()) | bases)
        for owner, name, origins in scan.source_values.attributes:
            value = (module_name, owner)
            previous = self.attributes.get(value)
            self._publish(
                self.attributes,
                value,
                (
                    name,
                    origins | (previous[1] if previous is not None else frozenset()),
                ),
            )
        for element in scan.source_values.elements:
            value = (module_name, element.owner)
            previous = self.elements.get(value)
            if previous is not None:
                if previous.protocol != element.protocol:
                    raise ValueError("source element changed protocol")
                element = element._replace(origins=previous.origins | element.origins)
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
                call = call._replace(
                    callees=call.callees | previous.callees,
                    intrinsic_result=call.intrinsic_result | previous.intrinsic_result,
                    arguments=tuple(
                        (name, origins | prior[1], exposed | prior[2])
                        for (name, origins, exposed), prior in zip(
                            call.arguments, previous.arguments, strict=True
                        )
                    ),
                )
            self._publish(self.calls, value, call)
        for source, origins in scan.source_values.completions:
            value = (module_name, source)
            self._publish(
                self.completions,
                value,
                self.completions.get(value, frozenset()) | origins,
            )
        for parameters in scan.source_values.parameters:
            self._publish(self.parameters, (module_name, parameters.owner), parameters)
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
            if binding.attribute == "*":
                self.star_aliases.setdefault(module_name, set()).add(binding.source)
                for incidence in self.escaped_modules.get(module_name, ()):
                    self.demand(
                        key,
                        incidence.phase,
                        incidence.arguments,
                        consumer=incidence.consumer,
                    )
        for request in scan.source_demands:
            self.demand(
                (module_name, request.source),
                request.phase,
                argument_count=request.argument_count,
                consumer=module_name,
            )
        for value in self.pending_modules.pop(module_name, ()):
            self.queue_value(value)

    def drain(self) -> None:
        while self.queue:
            self.process_value(self.queue.popleft())

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
    ):
        if owner.identity is not None:
            return set()
        owner_module, owner_source = owner.value
        result = set()
        if owner.contained or owner in member_visiting:
            return result
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
        namespace, _exposure = self.namespaces.get(owner.value, ({}, frozenset()))
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
                result.update(self.watch((owner_module, origin), user))
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
                            inherited, name, user, member_visiting=member_visiting
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
        if phase in {"read", "factory"} or incidence in self.activated:
            return
        self.activated.add(incidence)
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
        if origin.contained:
            if phase in {"escape", "release", "expose", "protocol", "descriptor"}:
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
                namespace, _exposure = self.namespaces.get(
                    origin.value, ({}, frozenset())
                )
                for source in _exposure.union(*namespace.values()):
                    self.demand((module_name, source), phase, arguments)
            return
        if phase == "protocol" and origin.kind != "instance":
            return
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
            namespace, exposure = self.namespaces.get(origin.value, ({}, frozenset()))
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
            for values in namespace.values():
                for value in values:
                    self.demand(
                        (module_name, value),
                        namespace_phase,
                    )
            for value in exposure:
                self.demand(
                    (module_name, value),
                    namespace_phase,
                )
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
            if (
                module_name not in self.full_modules
                and (module_name, owner, arguments) not in self.body_jobs
            ):
                self.body_jobs.add((module_name, owner, arguments))
                self.queue_body(module_name, owner, arguments)

    def process_value(self, value: SourceValue) -> None:
        self.value_jobs.discard(value)
        module_name, source = value
        if module_name not in self.modules:
            self.pending_modules.setdefault(module_name, set()).add(value)
            return
        candidates = (
            {
                SourceCandidate(
                    module_name, cast(PythonSourceKey, source), identity=identity
                )
                for identity in PythonIdentity
                if self.callables.get(value, 0) & int(identity)
            }
            if isinstance(source, tuple)
            else set()
        )
        if isinstance(source, SourceReturn):
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
                    parameters = self.parameters.get(callee.value)
                    positional = parameters.positional if parameters is not None else ()
                    vararg = parameters.vararg if parameters is not None else None
                    parameter = positional[0] if positional else vararg
                    if parameter is not None:
                        formal = (callee.module, parameter)
                        argument = source.owner._replace(contained=not positional)
                        known = self.arguments.setdefault(formal, set())
                        if argument not in known:
                            known.add(argument)
                            self.queue_value(formal)
        elif source is not None and source[-1] == "arg":
            candidates.update(self.arguments.get(value, ()))
        elif source is None or source[-1] == "Module":
            candidates.add(SourceCandidate(module_name, None))
        elif source[-1] in {
            "FunctionDef",
            "AsyncFunctionDef",
            "Lambda",
            "ClassDef",
            "TypeAlias",
            "GeneratorExp",
        }:
            candidates.add(
                SourceCandidate(
                    module_name,
                    source,
                    "resumable" if source[-1] == "GeneratorExp" else "value",
                )
            )
        elif value in self.elements:
            element = self.elements[value]
            for owner in element.origins:
                for candidate in tuple(self.watch((module_name, owner), value)):
                    if candidate.contained:
                        candidates.add(candidate._replace(contained=False))
                    else:
                        match element.protocol:
                            case PythonElementProtocol.SUBSCRIPT:
                                if candidate.kind == "instance":
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
            for callee in call.callees:
                for candidate in tuple(self.watch((module_name, callee), value)):
                    if candidate.contained:
                        continue
                    if candidate.identity is not None:
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
                                for identity in PythonIdentity
                                if int(identity)
                                & call.intrinsic_result
                                & SOURCE_CALLABLE_IDENTITIES
                            )
                        continue
                    if candidate.source is None:
                        continue
                    self._bind_call_arguments(call, candidate, value)
                    if candidate.source[-1] == "ClassDef":
                        if candidate.kind == "value":
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
            name, owners = self.attributes[value]
            for owner in owners:
                for candidate in tuple(self.watch((module_name, owner), value)):
                    candidates.update(self.select_member(candidate, name, value))
        elif value in self.import_bindings:
            binding = self.import_bindings[value]
            for imported in binding.modules:
                normalized = normalize_stdlib_module_name(imported)
                if imported not in self.modules and normalized in self.modules:
                    imported = normalized
                for candidate in tuple(self.watch((imported, None), value)):
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
            for user in self.value_users.get(value, ()):
                self.queue_value(user)
        # Incremental join: each candidate/demand incidence is emitted once.
        # A namespace dependency wake with neither delta performs no old product.
        new_phases = self.pending_phases.pop(value, set())
        old_phases = (self.value_phases.get(value, set()) - new_phases) if added else ()
        for candidate in added:
            for incidence in old_phases:
                self.activate(
                    candidate,
                    incidence.phase,
                    incidence.arguments,
                    argument_count=incidence.argument_count,
                    consumer=incidence.consumer,
                )
        if new_phases:
            for candidate in tuple(known):
                for incidence in new_phases:
                    self.activate(
                        candidate,
                        incidence.phase,
                        incidence.arguments,
                        argument_count=incidence.argument_count,
                        consumer=incidence.consumer,
                    )

    def _bind_call_arguments(
        self, call: PythonSourceCall, callee: SourceCandidate, user: SourceValue
    ) -> None:
        """Transport argument origins through the existing formal-value worklist."""
        self.watch(callee.value, user)
        parameters = self.parameters.get(callee.value)
        if parameters is None:
            return
        keywords = dict(parameters.keywords)
        for name, origins, exposed in call.arguments:
            if isinstance(name, int):
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
                origins = exposed
            for target, contained in targets:
                formal = (callee.module, target)
                known = self.arguments.setdefault(formal, set())
                for source in exposed:
                    for candidate in tuple(self.watch((user[0], source), user)):
                        argument = candidate._replace(
                            contained=contained
                            or (source not in origins and name is not None)
                        )
                        if argument not in known:
                            known.add(argument)
                            self.queue_value(formal)

    def _function_kind(self, callee: SourceCandidate) -> FunctionKind:
        result = self.functions.get(callee.value)
        return FunctionKind.SYNC if result is None else result.kind

    def _returned_values(
        self, callee: SourceCandidate, user: SourceValue
    ) -> set[SourceCandidate]:
        if callee.identity is not None or callee.kind == "resumable":
            return set()
        self.watch(callee.value, user)
        if self._function_kind(callee) != FunctionKind.SYNC:
            return {SourceCandidate(*callee.value, "resumable")}
        return self._function_values(callee, user)

    def _function_values(
        self, callee: SourceCandidate, user: SourceValue, *, yielded: bool = False
    ) -> set[SourceCandidate]:
        self.watch(callee.value, user)
        function = self.functions.get(callee.value)
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

    def _iterated_values(
        self, iterable: SourceCandidate, user: SourceValue, *, asynchronous: bool
    ) -> set[SourceCandidate]:
        expected = (
            FunctionKind.ASYNC_GENERATOR if asynchronous else FunctionKind.GENERATOR
        )
        if iterable.kind == "resumable":
            if self._function_kind(iterable) == expected:
                return self._function_values(iterable, user, yielded=True)
        elif iterable.kind == "instance":
            member = (
                iterable.module,
                SourceMember(iterable, "__aiter__" if asynchronous else "__iter__"),
            )
            returned = (iterable.module, SourceReturn(member))
            result: set[SourceCandidate] = set()
            for iterator in tuple(self.watch(returned, user)):
                if (
                    iterator.kind == "resumable"
                    and self._function_kind(iterator) == expected
                ):
                    result.update(self._function_values(iterator, user, yielded=True))
                elif iterator.kind == "instance":
                    advance = (
                        iterator.module,
                        SourceMember(
                            iterator, "__anext__" if asynchronous else "__next__"
                        ),
                    )
                    item = (iterator.module, SourceReturn(advance))
                    if asynchronous:
                        item = (iterator.module, SourceAwait(item))
                    result.update(self.watch(item, user))
            if not asynchronous:
                sequence = (iterable.module, SourceMember(iterable, "__getitem__"))
                result.update(
                    self.watch((iterable.module, SourceReturn(sequence)), user)
                )
            return result
        return set()
