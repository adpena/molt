"""Canonical source-ordered Python lexical binding and capability analysis.

This module is deliberately consumer-neutral.  It performs the control-flow and
lexical-identity work once, then exposes immutable facts to import discovery,
frontend lowering, and future semantic passes.  It never executes user code and
fails closed whenever Python can re-enter through reflection, imports, reference
release, descriptors, iteration, context managers, or comparison callbacks.
"""

from __future__ import annotations

from molt.compiler_analysis.python_function_kind import FunctionKind
from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION

import ast
from molt.python_canonical_ast import canonical_python_module
from molt.python_private_names import python_import_binding
from bisect import bisect_left
from collections import OrderedDict, deque
from collections.abc import (
    Callable,
    Generator,
    Iterable,
    Iterator,
    Mapping,
)
from contextlib import contextmanager
from contextvars import ContextVar
from copy import copy
from dataclasses import dataclass, field, fields, replace
from itertools import zip_longest
from threading import Event, RLock
from types import MappingProxyType
from typing import TYPE_CHECKING, Final, Literal, Sequence, assert_never, cast, overload

if TYPE_CHECKING:
    from _collections_abc import dict_items, dict_keys, dict_values

from molt.compiler_analysis.python_identity_catalog_generated import (
    BUILTIN_RETURN_CAPTURES,
    COMPILER_INTRINSIC_CALLABLES,
    ALL_INTRINSIC_CALLABLE_IDENTITIES,
    IMPORT_CALLABLES,
    INTRINSIC_ACQUISITION_IDENTITIES,
    OPTIONAL_INTRINSIC_ACQUISITION_IDENTITIES,
    ALL_INVALID_MEMBERS,
    ANY_CANONICAL_MODULE_IDENTITY,
    BUILTIN_IDENTITIES,
    BUILTIN_SHAPE_IDENTITIES,
    BUILTIN_SHAPE_NAMES_BY_IDENTITY,
    CANONICAL_FROM_IMPORT_IDENTITIES,
    CANONICAL_IMPORT_IDENTITIES,
    CANONICAL_MEMBER_IDENTITIES,
    CANONICAL_MEMBER_WRITES,
    CANONICAL_WILDCARD_WRITES,
    INTRINSIC_CALLABLE_IDENTITIES,
    INTRINSIC_TARGET_PROPERTIES,
    INTRINSIC_ATTRIBUTE_READS,
    INTRINSIC_NAMESPACE_ENTRY_READS,
    INTRINSIC_NATIVE_INITIALIZERS,
    PythonNativeInitializer,
    DataclassInitSpecField,
    IdentityMask,
    MemberMask,
    NO_IDENTITIES,
    NO_INVALID_MEMBERS,
    OTHER_IDENTITY,
    PythonIdentity,
    PythonMember,
    PythonTargetProperty,
    UNBOUND_IDENTITY,
)
from molt.compiler_analysis.python_call_arguments import (
    CallArgumentPlan,
    call_argument_plan,
)
from molt.compiler_analysis.python_parameter_classes import (
    CallEvidence,
)
from molt.compiler_analysis.python_namespace_writes import (
    _own_namespace_names,
)
from molt.compiler_analysis.python_string_facts import (
    PythonStringBound,
    python_string_spelling,
)
from molt.compiler_analysis.python_source_keys import (
    PythonElementProtocol,
    PythonSourceKey,
    PythonSourceProjection,
    python_source_projection_key,
    _PythonAstDigestAdmission,
    python_source_digest,
    python_node_source_key,
)

from molt.compiler_analysis.python_binding_facts import (
    MODULE_IMPORT_METADATA_NAMES,
    PythonBindingFacts,
    PythonSourceArgument,
    PythonSourceCall,
    PythonSourceCallDispatch,
    PythonNativeInitializerSource,
    PythonSourceClass,
    PythonSourceCallableDeclaration,
    PythonBindingIndex,
    PythonBindingTelemetry,
    PythonCallSiteFact,
    PythonCompletion,
    PythonCompletionFlow,
    PythonExpressionFact,
    PythonFunctionResult,
    PythonActivationPartition,
    PythonClassPreparation,
    PythonSourceElement,
    PythonIterationFact,
    PythonStringAlternatives,
    finite_python_strings,
    join_python_static_values,
    PythonStringValue,
    PythonModuleMetadataProof,
    NO_MODULE_METADATA_PROOF,
    PythonNameLookup,
    PythonParameterRef,
    PythonScopeFact,
    PythonStatementFact,
    PythonStaticValue,
    current_globals_dict_is_exact,
    globals_mutation_call_identity,
    python_static_value_key,
    same_python_static_value,
)
from molt.compiler_analysis.python_value_identity import (
    NATIVE_SELECTOR_CONSTRUCTORS,
    SOURCE_CALLABLE_IDENTITIES,
    exact_identity,
    possible_identity,
    subscription_is_inert_alias,
)
from molt.compiler_analysis.literal_identity import literal_identity_key
from molt.compiler_analysis.builtin_binding_guards_generated import (
    BUILTIN_BINDING_GUARD_BITS,
)
from molt.compiler_analysis.python_builtin_shapes import (
    MappingWriteScope,
    builtin_call_shape,
    builtin_mapping_write_scope,
    builtin_method_call_shape,
    builtin_method_descriptor_known,
    builtin_method_writes_receiver,
    literal_mapping_update_keys,
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
    NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS,
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
    ClassCallProof,
    ExpressionSequenceItem,
    StaticExpressionResult,
    StaticSubscriptionShape,
    SourceDemandPhase,
    UNKNOWN_EXPRESSION_RESULT,
    expression_result_for_owned_binding,
    expression_result_without_mutable_contents,
    expression_result_without_value_facts,
    expression_result_for_publication,
    join_static_expression_results,
    DeferredExecution,
    static_comparison_result,
    static_binary_result,
    static_expression_result,
    static_subscription_shape,
    static_unpack_results,
)
from molt.compiler_analysis.python_effects import (
    AccumulatedKeyEffects,
    binary_operation_effects,
    unary_operation_effects,
    iterable_unpack_effects,
    mapping_unpack_effects,
)
from molt.compiler_analysis.python_imports import (
    ModuleImportContext,
    import_metadata_target_name,
    import_call_fallback_operand,
    is_compiler_intrinsics_module,
)
from molt.compiler_analysis.python_lexical_scope import (
    PythonDependencyAuthority,
    PythonModuleLexicalPlan,
    PythonScopeDeclarations,
    function_annotation_expressions,
    function_parameter_names,
    function_parameter_nodes,
    type_parameter_expressions,
)
from molt.compiler_analysis.python_source_keys import (
    python_pattern_capture_names,
    python_pattern_irrefutable_reason,
    python_pattern_is_capture_only,
)

# Fixed identity masks as plain ints: hot paths test them per call, and
# IntFlag arithmetic builds a pseudo-member on every composition.
# Builtins that execute source code.
_CODE_EXECUTION_CALL_IDENTITIES: Final = int(
    PythonIdentity.BUILTIN_EVAL | PythonIdentity.BUILTIN_EXEC
)
# Callables that import a module named at run time.
_DYNAMIC_IMPORT_CALL_IDENTITIES: Final = int(
    PythonIdentity.BUILTINS_IMPORT | PythonIdentity.IMPORTLIB_IMPORT_MODULE
)
# Modules that own a dynamic import callable.
_DYNAMIC_IMPORT_MODULE_IDENTITIES: Final = int(
    PythonIdentity.BUILTINS_MODULE | PythonIdentity.IMPORTLIB_MODULE
)
# Values that read or expose a frame or module namespace.
_FRAME_NAMESPACE_IDENTITIES: Final = int(
    PythonIdentity.CURRENT_MODULE
    | PythonIdentity.CURRENT_GLOBALS
    | PythonIdentity.CURRENT_LOCALS
    | PythonIdentity.CURRENT_FRAME
    | PythonIdentity.BUILTIN_GLOBALS
    | PythonIdentity.BUILTIN_LOCALS
    | PythonIdentity.BUILTIN_VARS
)
# Callables that run the import machinery.
_IMPORT_MACHINERY_CALL_IDENTITIES: Final = int(
    PythonIdentity.BUILTINS_IMPORT
    | PythonIdentity.IMPORTLIB_IMPORT_MODULE
    | PythonIdentity.IMPORTLIB_FIND_SPEC
)
# Builtins that return the calling frame's locals.
_LOCALS_READER_CALL_IDENTITIES: Final = int(
    PythonIdentity.BUILTIN_LOCALS | PythonIdentity.BUILTIN_VARS
)
# The current module or an unknown value.
_MODULE_OR_UNKNOWN_IDENTITIES: Final = int(
    PythonIdentity.CURRENT_MODULE | PythonIdentity.OTHER
)
# Builtins that return a namespace mapping.
_NAMESPACE_READER_CALL_IDENTITIES: Final = int(
    PythonIdentity.BUILTIN_GLOBALS
    | PythonIdentity.BUILTIN_LOCALS
    | PythonIdentity.BUILTIN_VARS
)
# Facts whose release cannot run user code.
_RELEASE_ROOTED_IDENTITIES: Final = int(
    PythonIdentity.CURRENT_GLOBALS
    | PythonIdentity.STATIC_FALSE
    | PythonIdentity.UNBOUND
)
# Callables defined by user code.
_USER_CALLABLE_IDENTITIES: Final = int(
    PythonIdentity.USER_FUNCTION | PythonIdentity.USER_CLASS
)


_ANALYSIS_SCHEMA: Final = 90
_BUILTIN_SHAPE_IDENTITY_MASK: Final[IdentityMask] = sum(BUILTIN_SHAPE_NAMES_BY_IDENTITY)
_RELEASE_CALLBACK_EFFECTS: Final[EffectMask] = (
    RELEASES_REFERENCE | RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK
)
# A descriptor boundary can also retire operands. Other callback categories
# (including the unknown mask) require the general object protocol instead.
_NON_DESCRIPTOR_CALLBACK_EFFECTS: Final[EffectMask] = (
    NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
    & ~(INVOKES_DESCRIPTOR | EXECUTES_ARBITRARY_PYTHON | _RELEASE_CALLBACK_EFFECTS)
)
# Scopes whose own locals the frontend binds in a running frame's homes.
_FRAME_LOCAL_SCOPE_KINDS: Final[frozenset[str]] = frozenset(
    {"function", "lambda", "comprehension"}
)
_CALLEE_ELISION_FORBIDDEN_EFFECTS: Final[EffectMask] = (
    EXECUTES_ARBITRARY_PYTHON
    | INVOKES_COMPARISON_CALLBACK
    | INVOKES_CONTEXT_CALLBACK
    | INVOKES_DESCRIPTOR
    | INVOKES_IMPORT_SYSTEM
    | INVOKES_ITERATION_CALLBACK
    | READS_FRAME_STATE
    | READS_GLOBAL_NAMESPACE
    | REFLECTS_NAMESPACE
    | RUNS_FINALIZER
    | RUNS_WEAKREF_CALLBACK
    | SUSPENDS
    | WRITES_FRAME_STATE
    | WRITES_GLOBAL_NAMESPACE
    | WRITES_MODULE_METADATA
    | WRITES_OBJECT_STATE
)
_CALLEE_RETENTION_FORBIDDEN_EFFECTS: Final[EffectMask] = (
    _CALLEE_ELISION_FORBIDDEN_EFFECTS
    & ~(READS_FRAME_STATE | READS_GLOBAL_NAMESPACE | REFLECTS_NAMESPACE)
)


def _canonical_member_declarations(
    base: IdentityMask, member: str
) -> Iterator[tuple[PythonIdentity, PythonIdentity, PythonMember]]:
    """Use the same identity/guard declarations for acquisition and mutation."""
    for owner, identity, guard in CANONICAL_MEMBER_IDENTITIES.get(member, ()):
        if base & int(owner):
            yield owner, identity, guard


def _member_write_mask(base: IdentityMask, member: str) -> MemberMask:
    """Canonical members a write of ``member`` on ``base`` can change."""
    members = 0
    for owner, guards in CANONICAL_MEMBER_WRITES.get(member, ()):
        if base & int(owner):
            members |= guards
    for owner, guards in CANONICAL_WILDCARD_WRITES:
        if base & int(owner):
            members |= guards
    return members


def _canonical_spelling_identity(module: str) -> IdentityMask:
    """The canonical identity a dotted module or member spelling names."""
    identity = CANONICAL_IMPORT_IDENTITIES.get(module)
    if identity is not None:
        return int(identity)
    owner, _, member = module.rpartition(".")
    declaration = CANONICAL_FROM_IMPORT_IDENTITIES.get((owner, member))
    return int(declaration[0]) if declaration is not None else NO_IDENTITIES


def python_canonical_member_writes(module: str | None, name: str | None) -> MemberMask:
    """Canonical members a summarized namespace write can change.

    ``module`` is a write's dotted receiver spelling, ``None`` for a namespace
    reached by a computed ``sys.modules`` key; ``name`` is ``None`` when computed.
    Shares ``_member_write_mask`` with the flow's own direct stores.
    """
    base = (
        ANY_CANONICAL_MODULE_IDENTITY
        if module is None
        else _canonical_spelling_identity(module)
    )
    if base == NO_IDENTITIES:
        return NO_INVALID_MEMBERS
    if name is None:
        return ALL_INVALID_MEMBERS
    return _member_write_mask(base, name)


def python_dynamic_import_facts_required(
    tree: ast.Module, *, nodes: Iterable[ast.AST] | None = None
) -> bool:
    """Conservatively demand flow facts when an import-call identity has an origin.

    This is an absence proof over the binding authority's identity producers,
    not callee recognition. A positive result still requires the full fixpoint
    for aliases, rebinding, callbacks, deferred bodies and execution order.
    Namespace/frame access and unknown calls cannot introduce a canonical
    importer identity: they preserve source-owned identities or yield OTHER.
    Default traversal includes the complete AST; a source dependency consumer
    may supply the canonical eager-region projection for module-only policy.
    """
    call_identities = _DYNAMIC_IMPORT_CALL_IDENTITIES
    module_identities = _DYNAMIC_IMPORT_MODULE_IDENTITIES
    for node in ast.walk(tree) if nodes is None else nodes:
        if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Load):
            declaration = BUILTIN_IDENTITIES.get(node.id)
            if declaration is not None and int(declaration[0]) & call_identities:
                return True
        elif isinstance(node, ast.Import):
            for alias in node.names:
                module = alias.name if alias.asname else alias.name.split(".", 1)[0]
                if (
                    int(CANONICAL_IMPORT_IDENTITIES.get(module, PythonIdentity.OTHER))
                    & module_identities
                ):
                    return True
        elif isinstance(node, ast.ImportFrom):
            for alias in node.names:
                declaration = CANONICAL_FROM_IMPORT_IDENTITIES.get(
                    (node.module or "", alias.name)
                )
                if declaration is not None and int(declaration[0]) & (
                    call_identities | module_identities
                ):
                    return True
    return False


NO_CLOSED_GRAPH_FRAME_LOCALS_WRITER: Final = "no closed namespace graph"


def frame_locals_exposed(
    target_python: tuple[int, int], frame_locals_writer: str | None
) -> bool:
    """Whether a callback may rebind a running function's fast locals.

    PEP 667 (3.13) makes ``frame.f_locals`` a write-through proxy, so a
    callback can replace a caller's local only on a 3.13+ target and only
    when the closed program holds a possible writer (HF-901).
    """
    return target_python >= (3, 13) and frame_locals_writer is not None


@dataclass(frozen=True, slots=True)
class PythonBindingFlowPolicy:
    """Only assumptions that can change lexical fixpoint facts."""

    target_python: tuple[int, int] = _DEFAULT_TARGET_PYTHON_VERSION.feature_version
    target_sys_platform: str | None = None
    standard_imports_are_canonical: bool = True
    noncanonical_imports: tuple[str, ...] = ()
    # Bodies with one or more callers outside the proven argument graph.
    opaque_activation_sources: tuple[PythonSourceKey, ...] | None = None
    # Recreating an exposed function over other globals requires a code
    # rebinder. Standalone callers have no closed graph ruling one out.
    code_rebinding: bool = True
    analyze_deferred_bodies: bool = True
    # One source-owned activation may carry proven string arguments, including
    # a joined finite domain. The binding fixpoint owns their lifetime and flow;
    # consumers must not reconstruct those facts after flow analysis.
    activation_argument_values: tuple[
        tuple[PythonSourceKey, PythonStringValue], ...
    ] = ()
    # Spec 0216 admits scoped mutation and excludes unrestricted reflection,
    # exec/eval and arbitrary monkeypatching. A module's globals then change
    # outside its own statements only through named writes from other modules
    # and the import system's submodule bindings. A closed-graph consumer
    # supplies those names, sorted (``python_namespace_writes``); callbacks then
    # expire only them and the names this module's own deferred code rebinds.
    # None: any name may change, the only premise without a graph.
    foreign_namespace_writes: tuple[str, ...] | None = None
    # A source scan's view (``PythonNamespacePremise.scan_view``) keeps only
    # the foreign names its source can spell (``python_namespace_name_universe``)
    # so a write of any other name neither re-keys nor revises the scan. A name
    # outside this universe can reach the namespace only through a computed
    # key; the analysis then treats it as foreign-written, as the whole premise
    # might. None: ``foreign_namespace_writes`` is the whole premise.
    foreign_namespace_universe: frozenset[str] | None = None
    # Canonical members that code anywhere in that graph, this module included,
    # writes (``python_canonical_member_writes``). Callbacks change only these.
    callback_member_writes: MemberMask = ALL_INVALID_MEMBERS
    # A graph writer may have run before this module starts. This is separate
    # from future callbacks; standalone analysis keeps its initial premise.
    entry_invalid_members: MemberMask = NO_INVALID_MEMBERS
    # Producer semantics, independent of imports or namespace mutation masks.
    class_preparation: PythonClassPreparation = PythonClassPreparation.PYTHON
    # The program's first possible PEP 667 frame-locals writer
    # (``PythonNamespacePremise.frame_locals_writer``). On a 3.13+ target a
    # callback may rebind a running function's fast locals only while one
    # exists. A closed graph supplies None when no code can; standalone
    # analysis, with no graph, keeps the conservative premise.
    frame_locals_writer: str | None = NO_CLOSED_GRAPH_FRAME_LOCALS_WRITER
    declaration_only: tuple[PythonSourceKey, ...] = ()
    # Exact intrinsics whose only normal result on the linked runtime is None.
    normal_none_intrinsics: IdentityMask = 0
    # Analysis can also describe hosted Python, where the loader reports
    # whether it actually found a runtime.
    molt_runtime_linked: bool = False

    def __post_init__(self) -> None:
        for _parameter, value in self.activation_argument_values:
            if isinstance(value, PythonStringAlternatives) and (
                not value.values or any(type(item) is not str for item in value.values)
            ):
                raise ValueError(
                    "activation string alternatives require nonempty exact strings"
                )
        if type(self.class_preparation) is not PythonClassPreparation:
            raise TypeError("class preparation requires its closed execution model")
        if self.foreign_namespace_universe is not None and (
            self.foreign_namespace_writes is None
            or not self.foreign_namespace_universe.issuperset(
                self.foreign_namespace_writes
            )
        ):
            raise ValueError("a name universe bounds a closed foreign write set")


@dataclass(frozen=True, slots=True)
class PythonBindingPolicy(PythonBindingFlowPolicy):
    """Complete analysis identity, including the module-context projection."""

    module_name: str | None = None
    module_spec_name: str | None = None
    module_is_package: bool = False
    module_execution_kind: Literal["imported", "module", "script"] = "imported"
    include_import_discovery: bool = False

    def flow_policy(self) -> PythonBindingFlowPolicy:
        # The analyzer receives this narrower type, never module identity.
        return PythonBindingFlowPolicy(
            target_python=self.target_python,
            target_sys_platform=self.target_sys_platform,
            standard_imports_are_canonical=self.standard_imports_are_canonical,
            noncanonical_imports=self.noncanonical_imports,
            opaque_activation_sources=self.opaque_activation_sources,
            code_rebinding=self.code_rebinding,
            analyze_deferred_bodies=self.analyze_deferred_bodies,
            activation_argument_values=self.activation_argument_values,
            foreign_namespace_writes=self.foreign_namespace_writes,
            foreign_namespace_universe=self.foreign_namespace_universe,
            callback_member_writes=self.callback_member_writes,
            entry_invalid_members=self.entry_invalid_members,
            class_preparation=self.class_preparation,
            frame_locals_writer=self.frame_locals_writer,
            declaration_only=self.declaration_only,
            normal_none_intrinsics=self.normal_none_intrinsics,
            molt_runtime_linked=self.molt_runtime_linked,
        )


@dataclass(frozen=True, slots=True)
class _OpaqueActivationReads:
    present: tuple[frozenset[PythonSourceKey], ...]
    absent: frozenset[PythonSourceKey]

    def unchanged_under(self, opaque: frozenset[PythonSourceKey]) -> bool:
        return self.absent.isdisjoint(opaque) and all(
            not group.isdisjoint(opaque) for group in self.present
        )


@dataclass(frozen=True, slots=True)
class _ForeignNamespaceReads:
    names: frozenset[str]
    declaration_end: int
    callback_end: int
    tainted_at_exit: bool
    has_empty_module_expiry: bool


def _project_binding_slots(
    facts: PythonBindingFacts,
    slots: tuple[str, ...],
    module_additions: Mapping[str, str] | None = None,
) -> PythonBindingFacts:
    indices = {name: index for index, name in enumerate(slots)}
    scopes = []
    for scope in facts.scopes:
        bindings = {
            name: indices[facts.slot_names[slot]] for name, slot in scope.binding_slots
        }
        if scope.scope_id == 0 and module_additions is not None:
            bindings.update(
                (name, indices[slot]) for name, slot in module_additions.items()
            )
        scopes.append(replace(scope, binding_slots=tuple(sorted(bindings.items()))))
    return replace(facts, slot_names=slots, scopes=tuple(scopes))


@dataclass(frozen=True, slots=True)
class _BindingAnalysis:
    facts: PythonBindingFacts
    assignment_effects: Mapping[PythonSourceKey, EffectMask]
    module_import_flow_required: bool
    opaque_activation_reads: _OpaqueActivationReads | None
    foreign_namespace_reads: _ForeignNamespaceReads | None
    projections: _BindingCache[PythonBindingIndex] = field(
        default_factory=lambda: _BindingCache(max_entries=8), compare=False, repr=False
    )

    def with_added_foreign_slots(
        self, names: frozenset[str]
    ) -> _BindingAnalysis | None:
        """Project slots the transfer never looked up into their revised layout."""
        reads = self.foreign_namespace_reads
        if (
            reads is None
            or reads.has_empty_module_expiry
            or not reads.names.isdisjoint(names)
        ):
            return None
        slots = self.facts.slot_names
        additions = {name: f"0:{name}" for name in names}
        if not set(additions.values()).isdisjoint(slots):
            return None
        # Source declarations came first, followed by sorted callback slots.
        # Later allocations keep their relative order in the revised layout.
        callback_layout = tuple(
            sorted(
                (
                    *slots[reads.declaration_end : reads.callback_end],
                    *additions.values(),
                )
            )
        )
        new_slots = (
            *slots[: reads.declaration_end],
            *callback_layout,
            *slots[reads.callback_end :],
        )
        facts = _project_binding_slots(self.facts, new_slots, additions)
        # A callback may install a module export hook even when the incoming
        # value has no source origin.
        if reads.tainted_at_exit and "__getattr__" in names:
            facts = replace(facts, module_getattr_may_be_bound=True)
        return replace(
            self,
            facts=facts,
            foreign_namespace_reads=replace(
                reads, callback_end=reads.callback_end + len(names)
            ),
            projections=_BindingCache(max_entries=8),
        )


_BINDING_CHUNK_SHIFT: Final = 5
_BINDING_CHUNK_SIZE: Final = 1 << _BINDING_CHUNK_SHIFT
_BINDING_CHUNK_MASK: Final = _BINDING_CHUNK_SIZE - 1
_BINDING_CHUNK_BITS_MASK: Final = (1 << _BINDING_CHUNK_SIZE) - 1
_BINDING_TREE_SHIFT: Final = 2
_BINDING_TREE_SIZE: Final = 1 << _BINDING_TREE_SHIFT
_BINDING_TREE_MASK: Final = _BINDING_TREE_SIZE - 1


@dataclass(frozen=True, slots=True, eq=False)
class _BindingChunk:
    identities: tuple[IdentityMask, ...]
    static_values: tuple[PythonStaticValue, ...]
    results: tuple[StaticExpressionResult, ...]
    owner_tokens: tuple[int, ...]
    clean_epochs: tuple[int, ...]
    active_mask: int
    clean_mask: int
    mutation_mask: int = 0
    owner_mutation_mask: int = 0


_EMPTY_BINDING_CHUNK: Final = _BindingChunk(
    (UNBOUND_IDENTITY,) * _BINDING_CHUNK_SIZE,
    (None,) * _BINDING_CHUNK_SIZE,
    (UNKNOWN_EXPRESSION_RESULT,) * _BINDING_CHUNK_SIZE,
    (0,) * _BINDING_CHUNK_SIZE,
    (0,) * _BINDING_CHUNK_SIZE,
    0,
    0,
)


@dataclass(frozen=True, slots=True, eq=False)
class _BindingBranch:
    """Persistent radix children with raw-result mutation frontiers."""

    children: tuple[_BindingChunk | _BindingBranch, ...] = ()
    mutation_mask: int = 0
    owner_mutation_mask: int = 0
    chunk_count: int = 0


_EMPTY_BINDING_BRANCH: Final = _BindingBranch()


def _binding_branch(
    children: tuple[_BindingChunk | _BindingBranch, ...],
    level: int,
    previous: _BindingBranch = _EMPTY_BINDING_BRANCH,
) -> _BindingBranch:
    filler: _BindingChunk | _BindingBranch = (
        _EMPTY_BINDING_CHUNK if level == 0 else _EMPTY_BINDING_BRANCH
    )
    count = len(children)
    while count and children[count - 1] is filler:
        count -= 1
    children = children[:count]
    if not children:
        return _EMPTY_BINDING_BRANCH
    # Branches and chunks use object identity, so tuple equality tests sharing.
    if children == previous.children:
        return previous
    ordinary = preserved = 0
    for offset, child in enumerate(children):
        if child.mutation_mask:
            ordinary |= 1 << offset
        if child.owner_mutation_mask:
            preserved |= 1 << offset
    last = cast(_BindingBranch, children[-1]) if level else None
    extent = ((len(children) - 1) << (level * _BINDING_TREE_SHIFT)) + (
        last.chunk_count if last is not None else 1
    )
    return _BindingBranch(children, ordinary, preserved, extent)


@dataclass(frozen=True, slots=True)
class _BindingEnvironment:
    root: _BindingBranch = _EMPTY_BINDING_BRANCH
    depth: int = 1

    @property
    def chunk_count(self) -> int:
        return self.root.chunk_count


_EMPTY_BINDING_ENVIRONMENT: Final = _BindingEnvironment()


@dataclass(frozen=True, slots=True)
class _BindingResolution:
    """Raw storage payload, including custody; public projection is explicit."""

    identities: IdentityMask
    static_value: PythonStaticValue
    result: StaticExpressionResult
    clean: bool
    owner_token: int = 0
    namespace_clean: bool = False
    present: bool = False

    def public(self) -> _BindingResolution:
        if self.clean:
            return self
        return _BindingResolution(
            self.identities | OTHER_IDENTITY,
            None,
            expression_result_without_value_facts(self.result),
            False,
            present=self.present,
        )


_UNBOUND_BINDING_RESOLUTION: Final = _BindingResolution(
    UNBOUND_IDENTITY, None, UNKNOWN_EXPRESSION_RESULT, False
)


def _join_binding_payloads(
    alternatives: Sequence[
        tuple[IdentityMask, PythonStaticValue, StaticExpressionResult]
    ],
    *,
    telemetry: _StatePool | None = None,
) -> tuple[PythonStaticValue, StaticExpressionResult]:
    """Join normal payloads through one exact, ordered static/result authority."""
    if len(alternatives) == 2:
        left, right = alternatives
        left_normal = bool(left[0] & ~UNBOUND_IDENTITY)
        right_normal = bool(right[0] & ~UNBOUND_IDENTITY)
        if not left_normal or not right_normal:
            if telemetry is not None:
                telemetry.join_payload_absence_skips += 1
            normal = left if left_normal else right if right_normal else None
            return (None, UNKNOWN_EXPRESSION_RESULT) if normal is None else normal[1:]
        if left[1] is right[1] and left[2] is right[2]:
            if telemetry is not None:
                telemetry.join_payload_identity_skips += 1
            return left[1:]
        first_static, first_result = left[1:]
        static_value = join_python_static_values(first_static, right[1])
        results = (first_result, right[2])
    else:
        normal_payloads = tuple(
            (static_value, result)
            for identities, static_value, result in alternatives
            if identities & ~UNBOUND_IDENTITY
        )
        if not normal_payloads:
            return None, UNKNOWN_EXPRESSION_RESULT
        static_value = normal_payloads[0][0]
        for candidate, _result in normal_payloads[1:]:
            static_value = join_python_static_values(static_value, candidate)
            if static_value is None:
                break
        results = tuple(result for _static_value, result in normal_payloads)
    if telemetry is not None:
        telemetry.join_payload_algebra_calls += 1
    result = join_static_expression_results(results)
    if telemetry is not None and any(result is candidate for candidate in results):
        telemetry.join_payload_absorptions += 1
    return static_value, result


@dataclass(frozen=True, slots=True)
class _BindingState:
    """One interned persistent control-flow environment node."""

    parents: tuple[int, ...] = ()
    updated_slot: int = -1
    updated_value: IdentityMask = UNBOUND_IDENTITY
    updated_static_value: PythonStaticValue = field(default=None, compare=False)
    updated_result: StaticExpressionResult = UNKNOWN_EXPRESSION_RESULT
    updated_owner_token: int = 0
    updated_clean: bool | None = None
    updated_bindings: tuple[
        tuple[int, IdentityMask, PythonStaticValue, StaticExpressionResult, int, bool],
        ...,
    ] = field(default=(), compare=False)
    _static_identity: tuple[object, ...] = field(init=False, repr=False)
    taint_epoch: int = 0
    maybe_invalidated_members: MemberMask = 0
    definitely_invalidated_members: MemberMask = 0
    namespace_values_unknown: bool = False

    def __post_init__(self) -> None:
        object.__setattr__(
            self,
            "_static_identity",
            (
                python_static_value_key(self.updated_static_value),
                tuple(
                    (
                        slot,
                        identities,
                        python_static_value_key(static),
                        result,
                        owner,
                        clean,
                    )
                    for slot, identities, static, result, owner, clean in self.updated_bindings
                ),
            ),
        )


@dataclass(frozen=True, slots=True)
class _BindingEntryState:
    environment: _BindingEnvironment
    taint_epoch: int
    maybe_invalidated_members: MemberMask
    definitely_invalidated_members: MemberMask
    namespace_values_unknown: bool
    taint_domain: int
    callback_deletions: int

    def open(self) -> tuple[_StatePool, int]:
        pool = _StatePool()
        entry = _BindingState(
            parents=(0,),
            taint_epoch=self.taint_epoch,
            maybe_invalidated_members=self.maybe_invalidated_members,
            definitely_invalidated_members=self.definitely_invalidated_members,
            namespace_values_unknown=self.namespace_values_unknown,
        )
        pool._states.append(entry)
        pool._ids[entry] = 1
        pool._binding_environments.append(self.environment)
        pool._taint_domain_mask = self.taint_domain
        pool._callback_deletion_mask = self.callback_deletions
        return pool, 1


type _BindingUpdate = tuple[
    int, IdentityMask, PythonStaticValue, StaticExpressionResult, int
]


class _StatePool:
    def __init__(self) -> None:
        initial = _BindingState()
        self._states: list[_BindingState] = [initial]
        self._ids: dict[_BindingState, int] = {initial: 0}
        self._binding_environments: list[_BindingEnvironment] = [
            _EMPTY_BINDING_ENVIRONMENT
        ]
        self._taint_domain_mask = 0
        # Taint-domain slots a callback can also delete (`nonlocal` + `del`).
        self._callback_deletion_mask = 0
        self.taint_domain_generation = 0
        self.join_parent_inputs = 0
        self.join_max_parents = 0
        self.join_two_way_slots = 0
        self.join_wide_slots = 0
        self.join_wide_alternatives = 0
        self.join_payload_identity_skips = 0
        self.join_payload_absence_skips = 0
        self.join_payload_algebra_calls = 0
        self.join_payload_absorptions = 0
        self.join_custody_identity_skips = 0
        self.observation_fold_calls = 0
        self.observation_fold_new_parents = 0
        self.observation_fold_rebuilds = 0
        self.structural_diff_node_visits = 0
        self.structural_diff_shared_skips = 0
        self.binding_lookups = 0
        self.join_calls = 0
        self.join_node_visits = 0
        self.join_node_contributor_inputs = 0
        self.join_node_contributor_outputs = 0
        self.join_node_duplicate_contributors = 0
        self.join_shared_subtrees_skipped = 0
        self.join_chunk_merges = 0
        self.binding_chunk_copies = 0
        self.binding_branch_copies = 0
        self.binding_resolution_calls = 0
        self.mutation_frontier_node_visits = 0
        self.mutation_frontier_chunk_visits = 0
        self.mutation_frontier_slot_visits = 0

    def set_taint_domain(self, slots: int) -> None:
        if slots < 0:
            raise ValueError("binding taint domain cannot be negative")
        if slots & self._taint_domain_mask != self._taint_domain_mask:
            raise RuntimeError("binding taint domain can only grow")
        if slots != self._taint_domain_mask:
            self._taint_domain_mask = slots
            self.taint_domain_generation += 1

    def deferred_entry(self, state_id: int) -> _BindingEntryState:
        state = self._states[state_id]
        return _BindingEntryState(
            self._binding_environments[state_id],
            state.taint_epoch,
            state.maybe_invalidated_members,
            state.definitely_invalidated_members,
            state.namespace_values_unknown,
            self._taint_domain_mask,
            self._callback_deletion_mask,
        )

    def add_callback_deletions(self, slots: int) -> None:
        """Record lexical slots some reachable code deletes through `nonlocal`.

        The set of such writers is a closed-world fact of the source. A slot
        in it that a callback may have touched may also be empty, so its
        tainted resolution carries the unbound identity (HF-875).
        """
        if slots & ~self._taint_domain_mask:
            raise RuntimeError(
                "a callback deletion names a slot outside the taint domain"
            )
        if slots & ~self._callback_deletion_mask:
            self._callback_deletion_mask |= slots
            self.taint_domain_generation += 1

    @staticmethod
    def _chunk_at(environment: _BindingEnvironment, chunk_index: int) -> _BindingChunk:
        if chunk_index >= environment.chunk_count:
            return _EMPTY_BINDING_CHUNK
        node = environment.root
        for level in range(environment.depth - 1, -1, -1):
            offset = (chunk_index >> (level * _BINDING_TREE_SHIFT)) & _BINDING_TREE_MASK
            if offset >= len(node.children):
                return _EMPTY_BINDING_CHUNK
            child = node.children[offset]
            if level == 0:
                return cast(_BindingChunk, child)
            node = cast(_BindingBranch, child)
        return _EMPTY_BINDING_CHUNK

    def _binding_resolution(self, state_id: int, slot: int) -> _BindingResolution:
        environment = self._binding_environments[state_id]
        return self._resolve_chunk_binding(
            self._chunk_at(environment, slot >> _BINDING_CHUNK_SHIFT),
            slot,
            self._states[state_id].taint_epoch,
        )

    def _resolve_chunk_binding(
        self, chunk: _BindingChunk, slot: int, state_epoch: int
    ) -> _BindingResolution:
        # The taint domain can grow after an immutable state was interned.
        # Only storage is persistent: never memoize (state, slot) projections.
        self.binding_resolution_calls += 1
        offset = slot & _BINDING_CHUNK_MASK
        bit = 1 << offset
        present = bool((chunk.active_mask | chunk.clean_mask) & bit)
        namespace_clean = self._slot_is_clean(
            present=present,
            stored_clean=bool(chunk.clean_mask & bit),
            in_taint_domain=True,
            state_epoch=state_epoch,
            clean_epoch=chunk.clean_epochs[offset],
        )
        clean = (
            namespace_clean
            if self.slot_in_taint_domain(slot)
            else not present or bool(chunk.clean_mask & bit)
        )
        if not present:
            return _BindingResolution(
                UNBOUND_IDENTITY,
                None,
                UNKNOWN_EXPRESSION_RESULT,
                clean,
                namespace_clean=namespace_clean,
            )
        identities = chunk.identities[offset]
        if not namespace_clean and self._callback_deletion_mask >> slot & 1:
            # A callback that ran since the last store may have deleted it.
            identities |= UNBOUND_IDENTITY
        return _BindingResolution(
            identities,
            chunk.static_values[offset],
            chunk.results[offset],
            clean,
            chunk.owner_tokens[offset],
            namespace_clean,
            present=True,
        )

    @staticmethod
    def _slot_is_clean(
        *,
        present: bool,
        stored_clean: bool,
        in_taint_domain: bool,
        state_epoch: int,
        clean_epoch: int,
    ) -> bool:
        """Resolve one stored or absent slot against its namespace epoch."""

        if not present:
            return not in_taint_domain or state_epoch == 0
        if not stored_clean:
            return False
        return not in_taint_domain or clean_epoch == state_epoch

    def _make_chunk(
        self,
        identities: tuple[IdentityMask, ...],
        static_values: tuple[PythonStaticValue, ...],
        results: tuple[StaticExpressionResult, ...],
        owner_tokens: tuple[int, ...],
        clean_epochs: tuple[int, ...],
        active_mask: int,
        clean_mask: int,
        previous: _BindingChunk = _EMPTY_BINDING_CHUNK,
    ) -> _BindingChunk:
        """Construct storage and both frontiers from the canonical expiry."""
        if not (active_mask or clean_mask):
            return _EMPTY_BINDING_CHUNK
        # Reuse each exact field tuple, including custody-only publications.
        identities = (
            previous.identities if identities == previous.identities else identities
        )
        static_values = (
            previous.static_values
            if static_values is previous.static_values
            or all(
                same_python_static_value(new, old)
                for new, old in zip(static_values, previous.static_values, strict=True)
            )
            else static_values
        )
        results = previous.results if results == previous.results else results
        owner_tokens = (
            previous.owner_tokens
            if owner_tokens == previous.owner_tokens
            else owner_tokens
        )
        clean_epochs = (
            previous.clean_epochs
            if clean_epochs == previous.clean_epochs
            else clean_epochs
        )
        if (
            identities is previous.identities
            and static_values is previous.static_values
            and results is previous.results
            and owner_tokens is previous.owner_tokens
            and clean_epochs is previous.clean_epochs
            and active_mask == previous.active_mask
            and clean_mask == previous.clean_mask
        ):
            return previous
        payload = (
            identities,
            static_values,
            results,
            owner_tokens,
            clean_epochs,
            active_mask,
            clean_mask,
        )
        candidates = active_mask & clean_mask
        ordinary = previous.mutation_mask & candidates
        preserved = previous.owner_mutation_mask & candidates
        remaining = candidates
        while remaining:
            bit = remaining & -remaining
            remaining ^= bit
            offset = bit.bit_length() - 1
            result = results[offset]
            if (
                previous.active_mask & previous.clean_mask & bit
                and result is previous.results[offset]
            ):
                continue
            ordinary &= ~bit
            preserved &= ~bit
            if expression_result_without_mutable_contents(result) != result:
                ordinary |= bit
            if (
                expression_result_without_mutable_contents(result, preserve_owner=True)
                != result
            ):
                preserved |= bit
        self.binding_chunk_copies += 1
        return _BindingChunk(*payload, ordinary, preserved)

    @staticmethod
    def _root_at_depth(environment: _BindingEnvironment, depth: int) -> _BindingBranch:
        root = environment.root
        for level in range(environment.depth, depth):
            root = (
                _binding_branch((root,), level)
                if root.children
                else _EMPTY_BINDING_BRANCH
            )
        return root

    def _publish_environment(
        self,
        environment: _BindingEnvironment,
        updates: Sequence[
            tuple[
                int, IdentityMask, PythonStaticValue, StaticExpressionResult, int, bool
            ]
        ],
        clean_epoch: int,
    ) -> _BindingEnvironment:
        """Publish one batch: each touched chunk and old ancestor copied once.

        History keeps ordered effective updates, including all recorded writes.
        Storage groups by chunk with final-write-wins within the supplied batch.
        """
        grouped: dict[
            int,
            dict[
                int,
                tuple[
                    IdentityMask, PythonStaticValue, StaticExpressionResult, int, bool
                ],
            ],
        ] = {}
        for slot, identity, static_value, result, owner_token, clean in updates:
            if slot < 0:
                raise ValueError("binding slots must be nonnegative")
            grouped.setdefault(slot >> _BINDING_CHUNK_SHIFT, {})[
                slot & _BINDING_CHUNK_MASK
            ] = (identity, static_value, result, owner_token, clean)
        if not grouped:
            return environment
        chunks: dict[int, _BindingChunk] = {}
        for chunk_index in sorted(grouped):
            previous = self._chunk_at(environment, chunk_index)
            identities = list(previous.identities)
            static_values = list(previous.static_values)
            results = list(previous.results)
            owners = list(previous.owner_tokens)
            epochs = list(previous.clean_epochs)
            active_mask, clean_mask = previous.active_mask, previous.clean_mask
            for offset, (identity, static_value, result, owner, clean) in sorted(
                grouped[chunk_index].items()
            ):
                identities[offset] = identity
                static_values[offset] = static_value
                results[offset] = result
                owners[offset] = owner
                epochs[offset] = clean_epoch
                bit = 1 << offset
                if (
                    identity == UNBOUND_IDENTITY
                    and static_value is None
                    and result == UNKNOWN_EXPRESSION_RESULT
                    and clean
                ):
                    active_mask &= ~bit
                else:
                    active_mask |= bit
                clean_mask = clean_mask | bit if clean else clean_mask & ~bit
            updated = self._make_chunk(
                tuple(identities),
                tuple(static_values),
                tuple(results),
                tuple(owners),
                tuple(epochs),
                active_mask,
                clean_mask,
                previous,
            )
            if updated is not previous:
                chunks[chunk_index] = updated
        if not chunks:
            return environment
        depth = environment.depth
        while max(chunks) >= _BINDING_TREE_SIZE**depth:
            depth += 1
        root = self._root_at_depth(environment, depth)

        def rebuild(
            node: _BindingBranch, level: int, indices: list[int]
        ) -> _BindingBranch:
            by_offset: dict[int, list[int]] = {}
            for index in indices:
                offset = (index >> (level * _BINDING_TREE_SHIFT)) & _BINDING_TREE_MASK
                by_offset.setdefault(offset, []).append(index)
            children = list(node.children)
            filler: _BindingChunk | _BindingBranch = (
                _EMPTY_BINDING_CHUNK if level == 0 else _EMPTY_BINDING_BRANCH
            )
            required = max(by_offset) + 1
            if required > len(children):
                children.extend((filler,) * (required - len(children)))
            for offset, branch_indices in sorted(by_offset.items()):
                children[offset] = (
                    chunks[branch_indices[0]]
                    if level == 0
                    else rebuild(
                        cast(_BindingBranch, children[offset]),
                        level - 1,
                        branch_indices,
                    )
                )
            updated = _binding_branch(tuple(children), level, node)
            if updated is not node:
                self.binding_branch_copies += 1
            return updated

        return _BindingEnvironment(rebuild(root, depth - 1, sorted(chunks)), depth)

    def _joined_environment(
        self, parents: tuple[int, ...], taint_epoch: int
    ) -> _BindingEnvironment:
        environments = tuple(self._binding_environments[parent] for parent in parents)
        first_environment = environments[0]
        if all(environment is first_environment for environment in environments[1:]):
            # Only immutable raw storage is shared. The new state still owns all
            # parent writes, namespace epochs and member invalidations; projecting
            # that storage later applies its own current domain and deletion mask.
            self.join_shared_subtrees_skipped += 1
            return first_environment
        depth = max((environment.depth for environment in environments), default=1)
        roots = tuple(
            self._root_at_depth(environment, depth) for environment in environments
        )
        parent_epochs = tuple(self._states[parent].taint_epoch for parent in parents)
        joined_epoch = max(parent_epochs)
        # Raw custody is independent of the live domain. clean_mask joins
        # outside-domain cleanliness; clean_epochs records the joined epoch only
        # when every parent is hypothetically clean inside the domain, else -1.
        # Monotone state epochs make shared-subtree reuse equivalent to this join.

        def distinct_contributors[T: _BindingChunk | _BindingBranch](
            nodes: tuple[T, ...], epochs: tuple[int, ...]
        ) -> tuple[tuple[T, ...], tuple[int, ...]]:
            """Prune identical raw storage/epoch pairs in first-parent order.

            This is storage work only: the interned state retains every parent
            and its writes, invalidations, and namespace epoch. Equal payloads
            or shared nodes at different epochs are not interchangeable here.
            """
            unique_nodes: list[T] = []
            unique_epochs: list[int] = []
            seen: set[tuple[int, int]] = set()
            for node, epoch in zip(nodes, epochs, strict=True):
                key = (id(node), epoch)
                if key in seen:
                    continue
                seen.add(key)
                unique_nodes.append(node)
                unique_epochs.append(epoch)
            if len(unique_nodes) == len(nodes):
                return nodes, epochs
            return tuple(unique_nodes), tuple(unique_epochs)

        def merge_chunks(
            chunks: tuple[_BindingChunk, ...],
            chunk_epochs: tuple[int, ...],
        ) -> _BindingChunk:
            self.join_chunk_merges += 1
            first = chunks[0]
            if chunks.count(first) == len(chunks):
                self.join_shared_subtrees_skipped += 1
                return first
            if len(chunks) > 3:
                chunks, chunk_epochs = distinct_contributors(chunks, chunk_epochs)
            if len(chunks) == 1 and chunk_epochs[0] == taint_epoch:
                return chunks[0]
            candidate_mask = 0
            dirty_mask = 0
            for chunk in chunks:
                candidate_mask |= chunk.active_mask | chunk.clean_mask
                dirty_mask |= chunk.active_mask & ~chunk.clean_mask
            if not candidate_mask:
                return _EMPTY_BINDING_CHUNK
            # Copy a field only when its joined value differs from the first
            # parent. Every published slot has an active or clean bit, so fields
            # outside candidate_mask retain their canonical empty values.
            identities: list[IdentityMask] | None = None
            static_values: list[PythonStaticValue] | None = None
            results: list[StaticExpressionResult] | None = None
            owner_tokens: list[int] | None = None
            clean_epochs: list[int] | None = None
            active_mask = 0
            # Outside the taint domain, absent slots are clean. A stored
            # dirty binding in any predecessor is the only way to lose it.
            clean_mask = candidate_mask & ~dirty_mask
            if len(chunks) == 2:
                left, right = chunks
                left_epoch, right_epoch = chunk_epochs
                left_present = left.active_mask | left.clean_mask
                right_present = right.active_mask | right.clean_mask
                different_custody = (left_present ^ right_present) | (
                    left.clean_mask ^ right.clean_mask
                )
                matching_epoch = left_epoch == right_epoch == taint_epoch
                remaining = candidate_mask
                while remaining:
                    slot_bit = remaining & -remaining
                    chunk_offset = slot_bit.bit_length() - 1
                    remaining ^= slot_bit
                    self.join_two_way_slots += 1
                    if (
                        not (different_custody & slot_bit)
                        and (
                            matching_epoch
                            and left.clean_epochs[chunk_offset]
                            == right.clean_epochs[chunk_offset]
                            == taint_epoch
                            or left.clean_epochs[chunk_offset]
                            == right.clean_epochs[chunk_offset]
                            == -1
                            and (
                                bool(clean_mask & slot_bit)
                                or left.owner_tokens[chunk_offset] == 0
                            )
                        )
                        and left.identities[chunk_offset]
                        == right.identities[chunk_offset]
                        and left.static_values[chunk_offset]
                        is right.static_values[chunk_offset]
                        and left.results[chunk_offset] is right.results[chunk_offset]
                        and left.owner_tokens[chunk_offset]
                        == right.owner_tokens[chunk_offset]
                    ):
                        # Keep the left raw payload/custody. The state join itself
                        # is still interned: equal storage never erases path writes.
                        # Epoch -1 is already dirty in every namespace epoch;
                        # joining it cannot recover in-domain cleanliness.
                        self.join_custody_identity_skips += 1
                        active_mask |= left.active_mask & slot_bit
                        continue
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
                    left_result = (
                        left.results[chunk_offset]
                        if left_present & slot_bit
                        else UNKNOWN_EXPRESSION_RESULT
                    )
                    right_result = (
                        right.results[chunk_offset]
                        if right_present & slot_bit
                        else UNKNOWN_EXPRESSION_RESULT
                    )
                    static_value, result = _join_binding_payloads(
                        (
                            (left_identity, left_static, left_result),
                            (right_identity, right_static, right_result),
                        ),
                        telemetry=self,
                    )
                    clean = bool(clean_mask & slot_bit)
                    inside_clean = (
                        clean
                        and self._slot_is_clean(
                            present=bool(left_present & slot_bit),
                            stored_clean=bool(left.clean_mask & slot_bit),
                            in_taint_domain=True,
                            state_epoch=left_epoch,
                            clean_epoch=left.clean_epochs[chunk_offset],
                        )
                        and self._slot_is_clean(
                            present=bool(right_present & slot_bit),
                            stored_clean=bool(right.clean_mask & slot_bit),
                            in_taint_domain=True,
                            state_epoch=right_epoch,
                            clean_epoch=right.clean_epochs[chunk_offset],
                        )
                    )
                    left_owner = left.owner_tokens[chunk_offset]
                    right_owner = right.owner_tokens[chunk_offset]
                    owner_token = (
                        left_owner
                        if clean and left_owner != 0 and left_owner == right_owner
                        else 0
                    )
                    clean_epoch = joined_epoch if inside_clean else -1
                    if identity != left_identity:
                        if identities is None:
                            identities = list(first.identities)
                        identities[chunk_offset] = identity
                    if static_value is not left_static:
                        if static_values is None:
                            static_values = list(first.static_values)
                        static_values[chunk_offset] = static_value
                    if result is not left_result:
                        if results is None:
                            results = list(first.results)
                        results[chunk_offset] = result
                    if owner_token != left_owner:
                        if owner_tokens is None:
                            owner_tokens = list(first.owner_tokens)
                        owner_tokens[chunk_offset] = owner_token
                    if clean_epoch != first.clean_epochs[chunk_offset]:
                        if clean_epochs is None:
                            clean_epochs = list(first.clean_epochs)
                        clean_epochs[chunk_offset] = clean_epoch
                    if (
                        identity != UNBOUND_IDENTITY
                        or static_value is not None
                        or result != UNKNOWN_EXPRESSION_RESULT
                        or not clean
                    ):
                        active_mask |= slot_bit
                if not (active_mask or clean_mask):
                    return _EMPTY_BINDING_CHUNK
                return self._make_chunk(
                    first.identities if identities is None else tuple(identities),
                    first.static_values
                    if static_values is None
                    else tuple(static_values),
                    first.results if results is None else tuple(results),
                    first.owner_tokens if owner_tokens is None else tuple(owner_tokens),
                    first.clean_epochs if clean_epochs is None else tuple(clean_epochs),
                    active_mask,
                    clean_mask,
                    first,
                )
            remaining = candidate_mask
            while remaining:
                slot_bit = remaining & -remaining
                chunk_offset = slot_bit.bit_length() - 1
                remaining ^= slot_bit
                self.join_wide_slots += 1
                self.join_wide_alternatives += len(chunks)
                identity = NO_IDENTITIES
                alternatives: list[
                    tuple[IdentityMask, PythonStaticValue, StaticExpressionResult]
                ] = []
                clean = bool(clean_mask & slot_bit)
                inside_clean = clean
                owner_token: int | None = None
                for chunk, parent_epoch in zip(chunks, chunk_epochs, strict=True):
                    present = bool((chunk.active_mask | chunk.clean_mask) & slot_bit)
                    candidate_identity = (
                        chunk.identities[chunk_offset] if present else UNBOUND_IDENTITY
                    )
                    identity |= candidate_identity
                    candidate_static = (
                        chunk.static_values[chunk_offset] if present else None
                    )
                    candidate_result = (
                        chunk.results[chunk_offset]
                        if present
                        else UNKNOWN_EXPRESSION_RESULT
                    )
                    alternatives.append(
                        (candidate_identity, candidate_static, candidate_result)
                    )
                    inside_clean = inside_clean and self._slot_is_clean(
                        present=present,
                        stored_clean=bool(chunk.clean_mask & slot_bit),
                        in_taint_domain=True,
                        state_epoch=parent_epoch,
                        clean_epoch=chunk.clean_epochs[chunk_offset],
                    )
                    candidate_owner = chunk.owner_tokens[chunk_offset]
                    owner_token = (
                        candidate_owner
                        if owner_token is None
                        else owner_token
                        if owner_token == candidate_owner
                        else 0
                    )
                static_value, result = _join_binding_payloads(
                    alternatives, telemetry=self
                )
                owner_token = owner_token if clean and owner_token is not None else 0
                clean_epoch = joined_epoch if inside_clean else -1
                if identity != first.identities[chunk_offset]:
                    if identities is None:
                        identities = list(first.identities)
                    identities[chunk_offset] = identity
                if static_value is not first.static_values[chunk_offset]:
                    if static_values is None:
                        static_values = list(first.static_values)
                    static_values[chunk_offset] = static_value
                if result is not first.results[chunk_offset]:
                    if results is None:
                        results = list(first.results)
                    results[chunk_offset] = result
                if owner_token != first.owner_tokens[chunk_offset]:
                    if owner_tokens is None:
                        owner_tokens = list(first.owner_tokens)
                    owner_tokens[chunk_offset] = owner_token
                if clean_epoch != first.clean_epochs[chunk_offset]:
                    if clean_epochs is None:
                        clean_epochs = list(first.clean_epochs)
                    clean_epochs[chunk_offset] = clean_epoch
                if (
                    identity != UNBOUND_IDENTITY
                    or static_value is not None
                    or result != UNKNOWN_EXPRESSION_RESULT
                    or not clean
                ):
                    active_mask |= slot_bit
            if not (active_mask or clean_mask):
                return _EMPTY_BINDING_CHUNK
            return self._make_chunk(
                first.identities if identities is None else tuple(identities),
                first.static_values if static_values is None else tuple(static_values),
                first.results if results is None else tuple(results),
                first.owner_tokens if owner_tokens is None else tuple(owner_tokens),
                first.clean_epochs if clean_epochs is None else tuple(clean_epochs),
                active_mask,
                clean_mask,
                first,
            )

        def merge_nodes(
            nodes: tuple[_BindingBranch, ...],
            epochs: tuple[int, ...],
            level: int,
        ) -> _BindingBranch:
            self.join_node_visits += 1
            submitted = len(nodes)
            self.join_node_contributor_inputs += submitted
            first = nodes[0]
            if nodes.count(first) == submitted:
                self.join_node_contributor_outputs += submitted
                self.join_shared_subtrees_skipped += 1
                return first
            if len(nodes) > 3:
                nodes, epochs = distinct_contributors(nodes, epochs)
            self.join_node_contributor_outputs += len(nodes)
            self.join_node_duplicate_contributors += submitted - len(nodes)
            children: list[_BindingChunk | _BindingBranch] = []
            filler: _BindingChunk | _BindingBranch = (
                _EMPTY_BINDING_CHUNK if level == 0 else _EMPTY_BINDING_BRANCH
            )
            # Zip aligned child columns, padding the shorter persistent spines
            # with canonical empty storage.
            for contributors in zip_longest(
                *(node.children for node in nodes), fillvalue=filler
            ):
                if level == 0:
                    child = merge_chunks(
                        cast(tuple[_BindingChunk, ...], contributors), epochs
                    )
                else:
                    child = merge_nodes(
                        cast(tuple[_BindingBranch, ...], contributors),
                        epochs,
                        level - 1,
                    )
                children.append(child)
            return _binding_branch(tuple(children), level, first)

        root = merge_nodes(tuple(roots), parent_epochs, depth - 1)
        return _BindingEnvironment(root, depth)

    def intern(self, state: _BindingState) -> int:
        # Epochs are monotone along every state edge. Shared raw chunks can
        # therefore project at the max parent epoch without re-materialization.
        if state.taint_epoch < 0 or any(
            self._states[parent].taint_epoch > state.taint_epoch
            for parent in state.parents
        ):
            raise ValueError("binding namespace epochs cannot regress")
        known = self._ids.get(state)
        if known is not None:
            return known
        if not state.parents:
            environment = _EMPTY_BINDING_ENVIRONMENT
        elif len(state.parents) == 1:
            environment = self._binding_environments[state.parents[0]]
        else:
            environment = self._joined_environment(state.parents, state.taint_epoch)
        updates: list[
            tuple[
                int, IdentityMask, PythonStaticValue, StaticExpressionResult, int, bool
            ]
        ] = []
        if state.updated_slot >= 0:
            assert state.updated_clean is not None
            updates.append(
                (
                    state.updated_slot,
                    state.updated_value,
                    state.updated_static_value,
                    state.updated_result,
                    state.updated_owner_token,
                    state.updated_clean,
                )
            )
        updates.extend(state.updated_bindings)
        environment = self._publish_environment(environment, updates, state.taint_epoch)
        index = len(self._states)
        self._states.append(state)
        self._binding_environments.append(environment)
        self._ids[state] = index
        return index

    def get(self, state_id: int) -> _BindingState:
        return self._states[state_id]

    def _binding_details(
        self, state_id: int, slot: int
    ) -> tuple[IdentityMask, PythonStaticValue, StaticExpressionResult, bool]:
        resolution = self._binding_resolution(state_id, slot).public()
        return (
            resolution.identities,
            resolution.static_value,
            resolution.result,
            resolution.clean,
        )

    def binding(self, state_id: int, slot: int) -> IdentityMask:
        self.binding_lookups += 1
        return self._binding_details(state_id, slot)[0]

    def static_value(self, state_id: int, slot: int) -> PythonStaticValue:
        return self._binding_details(state_id, slot)[1]

    def result(self, state_id: int, slot: int) -> StaticExpressionResult:
        return self._binding_details(state_id, slot)[2]

    def owner_token(self, state_id: int, slot: int) -> int:
        resolution = self._binding_resolution(state_id, slot)
        return resolution.owner_token if resolution.clean else 0

    def set_binding(
        self,
        state_id: int,
        slot: int,
        value: IdentityMask,
        static_value: PythonStaticValue = None,
        result: StaticExpressionResult = UNKNOWN_EXPRESSION_RESULT,
        owner_token: int = 0,
    ) -> int:
        return self.set_bindings(
            state_id, ((slot, value, static_value, result, owner_token),)
        )

    def set_bindings(
        self,
        state_id: int,
        bindings: Sequence[_BindingUpdate],
        *,
        record_writes: bool = False,
    ) -> int:
        updates: list[
            tuple[
                int, IdentityMask, PythonStaticValue, StaticExpressionResult, int, bool
            ]
        ] = []
        # This map exists only during this call. It transports preceding writes,
        # never cached projections of an immutable state across domain growth.
        # A default write also refreshes hypothetical in-domain custody, even
        # when its public payload outside the current domain is unchanged.
        staged: dict[int, _BindingResolution] = {}
        for slot, value, static_value, result, owner_token in bindings:
            if slot < 0:
                raise ValueError("binding slots must be nonnegative")
            if not record_writes:
                current = staged.get(slot)
                if current is None:
                    current = self._binding_resolution(state_id, slot).public()
                    staged[slot] = current
                if (
                    current.present
                    and current.identities == value
                    and same_python_static_value(current.static_value, static_value)
                    and current.result == result
                    and current.owner_token == owner_token
                    and current.clean
                    and current.namespace_clean
                ):
                    continue
                staged[slot] = _BindingResolution(
                    value, static_value, result, True, owner_token, True, True
                )
            updates.append((slot, value, static_value, result, owner_token, True))
        return self._publish_updates(state_id, updates)

    def _publish_updates(
        self,
        state_id: int,
        updates: Sequence[
            tuple[
                int, IdentityMask, PythonStaticValue, StaticExpressionResult, int, bool
            ]
        ],
    ) -> int:
        """Preserve ordered history independently of storage path sharing."""
        # -1 encodes "no point write" in _BindingState, so reject it before
        # selecting the single-write history representation.
        if any(slot < 0 for slot, *_payload in updates):
            raise ValueError("binding slots must be nonnegative")
        if not updates:
            return state_id
        state = self._states[state_id]
        if len(updates) == 1:
            slot, value, static_value, result, owner_token, clean = updates[0]
            return self.intern(
                _BindingState(
                    parents=(state_id,),
                    updated_slot=slot,
                    updated_value=value,
                    updated_static_value=static_value,
                    updated_result=result,
                    updated_owner_token=owner_token,
                    updated_clean=clean,
                    taint_epoch=state.taint_epoch,
                    maybe_invalidated_members=state.maybe_invalidated_members,
                    definitely_invalidated_members=state.definitely_invalidated_members,
                    namespace_values_unknown=state.namespace_values_unknown,
                )
            )
        return self.intern(
            _BindingState(
                parents=(state_id,),
                updated_bindings=tuple(updates),
                taint_epoch=state.taint_epoch,
                maybe_invalidated_members=state.maybe_invalidated_members,
                definitely_invalidated_members=state.definitely_invalidated_members,
                namespace_values_unknown=state.namespace_values_unknown,
            )
        )

    def _mutation_chunks(
        self, environment: _BindingEnvironment
    ) -> Iterator[tuple[int, _BindingChunk]]:
        pending = [(environment.root, environment.depth - 1, 0)]
        while pending:
            node, level, prefix = pending.pop()
            self.mutation_frontier_node_visits += 1
            frontier = node.mutation_mask | node.owner_mutation_mask
            if not frontier:
                continue
            if level == 0:
                while frontier:
                    bit = frontier & -frontier
                    frontier ^= bit
                    offset = bit.bit_length() - 1
                    self.mutation_frontier_chunk_visits += 1
                    yield prefix | offset, cast(_BindingChunk, node.children[offset])
            else:
                # Stack traversal is ascending by chunk, independent of batch order.
                for offset in range(len(node.children) - 1, -1, -1):
                    if frontier & (1 << offset):
                        pending.append(
                            (
                                cast(_BindingBranch, node.children[offset]),
                                level - 1,
                                prefix | (offset << (level * _BINDING_TREE_SHIFT)),
                            )
                        )

    def invalidate_mutable_contents(
        self, state_id: int, *, except_slots: frozenset[int] = frozenset()
    ) -> int:
        """Expire only susceptible storage, resolving live cleanliness once."""
        environment = self._binding_environments[state_id]
        state_epoch = self._states[state_id].taint_epoch
        exemptions: dict[int, int] = {}
        for slot in except_slots:
            if slot >= 0:
                chunk_index = slot >> _BINDING_CHUNK_SHIFT
                exemptions[chunk_index] = exemptions.get(chunk_index, 0) | (
                    1 << (slot & _BINDING_CHUNK_MASK)
                )
        updates: list[
            tuple[
                int, IdentityMask, PythonStaticValue, StaticExpressionResult, int, bool
            ]
        ] = []
        for chunk_index, chunk in self._mutation_chunks(environment):
            exempt = exemptions.get(chunk_index, 0)
            remaining = (chunk.mutation_mask & ~exempt) | (
                chunk.owner_mutation_mask & exempt
            )
            while remaining:
                bit = remaining & -remaining
                remaining ^= bit
                slot = (chunk_index << _BINDING_CHUNK_SHIFT) | (bit.bit_length() - 1)
                self.mutation_frontier_slot_visits += 1
                resolution = self._resolve_chunk_binding(chunk, slot, state_epoch)
                if not resolution.clean:
                    continue
                widened = expression_result_without_mutable_contents(
                    resolution.result, preserve_owner=bool(exempt & bit)
                )
                if widened != resolution.result:
                    updates.append(
                        (
                            slot,
                            resolution.identities,
                            resolution.static_value,
                            widened,
                            resolution.owner_token,
                            True,
                        )
                    )
        return self._publish_updates(state_id, updates)

    @staticmethod
    def _slot_mask_reaches_subtree(slots: int, chunk_prefix: int, level: int) -> bool:
        """Intersect a sparse slot frontier without materializing a huge range mask.

        Level zero is one chunk; each higher level spans one radix branch.
        A domain may extend far beyond either stored environment.
        """
        suffix = slots >> (chunk_prefix << _BINDING_CHUNK_SHIFT)
        width = 1 << (level * _BINDING_TREE_SHIFT + _BINDING_CHUNK_SHIFT)
        return bool(suffix) and (suffix & -suffix).bit_length() <= width

    def _changed_chunks(
        self, previous: int, current: int, *, shared_slots: int = 0
    ) -> Iterator[tuple[int, _BindingChunk, _BindingChunk, int]]:
        """Yield raw-storage changes plus the explicit shared projection frontier.

        Equal raw storage only changes meaning for namespace-domain slots when
        epochs differ. Both semantic facts and ownership consume this frontier;
        the structural-only diff supplies no shared slots.
        """
        before = self._binding_environments[previous]
        after = self._binding_environments[current]
        depth = max(before.depth, after.depth)
        pending = [
            (
                self._root_at_depth(before, depth),
                self._root_at_depth(after, depth),
                depth - 1,
                0,
            )
        ]
        while pending:
            left, right, level, prefix = pending.pop()
            self.structural_diff_node_visits += 1
            if left is right and (
                not left.children
                or not self._slot_mask_reaches_subtree(shared_slots, prefix, level + 1)
            ):
                self.structural_diff_shared_skips += 1
                continue
            count = max(len(left.children), len(right.children))
            filler: _BindingChunk | _BindingBranch = (
                _EMPTY_BINDING_CHUNK if level == 0 else _EMPTY_BINDING_BRANCH
            )
            offsets = range(count) if level == 0 else range(count - 1, -1, -1)
            for offset in offsets:
                left_child = (
                    left.children[offset] if offset < len(left.children) else filler
                )
                right_child = (
                    right.children[offset] if offset < len(right.children) else filler
                )
                child_prefix = prefix | (offset << (level * _BINDING_TREE_SHIFT))
                if left_child is right_child and (
                    left_child is filler
                    or not self._slot_mask_reaches_subtree(
                        shared_slots, child_prefix, level
                    )
                ):
                    self.structural_diff_shared_skips += 1
                    continue
                if level:
                    pending.append(
                        (
                            cast(_BindingBranch, left_child),
                            cast(_BindingBranch, right_child),
                            level - 1,
                            child_prefix,
                        )
                    )
                else:
                    left_chunk = cast(_BindingChunk, left_child)
                    right_chunk = cast(_BindingChunk, right_child)
                    candidates = (
                        left_chunk.active_mask
                        | left_chunk.clean_mask
                        | right_chunk.active_mask
                        | right_chunk.clean_mask
                    )
                    if left_chunk is right_chunk:
                        candidates &= shared_slots >> (
                            child_prefix << _BINDING_CHUNK_SHIFT
                        )
                    if candidates:
                        yield child_prefix, left_chunk, right_chunk, candidates

    def _binding_differences(
        self, previous: int, current: int, *, semantic: bool
    ) -> Iterator[int]:
        """Yield slots whose projection differs, including shared state if semantic."""
        previous_epoch = self._states[previous].taint_epoch
        current_epoch = self._states[current].taint_epoch
        epochs_differ = previous_epoch != current_epoch
        stored_domain = 0
        for chunk_index, left, right, remaining in self._changed_chunks(
            previous,
            current,
            shared_slots=self._taint_domain_mask if semantic and epochs_differ else 0,
        ):
            if semantic and epochs_differ:
                stored_domain |= remaining << (chunk_index << _BINDING_CHUNK_SHIFT)
            while remaining:
                bit = remaining & -remaining
                remaining ^= bit
                slot = (chunk_index << _BINDING_CHUNK_SHIFT) | (bit.bit_length() - 1)
                old = self._resolve_chunk_binding(left, slot, previous_epoch).public()
                new = self._resolve_chunk_binding(right, slot, current_epoch).public()
                if (
                    old.present != new.present
                    or old.identities != new.identities
                    or not same_python_static_value(old.static_value, new.static_value)
                    or old.result != new.result
                ):
                    yield slot
        if semantic and epochs_differ:
            remaining = self._taint_domain_mask & ~stored_domain
            if remaining:
                # All absent live-domain slots share the same public projection.
                # Resolve once through the canonical rule, not once per slot.
                slot = (remaining & -remaining).bit_length() - 1
                old = self._resolve_chunk_binding(
                    _EMPTY_BINDING_CHUNK, slot, previous_epoch
                ).public()
                new = self._resolve_chunk_binding(
                    _EMPTY_BINDING_CHUNK, slot, current_epoch
                ).public()
                if old.identities != new.identities:
                    while remaining:
                        bit = remaining & -remaining
                        remaining ^= bit
                        yield bit.bit_length() - 1

    def changed_slots_between(self, previous: int, current: int) -> tuple[int, ...]:
        """Project exact differences on structurally changed storage only.

        Shared storage with a different namespace epoch is intentionally skipped.
        Semantic equality and history fallback use _binding_differences with
        semantic=True, including shared storage and absent live-domain slots.
        """
        return tuple(self._binding_differences(previous, current, semantic=False))

    def transition_binding_slots(self, previous: int, current: int) -> tuple[int, ...]:
        """Index explicit writes and nonlocal changes at observation boundaries."""
        direct: set[int] = set()
        cursor = current
        while cursor != previous:
            state = self._states[cursor]
            if state.updated_slot >= 0:
                direct.add(state.updated_slot)
            direct.update(row[0] for row in state.updated_bindings)
            if len(state.parents) != 1:
                break
            cursor = state.parents[0]
        if cursor == previous:
            return tuple(sorted(direct))
        return tuple(
            sorted(self._binding_differences(previous, current, semantic=True))
        )

    def slot_updated_between(self, previous: int, current: int, slot: int) -> bool:
        """Report a write on any state path, including owner-only branch joins."""

        pending = [current]
        visited: set[int] = set()
        reached_previous = False
        while pending:
            cursor = pending.pop()
            if cursor == previous:
                reached_previous = True
                continue
            if cursor in visited:
                continue
            visited.add(cursor)
            state = self._states[cursor]
            if state.updated_slot == slot or any(
                updated_slot == slot
                for updated_slot, *_payload in state.updated_bindings
            ):
                return True
            if not state.parents or any(parent < previous for parent in state.parents):
                return True
            pending.extend(state.parents)
        return not reached_previous

    def taint_exposed_bindings(self, state_id: int) -> int:
        """Expire the shared domain of callback-visible storage bindings."""
        # The namespace can acquire previously undeclared names, even when the
        # module has no statically allocated binding slots (e.g. import-star).
        state = self._states[state_id]
        return self.intern(
            _BindingState(
                parents=(state_id,),
                taint_epoch=state.taint_epoch + 1,
                maybe_invalidated_members=state.maybe_invalidated_members,
                definitely_invalidated_members=state.definitely_invalidated_members,
                namespace_values_unknown=state.namespace_values_unknown,
            )
        )

    def taint_slots(self, state_id: int, slots: int) -> int:
        if slots & self._taint_domain_mask:
            state_id = self.taint_exposed_bindings(state_id)
            slots &= ~self._taint_domain_mask
        updates: list[
            tuple[
                int, IdentityMask, PythonStaticValue, StaticExpressionResult, int, bool
            ]
        ] = []
        remaining = slots
        while remaining:
            bit = remaining & -remaining
            remaining ^= bit
            slot = bit.bit_length() - 1
            resolution = self._binding_resolution(state_id, slot)
            if resolution.clean:
                updates.append(
                    (
                        slot,
                        resolution.identities,
                        resolution.static_value,
                        resolution.result,
                        0,
                        False,
                    )
                )
        return self._publish_updates(state_id, updates)

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
                namespace_values_unknown=state.namespace_values_unknown,
            )
        )

    def invalidate_namespace_values(self, state_id: int) -> int:
        state = self._states[state_id]
        if state.namespace_values_unknown:
            return state_id
        return self.intern(
            _BindingState(
                parents=(state_id,),
                taint_epoch=state.taint_epoch,
                maybe_invalidated_members=state.maybe_invalidated_members,
                definitely_invalidated_members=state.definitely_invalidated_members,
                namespace_values_unknown=True,
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
                namespace_values_unknown=state.namespace_values_unknown,
            )
        )

    def overlay_summary_properties(
        self,
        state_id: int,
        *,
        maybe_invalidated_members: MemberMask,
        definitely_invalidated_members: MemberMask,
        taint_epoch: int,
        namespace_values_unknown: bool,
    ) -> int:
        state = self._states[state_id]
        maybe = state.maybe_invalidated_members | maybe_invalidated_members
        definitely = (
            state.definitely_invalidated_members & definitely_invalidated_members
        )
        epoch = max(state.taint_epoch, taint_epoch)
        unknown = state.namespace_values_unknown or namespace_values_unknown
        if (
            maybe == state.maybe_invalidated_members
            and definitely == state.definitely_invalidated_members
            and epoch == state.taint_epoch
            and unknown == state.namespace_values_unknown
        ):
            return state_id
        return self.intern(
            _BindingState(
                parents=(state_id,),
                taint_epoch=epoch,
                maybe_invalidated_members=maybe,
                definitely_invalidated_members=definitely,
                namespace_values_unknown=unknown,
            )
        )

    def slot_in_taint_domain(self, slot: int) -> bool:
        return bool(self._taint_domain_mask & (1 << slot))

    def join(self, *state_ids: int) -> int:
        return self._join_parents(tuple(sorted(set(state_ids))))

    def _join_parents(self, parents: tuple[int, ...]) -> int:
        """One join primitive; callers establish deterministic representative order."""
        self.join_calls += 1
        self.join_parent_inputs += len(parents)
        self.join_max_parents = max(self.join_max_parents, len(parents))
        if not parents:
            return 0
        if len(parents) == 1:
            return parents[0]
        maybe_invalidated = 0
        definitely_invalidated = ALL_INVALID_MEMBERS
        taint_epoch = 0
        namespace_values_unknown = False
        for state_id in parents:
            state = self._states[state_id]
            maybe_invalidated |= state.maybe_invalidated_members
            definitely_invalidated &= state.definitely_invalidated_members
            taint_epoch = max(taint_epoch, state.taint_epoch)
            namespace_values_unknown |= state.namespace_values_unknown
        return self.intern(
            _BindingState(
                parents=parents,
                taint_epoch=taint_epoch,
                maybe_invalidated_members=maybe_invalidated,
                definitely_invalidated_members=definitely_invalidated,
                namespace_values_unknown=namespace_values_unknown,
            )
        )

    def equivalent(self, left_id: int, right_id: int) -> bool:
        if left_id == right_id:
            return True
        left = self._states[left_id]
        right = self._states[right_id]
        if (
            left.maybe_invalidated_members != right.maybe_invalidated_members
            or left.namespace_values_unknown != right.namespace_values_unknown
            or left.definitely_invalidated_members
            != right.definitely_invalidated_members
        ):
            return False
        return (
            next(self._binding_differences(left_id, right_id, semantic=True), None)
            is None
        )

    def owner_tokens_equal(self, left_id: int, right_id: int) -> bool:
        """Compare custody separately, including epoch changes on shared storage."""
        if left_id == right_id:
            return True
        left_epoch = self._states[left_id].taint_epoch
        right_epoch = self._states[right_id].taint_epoch
        for chunk_index, left, right, remaining in self._changed_chunks(
            left_id,
            right_id,
            shared_slots=self._taint_domain_mask if left_epoch != right_epoch else 0,
        ):
            while remaining:
                bit = remaining & -remaining
                remaining ^= bit
                offset = bit.bit_length() - 1
                if not (left.owner_tokens[offset] or right.owner_tokens[offset]):
                    continue
                slot = (chunk_index << _BINDING_CHUNK_SHIFT) | offset
                old = self._resolve_chunk_binding(left, slot, left_epoch)
                new = self._resolve_chunk_binding(right, slot, right_epoch)
                if (old.owner_token if old.clean else 0) != (
                    new.owner_token if new.clean else 0
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
    source: PythonSourceKey
    parent: _Scope | None
    kind: _ScopeKind
    name: str
    locals: frozenset[str]
    globals: frozenset[str]
    nonlocals: frozenset[str]
    slots: dict[str, int]
    activation_namespace_stable: bool
    source_node: ast.AST | None = field(default=None, repr=False, compare=False)
    dynamic_class_namespace: bool = False
    annotation_evaluator: bool = False
    needs_annotation_namespace: bool = False
    escaped_deferred: frozenset[DeferredExecution] = frozenset()
    escaped_source_origins: frozenset[PythonSourceKey] = frozenset()
    returned_source_origins: frozenset[PythonSourceKey] = frozenset()
    returned_exposed_source_origins: frozenset[PythonSourceKey] = frozenset()
    returns_calls_only: bool = True
    yielded_source_origins: frozenset[PythonSourceKey] = frozenset()
    yielded_exposed_source_origins: frozenset[PythonSourceKey] = frozenset()
    escaped_metadata_effects: EffectMask = NO_EFFECTS
    module_metadata_effects: EffectMask = NO_EFFECTS
    deferred_execution: frozenset[DeferredExecution] = frozenset()

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


def _intrinsic_acquisition_identity(
    node: ast.Call, arguments: tuple[StaticExpressionResult, ...]
) -> IdentityMask:
    """Possible successful results of the canonical intrinsic acquisition API.

    Argument execution and failure remain owned by the normal call transfer.
    Unknown selectors/expansions join the declared callable identities with
    OTHER; they never establish an exact target-property specialization.
    """
    unknown = ALL_INTRINSIC_CALLABLE_IDENTITIES | OTHER_IDENTITY
    if any(isinstance(argument, ast.Starred) for argument in node.args) or any(
        keyword.arg is None for keyword in node.keywords
    ):
        return unknown
    if len(node.args) > 2 or len(arguments) != len(node.args) + len(node.keywords):
        return OTHER_IDENTITY
    bound = dict(zip(("name", "namespace"), arguments[: len(node.args)]))
    for keyword, argument in zip(
        node.keywords, arguments[len(node.args) :], strict=True
    ):
        name = keyword.arg
        if name is None or name not in {"name", "namespace"} or name in bound:
            return OTHER_IDENTITY
        bound[name] = argument
    name = bound.get("name")
    if name is None:
        return OTHER_IDENTITY
    if not name.value_known:
        return unknown
    if type(name.value) is not str:
        return OTHER_IDENTITY
    return int(INTRINSIC_CALLABLE_IDENTITIES.get(name.value, PythonIdentity.OTHER))


def _target_property_result(
    target_property: PythonTargetProperty,
    sys_platform: str | None,
    target_python: tuple[int, int],
) -> StaticExpressionResult:
    # Result proofs never erase acquisition, invocation, allocation failure or
    # operand retirement. Existing conditional lowering keeps evaluation.
    match target_property:
        case PythonTargetProperty.SYS_VERSION_INFO:
            # alloc_sys_version_info_tuple owns five fields. This policy proves
            # only major/minor; do not manufacture micro/releaselevel/serial.
            items = tuple(
                ExpressionSequenceItem(result)
                for result in (
                    StaticExpressionResult.scalar(target_python[0]),
                    StaticExpressionResult.scalar(target_python[1]),
                    StaticExpressionResult(kind="int", release_may_call=False),
                    StaticExpressionResult(kind="str", release_may_call=False),
                    StaticExpressionResult(kind="int", release_may_call=False),
                )
            )
            return StaticExpressionResult(
                truth=True,
                kind="tuple",
                identities=int(PythonIdentity.INERT_VALUE),
                evaluation_required=True,
                items=items,
                length=5,
                fresh_container=True,
                release_may_call=False,
            )
        case PythonTargetProperty.SYS_PLATFORM:
            value = sys_platform
        case PythonTargetProperty.OS_NAME:
            value = (
                None
                if sys_platform is None
                else "nt"
                if sys_platform == "win32"
                else "posix"
            )
        case _:
            assert_never(target_property)
    if value is None:
        return UNKNOWN_EXPRESSION_RESULT
    return StaticExpressionResult.scalar(value, evaluation_required=True)


def _identity_can_release(mask: IdentityMask) -> bool:
    # Identity is not retained-owner custody. Modules/functions can be evicted
    # from registries while private aliases remain exact; dropping the last
    # alias can release contents or run weakref callbacks. Locals/frame owners
    # can similarly escape their defining invocation (including 3.12 locals).
    # Only immortal absence/false sentinels and the globals dictionary still
    # rooted by the executing function are intrinsically safe here. INERT_VALUE
    # is a value/truth class, not a retained-owner proof; result facts own its
    # release safety.
    rooted = _RELEASE_ROOTED_IDENTITIES
    return bool(mask & ~rooted)


def _binding_read(node: ast.expr) -> bool:
    """Whether evaluating ``node`` yields only references that a binding, an
    owner's attribute or the code object keeps: a name, a constant, an
    attribute of such a read, or a tuple display of those. The caller proves
    the evaluation ran no Python (no descriptor or ``__getattr__``)."""
    if isinstance(node, (ast.Name, ast.Constant)):
        return True
    if isinstance(node, ast.Attribute):
        return _binding_read(node.value)
    return isinstance(node, ast.Tuple) and all(
        _binding_read(element) for element in node.elts
    )


def _plain_load(node: ast.expr) -> bool:
    """A name or constant load: it runs no Python."""
    return isinstance(node, (ast.Name, ast.Constant))


def _release_may_call(identities: IdentityMask, result: StaticExpressionResult) -> bool:
    """Compose independent rooted-identity and exact-result release proofs."""

    return _identity_can_release(identities) and result.release_may_call


def _operand_dispatch_deferred(
    effects: EffectMask,
    *operands: StaticExpressionResult,
    reaches_contents: bool = False,
    protocol: Literal["protocol", "truth"] = "protocol",
) -> frozenset[DeferredExecution]:
    """Attach operands to their own dispatch boundary, never inherited effects.

    Protocols can run an object's methods, not a stored function's body. Only
    escape-phase object candidates participate; iteration/factory resumption
    and arbitrary foreign calls keep their distinct existing phase transfers.
    Hashing, equality, repr, indexing and release may visit contained objects.

    One phase serves every operand candidate, whether it arrives as a source
    origin or as an escape-phase deferred reference: the operation decides
    which protocol of the operand can run, not the way the candidate was
    tracked. A descriptor-only boundary (an attribute store or load) runs the
    descriptor hooks and may release the overwritten value; it does not hand
    the operand's class namespace to unknown code, so it never escapes it.
    An escaped module reference stays escaped: its namespace is the
    dispatch target.
    """
    if not effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS:
        return frozenset()
    phase: SourceDemandPhase = (
        "release"
        if not effects & ~_RELEASE_CALLBACK_EFFECTS
        else "descriptor"
        if effects & INVOKES_DESCRIPTOR
        and not effects & _NON_DESCRIPTOR_CALLBACK_EFFECTS
        else protocol
    )
    phases: tuple[SourceDemandPhase, ...] = (
        (phase, "release")
        if phase == "descriptor" and effects & _RELEASE_CALLBACK_EFFECTS
        else (phase,)
    )
    sources = frozenset(
        DeferredExecution(source, candidate_phase)
        for operand in operands
        for source in (
            operand.exposed_source_origins
            if reaches_contents
            else operand.source_origins
        )
        for candidate_phase in phases
    )
    deferred = frozenset(
        DeferredExecution(ref.source, "escape")
        if ref.source[-1] == "Module"
        else DeferredExecution(ref.source, candidate_phase)
        for operand in operands
        for ref in (operand.exposed_deferred if reaches_contents else operand.deferred)
        if ref.phase == "escape"
        for candidate_phase in phases
    )
    return sources | deferred


def _owned_iterable_display(iterable: ast.expr) -> bool:
    """A list, tuple or set display in a loop or comprehension header.

    The header builds it fresh and holds the only reference, so no body write
    can change which elements it yields; a write can still change a mutable
    element's contents, which expression_result_without_mutable_contents
    expires. Any other iterable may alias what the body writes.
    """
    return isinstance(iterable, (ast.List, ast.Tuple, ast.Set))


def _result_after_retained_boundary(
    result: StaticExpressionResult, boundary: EffectMask
) -> StaticExpressionResult:
    """Drop alias-sensitive facts when a retained value can be exposed."""

    return (
        expression_result_without_mutable_contents(result)
        if boundary & _CALLEE_ELISION_FORBIDDEN_EFFECTS
        else result
    )


def _result_after_iteration_boundary(
    result: StaticExpressionResult, boundary: EffectMask
) -> StaticExpressionResult:
    """Retained builtin iterators do not publish an unaliased outer container.

    A loop body can still mutate yielded descendants. Use the same result-DAG
    invalidation as tracked nonalias owners, retaining only the private outer
    allocation. Published iterables keep the ordinary boundary rule.
    """
    if result.fresh_container and boundary & _CALLEE_ELISION_FORBIDDEN_EFFECTS:
        return expression_result_without_mutable_contents(result, preserve_owner=True)
    return _result_after_retained_boundary(result, boundary)


@dataclass(frozen=True, slots=True)
class _EvaluatedMemberTarget:
    node: ast.Attribute | ast.Subscript
    owner_identities: IdentityMask
    owner_result: StaticExpressionResult
    static_index: PythonStaticValue = None
    index_result: StaticExpressionResult = UNKNOWN_EXPRESSION_RESULT
    slice_parts: tuple[StaticExpressionResult, ...] | None = None
    index_identities: IdentityMask = OTHER_IDENTITY

    @property
    def is_exact_globals_dict(self) -> bool:
        # Namespace provenance does not exclude a FunctionType activation
        # backed by a dict subclass with user-defined mutation methods.
        return current_globals_dict_is_exact(self.owner_identities, self.owner_result)


@dataclass(slots=True)
class _ExpressionResult:
    state_id: int
    identities: IdentityMask
    effects: EffectMask
    static_value: PythonStaticValue = None
    assignment_target: _EvaluatedMemberTarget | None = None
    result: StaticExpressionResult = UNKNOWN_EXPRESSION_RESULT
    owner_token: int = 0


@dataclass(frozen=True, slots=True)
class _CallSemantics:
    """Normal-call transfer plus custody of the evaluated callee temporary."""

    result_identities: IdentityMask
    invocation_effects: EffectMask
    state_id: int
    result: StaticExpressionResult
    specialization_valid: bool
    argument_effects: EffectMask
    callee_release_safe: bool = False
    retained_argument_indices: frozenset[int] = frozenset()
    # None retains generic-call exposure. A closed set projects only the actual
    # operand protocols of a proven callee; stored values remain explicit escapes.
    argument_protocol_indices: frozenset[int] | None = None
    argument_escape_indices: frozenset[int] = frozenset()
    deferred_execution: frozenset[DeferredExecution] = frozenset()
    source_result_projected: bool = False
    borrowed_module_operand: int | None = None


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
    parameter_defaults: tuple[tuple[str, StaticExpressionResult], ...]
    annotation_scope: bool = False


@dataclass(slots=True)
class _HistorySummary:
    states: Sequence[int]
    maybe_suffix: tuple[MemberMask, ...]
    definitely_suffix: tuple[MemberMask, ...]
    taint_epoch_suffix: tuple[int, ...]
    namespace_unknown_suffix: tuple[bool, ...]
    taint_indices: tuple[int, ...]
    slot_events: dict[int, list[int]]
    slot_suffixes: dict[
        int,
        tuple[
            tuple[int, ...],
            tuple[IdentityMask, ...],
            tuple[StaticExpressionResult, ...],
        ],
    ]
    domain_generation: int
    state_count: int
    stored_slot_limit: int
    domain_mask: int
    callback_deletion_mask: int
    storage_limit_exact: bool = False

    @classmethod
    def build(cls, pool: _StatePool, states: Sequence[int]) -> _HistorySummary:
        count = len(states)
        maybe_suffix = [0] * count
        definitely_suffix = [ALL_INVALID_MEMBERS] * count
        taint_epoch_suffix = [0] * count
        namespace_unknown_suffix = [False] * count
        maybe = 0
        definitely = ALL_INVALID_MEMBERS
        taint_epoch = 0
        namespace_unknown = False
        stored_chunks = 0
        for index in range(count - 1, -1, -1):
            state = pool.get(states[index])
            stored_chunks = max(
                stored_chunks, pool._binding_environments[states[index]].chunk_count
            )
            maybe |= state.maybe_invalidated_members
            definitely &= state.definitely_invalidated_members
            taint_epoch = max(taint_epoch, state.taint_epoch)
            namespace_unknown |= state.namespace_values_unknown
            maybe_suffix[index] = maybe
            definitely_suffix[index] = definitely
            taint_epoch_suffix[index] = taint_epoch
            namespace_unknown_suffix[index] = namespace_unknown

        events: dict[int, list[int]] = {}
        taint_indices: list[int] = []
        for index in range(1, count):
            previous = states[index - 1]
            current = states[index]
            if pool.get(current).taint_epoch > pool.get(previous).taint_epoch:
                taint_indices.append(index)
            for slot in pool.transition_binding_slots(previous, current):
                events.setdefault(slot, []).append(index)

        return cls(
            states,
            tuple(maybe_suffix),
            tuple(definitely_suffix),
            tuple(taint_epoch_suffix),
            tuple(namespace_unknown_suffix),
            tuple(taint_indices),
            events,
            {},
            pool.taint_domain_generation,
            count,
            stored_chunks << _BINDING_CHUNK_SHIFT,
            pool._taint_domain_mask,
            pool._callback_deletion_mask,
        )

    def refresh(self, pool: _StatePool) -> None:
        if self.state_count == len(self.states):
            if self.domain_generation == pool.taint_domain_generation:
                return
            added = (pool._taint_domain_mask & ~self.domain_mask) | (
                pool._callback_deletion_mask & ~self.callback_deletion_mask
            )
            if (
                added
                and not self.storage_limit_exact
                and (added & -added).bit_length() <= self.stored_slot_limit
            ):
                # The final radix chunk only gives an upper bound. Refine it
                # when an admitted slot first lands inside that chunk.
                last_chunk = (self.stored_slot_limit - 1) >> _BINDING_CHUNK_SHIFT
                last_mask = 0
                for state in self.states:
                    environment = pool._binding_environments[state]
                    if environment.chunk_count == last_chunk + 1:
                        chunk = pool._chunk_at(environment, last_chunk)
                        last_mask |= chunk.active_mask | chunk.clean_mask
                        if last_mask.bit_length() == _BINDING_CHUNK_SIZE:
                            break
                self.stored_slot_limit = (
                    last_chunk << _BINDING_CHUNK_SHIFT
                ) + last_mask.bit_length()
                self.storage_limit_exact = True
            if not added or (added & -added).bit_length() > self.stored_slot_limit:
                # New activation slots cannot alter this completed history's
                # immutable storage. Absent slots project exposure on query;
                # their admission must not replay every historical transition.
                self.domain_generation = pool.taint_domain_generation
                self.domain_mask = pool._taint_domain_mask
                self.callback_deletion_mask = pool._callback_deletion_mask
                return
        current = type(self).build(pool, self.states)
        for item in fields(self):
            setattr(self, item.name, getattr(current, item.name))

    def properties(self, start: int) -> tuple[MemberMask, MemberMask, int, bool]:
        return (
            self.maybe_suffix[start],
            self.definitely_suffix[start],
            self.taint_epoch_suffix[start],
            self.namespace_unknown_suffix[start],
        )

    def project(
        self, pool: _StatePool, start: int, slot: int
    ) -> tuple[IdentityMask, StaticExpressionResult]:
        self.refresh(pool)
        # Exposure before start remains live even when there was no stored write.
        pool.binding_lookups += 1
        value, _, observed, _ = pool._binding_details(self.states[start], slot)
        result = expression_result_without_value_facts(observed)
        suffix = self.slot_suffixes.get(slot)
        if suffix is None and (events := self.slot_events.get(slot)) is not None:
            # Close over the events for this slot once. Module and closure
            # histories often contain temporaries that no nested body reads.
            suffix_values = [NO_IDENTITIES] * len(events)
            suffix_results = [UNKNOWN_EXPRESSION_RESULT] * len(events)
            future_value = NO_IDENTITIES
            future_result = UNKNOWN_EXPRESSION_RESULT
            for index in range(len(events) - 1, -1, -1):
                identities, _, observed, _ = pool._binding_details(
                    self.states[events[index]], slot
                )
                future_value |= identities
                suffix_values[index] = future_value
                future_result = join_static_expression_results(
                    (future_result, expression_result_without_value_facts(observed))
                )
                suffix_results[index] = future_result
            suffix = (
                tuple(events),
                tuple(suffix_values),
                tuple(suffix_results),
            )
            self.slot_suffixes[slot] = suffix
            del self.slot_events[slot]
        if suffix is not None:
            indices, suffix_values, suffix_results = suffix
            event_index = bisect_left(indices, start)
            if event_index < len(indices):
                value |= suffix_values[event_index]
                result = join_static_expression_results(
                    (result, suffix_results[event_index])
                )
        if pool.slot_in_taint_domain(slot) and bisect_left(
            self.taint_indices, start
        ) < len(self.taint_indices):
            # A callback may introduce a name as well as overwrite it.
            value |= OTHER_IDENTITY
            if pool._callback_deletion_mask & (1 << slot):
                value |= UNBOUND_IDENTITY
        return value, result


@dataclass(slots=True)
class _StatementSequenceFrame:
    body: Sequence[ast.stmt]
    flow: PythonCompletionFlow[int]
    position: int = 0


@dataclass(slots=True)
class _ConditionalStatementFrame:
    node: ast.If
    prefix: PythonCompletionFlow[int]
    branches: tuple[Sequence[ast.stmt], ...]
    observation_before: int
    flow: PythonCompletionFlow[int] = field(default_factory=PythonCompletionFlow)
    position: int = 0


@dataclass(slots=True)
class _ObservedStateFrame:
    """Full lexical observations plus a raw-custody incremental exceptional fold.

    The list is never coalesced: closure-history positions and original ancestors
    remain authoritative. An accumulator is a normal state with all path writes.
    """

    states: list[int]
    folded_count: int = 0
    accumulator: int | None = None
    folded_ids: set[int] = field(default_factory=set)
    largest_parent: int = -1
    # Operation dependencies use this existing statement observation, alongside
    # the states they affect. No second source/context registry is maintained.
    deferred_execution: frozenset[DeferredExecution] = frozenset()
    module_metadata_effects: EffectMask = NO_EFFECTS
    # The join of states that a failed frame-local store leaves in place. The
    # next completion flow built in this observation raises from it; one the
    # statement leaves unrouted raises from the statement itself.
    failed_store: int | None = None

    def exceptional_state(self, pool: _StatePool) -> int:
        pool.observation_fold_calls += 1
        rebuild = False
        pending = sorted(set(self.states[self.folded_count :]) - self.folded_ids)
        # An old interned state can be observed later (alternative/loop entry).
        # It must precede newer originals, not an opaque accumulated representative.
        rebuild |= bool(pending and pending[0] < self.largest_parent)
        if rebuild:
            if self.accumulator is not None:
                pool.observation_fold_rebuilds += 1
            self.accumulator = None
            self.folded_ids.clear()
            self.largest_parent = -1
            pending = sorted(set(self.states))
        if pending:
            parents = tuple(pending)
            if self.accumulator is not None:
                parents = (self.accumulator, *parents)
            self.accumulator = pool._join_parents(parents)
            pool.observation_fold_new_parents += len(pending)
            self.folded_ids.update(pending)
            self.largest_parent = pending[-1]
        self.folded_count = len(self.states)
        assert self.accumulator is not None, "observations require an entry state"
        return self.accumulator


class _BindingReplayRefused(Exception):
    """A recorded transfer cannot reproduce this abstract execution."""


@dataclass(slots=True)
class _BindingFactAccess:
    reads: set[object] = field(default_factory=set)
    writes: set[object] = field(default_factory=set)
    reads_existing: bool = False
    enumerates: bool = False


class _ObservedBindingTable[K, V](dict[K, V]):
    """Record each job's access to facts supplied by earlier transfers."""

    def __init__(
        self,
        values: Mapping[K, V],
        *,
        prior: tuple[Mapping[object, object], _BindingFactAccess | None] | None = None,
    ) -> None:
        super().__init__(values)
        self.observing = False
        self.access: _BindingFactAccess | None = None
        self.prior = prior

    def _read(self, key: object) -> _BindingFactAccess:
        access = self.access
        if access is None:
            self.access = access = _BindingFactAccess()
        if key in access.writes or key in access.reads:
            return access
        if self.prior is not None:
            table, old_access = self.prior
            if key in table and (old_access is None or key not in old_access.writes):
                raise _BindingReplayRefused
        access.reads.add(key)
        access.reads_existing |= super().__contains__(key)
        return access

    def _enumerate(self) -> None:
        if self.prior is not None:
            raise _BindingReplayRefused
        if self.access is None:
            self.access = _BindingFactAccess()
        self.access.enumerates = True

    def __getitem__(self, key: K) -> V:
        if self.observing:
            self._read(key)
        return super().__getitem__(key)

    def __contains__(self, key: object) -> bool:
        if self.observing:
            self._read(key)
        return super().__contains__(key)

    @overload
    def get(self, key: object, default: None = None, /) -> V | None: ...

    @overload
    def get(self, key: object, default: V, /) -> V: ...

    @overload
    def get[D](self, key: object, default: D, /) -> V | D: ...

    def get[D](self, key: object, default: D | None = None, /) -> V | D | None:
        if self.observing:
            self._read(key)
        return super().get(key, default)

    def setdefault(self, key: K, default: V, /) -> V:
        if self.observing:
            access = self._read(key)
            if not super().__contains__(key):
                access.writes.add(key)
        return super().setdefault(key, default)

    def __setitem__(self, key: K, value: V) -> None:
        if self.observing:
            self._read(key).writes.add(key)
        super().__setitem__(key, value)

    def __iter__(self) -> Iterator[K]:
        if self.observing:
            self._enumerate()
        return super().__iter__()

    def __len__(self) -> int:
        if self.observing:
            self._enumerate()
        return super().__len__()

    def copy(self) -> dict[K, V]:
        if self.observing:
            self._enumerate()
        return super().copy()

    def keys(self) -> dict_keys[K, V]:
        if self.observing:
            self._enumerate()
        return super().keys()

    def items(self) -> dict_items[K, V]:
        if self.observing:
            self._enumerate()
        return super().items()

    def values(self) -> dict_values[K, V]:
        if self.observing:
            self._enumerate()
        return super().values()


@dataclass
class _BindingTransferFacts:
    """Source facts accumulated by the initializer and deferred jobs before projection."""

    expressions: dict[PythonSourceKey, PythonExpressionFact] = field(
        default_factory=dict
    )
    class_builder_reads: dict[PythonSourceKey, IdentityMask] = field(
        default_factory=dict
    )
    decoration_annotation_heads: dict[PythonSourceKey, IdentityMask] = field(
        default_factory=dict
    )
    source_attributes: dict[PythonSourceKey, dict[str, frozenset[PythonSourceKey]]] = (
        field(default_factory=dict)
    )
    source_elements: dict[PythonSourceKey, PythonSourceElement] = field(
        default_factory=dict
    )
    source_calls: dict[PythonSourceKey, PythonSourceCall] = field(default_factory=dict)
    source_native_initializers: dict[PythonSourceKey, PythonNativeInitializerSource] = (
        field(default_factory=dict)
    )
    source_classes: dict[PythonSourceKey, PythonSourceClass] = field(
        default_factory=dict
    )
    source_callables: dict[PythonSourceKey, IdentityMask] = field(default_factory=dict)
    source_completions: dict[PythonSourceKey, frozenset[PythonSourceKey]] = field(
        default_factory=dict
    )
    source_callable_declarations: dict[
        PythonSourceKey, PythonSourceCallableDeclaration
    ] = field(default_factory=dict)
    executed_function_nodes: dict[
        PythonSourceKey, ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda
    ] = field(default_factory=dict)
    source_function_results: dict[PythonSourceKey, PythonFunctionResult] = field(
        default_factory=dict
    )
    source_namespaces: dict[PythonSourceKey, dict[str, frozenset[PythonSourceKey]]] = (
        field(default_factory=dict)
    )
    source_bases: dict[PythonSourceKey, frozenset[PythonSourceKey]] = field(
        default_factory=dict
    )
    source_namespace_exposure: dict[PythonSourceKey, frozenset[PythonSourceKey]] = (
        field(default_factory=dict)
    )
    _builtin_guard_reads: dict[PythonSourceKey, str] = field(default_factory=dict)
    statements: dict[PythonSourceKey, PythonStatementFact] = field(default_factory=dict)
    iterations: dict[PythonSourceKey, PythonIterationFact] = field(default_factory=dict)
    assignment_effects: dict[PythonSourceKey, EffectMask] = field(default_factory=dict)
    calls: dict[PythonSourceKey, PythonCallSiteFact] = field(default_factory=dict)

    def tables(self) -> tuple[dict[object, object], ...]:
        return tuple(getattr(self, item.name) for item in fields(self))

    def watch_tables(
        self,
        prior: tuple[tuple[Mapping[object, object], _BindingFactAccess | None], ...]
        | None = None,
    ) -> tuple[_ObservedBindingTable[object, object], ...]:
        watched = []
        for index, item in enumerate(fields(self)):
            table = _ObservedBindingTable(
                getattr(self, item.name),
                prior=None if prior is None else prior[index],
            )
            setattr(self, item.name, table)
            watched.append(table)
        return tuple(watched)


@dataclass(frozen=True, slots=True)
class _DeferredJobState:
    entry: _BindingEntryState
    scope_count: int
    slot_count: int
    observation_epoch: int
    next_owner_token: int
    callback_slots: int
    callback_module_slots: int
    callback_module_names: frozenset[str] | None
    import_flow_required: bool
    read_slots_prepared: bool


@dataclass(frozen=True, slots=True)
class _DeferredJobReads:
    activation_groups: tuple[frozenset[PythonSourceKey], ...]
    foreign_names: frozenset[str]
    empty_module_expiry: bool
    facts: tuple[_BindingFactAccess | None, ...]
    earlier_scope_writes: frozenset[int]
    state: _DeferredJobState | None
    scope_end: int
    slot_end: int
    next_owner_token: int
    callback_slots: int
    callback_deletions: int
    import_flow_required: bool
    read_slots_prepared: bool


class _DeferredJobRecorder:
    def __init__(self, analyzer: _Analyzer) -> None:
        self.tables = analyzer.transfer.watch_tables()
        self.initializer_reads = analyzer._opaque_activation_reads.copy()
        self.initializer_foreign_names = frozenset(analyzer._foreign_reads or ())
        self.initializer_empty_expiry = analyzer._has_empty_module_expiry
        self.jobs: list[_DeferredJobReads] = []

    def run(self, analyzer: _Analyzer, job: _FunctionJob, state_id: int) -> None:
        scope_count = len(analyzer.scopes)
        slot_count = len(analyzer.slot_names)
        queue_generation = analyzer._deferred_queue_generation
        state: _DeferredJobState | None = _DeferredJobState(
            analyzer.states.deferred_entry(state_id),
            scope_count,
            slot_count,
            analyzer._namespace_observation_epoch,
            analyzer._next_owner_token,
            analyzer.callback_slot_mask,
            analyzer.callback_module_slot_mask,
            analyzer._callback_module_names,
            analyzer._module_import_flow_required,
            analyzer._module_read_slots_prepared,
        )
        for table in self.tables:
            table.observing = True
        groups: dict[frozenset[PythonSourceKey], None] = {}
        earlier_scope_writes: set[int] = set()
        analyzer._recorded_activation_groups = groups
        prior_names = analyzer._foreign_reads
        job_names: set[str] = set()
        if prior_names is not None:
            # A job owns the namespace reads behind its slot lookups, even
            # when an earlier transfer already looked up the same slot.
            analyzer._scope_slot_cache.clear()
            analyzer._foreign_reads = job_names
        prior_expiry = analyzer._has_empty_module_expiry
        analyzer._has_empty_module_expiry = False
        analyzer._deferred_scope_writes = (scope_count, earlier_scope_writes)
        try:
            analyzer._analyze_function_job(job, state_id)
        finally:
            analyzer._recorded_activation_groups = None
            analyzer._deferred_scope_writes = None
            analyzer._foreign_reads = prior_names
            if prior_names is not None:
                prior_names.update(job_names)
            empty_module_expiry = analyzer._has_empty_module_expiry
            analyzer._has_empty_module_expiry |= prior_expiry
            accesses = tuple(table.access for table in self.tables)
            for table in self.tables:
                table.observing = False
                table.access = None
        # A replay may own the function and its comprehension scopes in one interval.
        if (
            not groups
            or earlier_scope_writes
            or len(analyzer.scopes) <= scope_count
            or any(
                scope.kind != "comprehension"
                for scope in analyzer.scopes[scope_count + 1 :]
            )
            or analyzer._deferred_queue_generation != queue_generation
            or analyzer.callback_module_slot_mask != state.callback_module_slots
            or any(
                access.reads_existing or access.enumerates
                for access in accesses
                if access is not None
            )
        ):
            state = None
        self.jobs.append(
            _DeferredJobReads(
                tuple(groups),
                frozenset(job_names),
                empty_module_expiry,
                accesses,
                frozenset(earlier_scope_writes),
                state,
                len(analyzer.scopes),
                len(analyzer.slot_names),
                analyzer._next_owner_token,
                analyzer.callback_slot_mask,
                analyzer.states._callback_deletion_mask,
                analyzer._module_import_flow_required,
                analyzer._module_read_slots_prepared,
            )
        )


@dataclass(slots=True)
class _InitializerPremiseReads:
    names: frozenset[str] = frozenset()
    metadata: dict[tuple[int, int], PythonModuleMetadataProof] = field(
        default_factory=dict
    )
    frame_borrows: bool = False
    deferred_names: set[str] = field(default_factory=set)


@dataclass(frozen=True, slots=True)
class _InitializerContinuation:
    completion: PythonCompletionFlow[int]
    observation: _ObservedStateFrame
    history_job: int | None


class _Analyzer:
    def __init__(self, policy: PythonBindingFlowPolicy, source_digest: str) -> None:
        self.policy = policy
        self._activation_argument_values = dict(policy.activation_argument_values)
        self._call_argument_plan: Callable[
            [ast.Call | ast.ClassDef], CallArgumentPlan
        ] = call_argument_plan
        self._opaque_activation_sources = (
            None
            if policy.opaque_activation_sources is None
            else frozenset(policy.opaque_activation_sources)
        )
        self._opaque_activation_reads: dict[frozenset[PythonSourceKey], bool] = {}
        self._opaque_lexical_activations: dict[
            int, tuple[frozenset[PythonSourceKey], bool]
        ] = {}
        self._recorded_activation_groups: (
            dict[frozenset[PythonSourceKey], None] | None
        ) = None
        self._foreign_universe = policy.foreign_namespace_universe
        self._foreign_reads: set[str] | None = None
        self._foreign_slot_bounds = (0, 0)
        self._appended_foreign_slots: tuple[int, ...] = ()
        self._foreign_tainted_at_exit = False
        self._module_getattr_may_be_bound = False
        self._has_empty_module_expiry = False
        self._initializer_premise_reads: _InitializerPremiseReads | None = None
        self._capture_initializer: (
            Callable[[PythonCompletionFlow[int]], None] | None
        ) = None
        self.source_digest = source_digest
        self.states = _StatePool()
        self.transfer = _BindingTransferFacts()
        self.scopes: list[_Scope] = []
        self.slot_names: list[str] = []
        self.slot_owner_scope_ids: list[int] = []
        # Native namespace freshness is a binding in the existing state pool.
        self._class_namespace_slots: dict[int, int] = {}
        # Each class's implicit `__class__` cell (PEP 3135), by class scope id.
        self._class_cell_slots: dict[int, int] = {}
        self._class_cell_slot_mask = 0
        # `id` of each call that is an attribute load's receiver expression.
        self._super_attribute_receivers: set[int] = set()
        # Star-import aliases of this module. A star import binds names that no
        # source node spells, so a module-namespace read selects its name from
        # each star-imported module instead of exposing those modules whole.
        self._star_import_origins: frozenset[PythonSourceKey] = frozenset()
        self._namespace_observation_epoch = 0
        self._future_annotations = False
        self._dependency_authority: PythonDependencyAuthority | None = None
        self.function_jobs: list[_FunctionJob] = []
        self._deferred_queue_generation = 0
        self._replaying_deferred = False
        self._deferred_scope_writes: tuple[int, set[int]] | None = None
        self._queued_function_jobs: dict[
            tuple[PythonSourceKey, int, bool], _FunctionJob
        ] = {}
        self._source_scopes: dict[
            tuple[PythonSourceKey, int | None, _ScopeKind], _Scope
        ] = {}
        self._active_lexical_history: list[int] | None = None
        self.module_scope: _Scope | None = None
        self.module_slots: set[int] = set()
        self.module_slot_mask = 0
        # None until a graph premise bounds the globals a callback can rebind.
        self._callback_module_names: frozenset[str] | None = None
        self.callback_module_slot_mask = 0
        self.callback_slot_mask = 0
        self._observed_stack: list[_ObservedStateFrame] = []
        self._module_history: list[int] = [0]
        self._active_module_states: tuple[int, ...] | None = None
        self._module_import_flow_required = False
        self._scope_slot_cache: dict[tuple[int, str], int | None] = {}
        self._history_summaries: dict[int, _HistorySummary] = {}
        self._next_owner_token = 1
        # Reads awaiting their source consumer, keyed by lexical storage slot.
        # A store invalidates a borrow, not the value already evaluated from it.
        # These operation-local scopes use the same writes/effects as binding
        # analysis; no second AST walk or spelling-based walrus classifier.
        self._binding_read_scopes: list[dict[int, set[PythonSourceKey]]] = []
        self._ast_admission: _PythonAstDigestAdmission | None = None
        self._node_keys: dict[ast.AST, PythonSourceKey] = {}

    @property
    def _eager_annotations(self) -> bool:
        return self.policy.target_python < (3, 14) and not self._future_annotations

    def _node_key(self, node: ast.AST) -> PythonSourceKey:
        admission = self._ast_admission
        if admission is not None:
            return admission.source_key(node)
        key = self._node_keys.get(node)
        if key is None:
            key = python_node_source_key(node)
            self._node_keys[node] = key
        return key

    @staticmethod
    def _target_may_write_import_metadata(target: ast.AST) -> bool:
        if isinstance(target, (ast.Tuple, ast.List)):
            return any(
                _Analyzer._target_may_write_import_metadata(element)
                for element in target.elts
            )
        return import_metadata_target_name(target) in MODULE_IMPORT_METADATA_NAMES

    def _queue_function(
        self,
        node: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda,
        scope: _Scope,
        state_id: int,
        parameter_defaults: tuple[tuple[str, StaticExpressionResult], ...] = (),
        *,
        annotation_scope: bool = False,
    ) -> None:
        if not annotation_scope:
            self._declare_function(node)
        if (
            self.policy.declaration_only
            and not annotation_scope
            and (self._node_key(node) in self.policy.declaration_only)
        ):
            return
        if self._replaying_deferred:
            raise _BindingReplayRefused
        self._deferred_queue_generation += 1
        reads = self._initializer_premise_reads
        source_reads = self._foreign_reads
        if reads is not None and source_reads is not None:
            self._foreign_reads = reads.deferred_names
        try:
            module_bindings, lexical_bindings = self._activation_slot_bindings(
                node,
                scope,
                state_id,
                annotation_scope=annotation_scope,
            )
        finally:
            self._foreign_reads = source_reads
        lexical_history = self._active_lexical_history if lexical_bindings else None
        job = _FunctionJob(
            node,
            scope,
            state_id,
            None
            if self._active_module_states is not None
            else len(self._module_history),
            self._active_module_states,
            module_bindings,
            lexical_history,
            len(lexical_history) if lexical_history is not None else 0,
            lexical_bindings,
            parameter_defaults,
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
        defaults = dict(previous.parameter_defaults)
        for name, result in parameter_defaults:
            defaults[name] = join_static_expression_results(
                (defaults.get(name, UNKNOWN_EXPRESSION_RESULT), result)
            )
        previous.parameter_defaults = tuple(defaults.items())
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

    def _activation_slot_bindings(
        self,
        node: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda | ast.GeneratorExp,
        parent_scope: _Scope,
        state_id: int,
        *,
        annotation_scope: bool = False,
    ) -> tuple[
        tuple[tuple[int, IdentityMask], ...],
        tuple[tuple[int, IdentityMask], ...],
    ]:
        """Discover and register every external input to a deferred activation."""

        assert self._dependency_authority is not None
        summary = self._dependency_authority.summary(node)
        dependencies = summary.body
        assert self.module_scope is not None
        module_slots: set[int] = {
            slot
            for name in dependencies.globals
            if (slot := self._module_slot(name)) is not None
        }
        lexical_slots: set[int] = set()
        for name in dependencies.lexical:
            slot = self._enclosing_slot(
                parent_scope,
                name,
                annotation_owner=parent_scope if annotation_scope else None,
            )
            if slot is None:
                continue
            if slot in self.module_slots:
                module_slots.add(slot)
            else:
                lexical_slots.add(slot)
        ordered_module_slots = tuple(sorted(module_slots))
        ordered_lexical_slots = tuple(sorted(lexical_slots))
        self.callback_slot_mask |= sum(1 << slot for slot in ordered_lexical_slots)
        self.states.set_taint_domain(self._callback_taint_domain())
        deletions = 0
        for name in summary.nonlocal_deletions:
            slot = self._enclosing_slot(
                parent_scope,
                name,
                annotation_owner=parent_scope if annotation_scope else None,
            )
            if slot is not None and slot in lexical_slots:
                deletions |= 1 << slot
        if deletions:
            self.states.add_callback_deletions(deletions)
        return (
            tuple(
                (slot, self.states.binding(state_id, slot))
                for slot in ordered_module_slots
            ),
            tuple(
                (slot, self.states.binding(state_id, slot))
                for slot in ordered_lexical_slots
            ),
        )

    def _activation_input_updates(
        self,
        state_id: int,
        lexical_bindings: Sequence[tuple[int, IdentityMask]],
    ) -> list[_BindingUpdate]:
        """Admit arbitrary or empty compatible cells at one activation entry."""
        updates: list[_BindingUpdate] = []
        for slot, _base_binding in lexical_bindings:
            binding = self.states._binding_resolution(state_id, slot)
            updates.append(
                (
                    slot,
                    binding.identities | OTHER_IDENTITY | UNBOUND_IDENTITY,
                    None,
                    expression_result_without_value_facts(binding.result),
                    0,
                )
            )
        return updates

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
            tuple(
                (slot, OTHER_IDENTITY, None, UNKNOWN_EXPRESSION_RESULT, 0)
                for slot in scope.slots.values()
            ),
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
        activation_namespace_stable: bool | None = None,
    ) -> _Scope:
        key = (
            self._node_key(source_node),
            parent.scope_id if parent is not None else None,
            kind,
        )
        existing = self._source_scopes.get(key)
        if existing is not None:
            self._record_deferred_scope_write(existing)
            return existing
        scope = _Scope(
            scope_id=len(self.scopes),
            source=self._node_key(source_node),
            source_node=source_node,
            parent=parent,
            kind=kind,
            name=name,
            locals=declarations.bound,
            globals=declarations.globals,
            nonlocals=declarations.nonlocals,
            slots={},
            activation_namespace_stable=(
                kind == "module"
                if parent is None
                else parent.activation_namespace_stable
            )
            if activation_namespace_stable is None
            else activation_namespace_stable,
        )
        if self.module_scope is not None:
            for global_name in sorted(scope.globals):
                self._ensure_module_slot(global_name)
        first_local_slot = len(self.slot_names)
        for local_name in sorted(scope.locals):
            scope.slots[local_name] = len(self.slot_names)
            self.slot_names.append(f"{scope.scope_id}:{local_name}")
            self.slot_owner_scope_ids.append(scope.scope_id)
        if kind == "class" or (
            frame_locals_exposed(
                self.policy.target_python, self.policy.frame_locals_writer
            )
            and kind != "module"
        ):
            # Class namespaces are mappings. PEP 667 also exposes optimized
            # activation bindings to write-through frame proxies: an arbitrary
            # callback can replace a local without a lexical/nonlocal store.
            # Use the same exposure epoch as captured cells and globals so all
            # consumers lose stale type, value, owner and callable identities.
            # A store can establish a clean fact when its displaced owner
            # cannot call back; inert operations do not advance the epoch.
            # Scope slots form one contiguous interval. Construct its bitset
            # once, without one growing arbitrary-precision addition per local.
            local_slot_mask = (1 << len(scope.slots)) - 1
            self.callback_slot_mask |= local_slot_mask << first_local_slot
            self.states.set_taint_domain(self._callback_taint_domain())
        self.scopes.append(scope)
        self._source_scopes[key] = scope
        return scope

    def _ensure_module_slot(self, name: str) -> int:
        """One slot authority for global declarations and reflective item writes."""
        existing = self._module_slot(name)
        if existing is not None:
            return existing
        assert self.module_scope is not None
        self._record_deferred_scope_write(self.module_scope)
        slot = len(self.slot_names)
        self.module_scope.slots[name] = slot
        self.slot_names.append(f"{self.module_scope.scope_id}:{name}")
        self.slot_owner_scope_ids.append(self.module_scope.scope_id)
        self.module_slots.add(slot)
        self.module_slot_mask |= 1 << slot
        if self._callback_module_names is not None and not self._foreign_name_known(
            name
        ):
            # A computed key reached a name the scan view left out: it may be
            # one the whole premise names, so callbacks may rebind it.
            self._callback_module_names = self._callback_module_names | {name}
            self.callback_module_slot_mask |= 1 << slot
        self.states.set_taint_domain(self._callback_taint_domain())
        self._scope_slot_cache.clear()
        return slot

    def _record_deferred_scope_write(self, scope: _Scope) -> None:
        # Execution mutates the scope that _new_scope returned. Slot admission
        # and annotation capture also write directly to an existing owner.
        observed = self._deferred_scope_writes
        if observed is not None and scope.scope_id < observed[0]:
            observed[1].add(scope.scope_id)

    def _foreign_name_known(self, name: str) -> bool:
        """Whether the policy states if other code writes ``name``: always for
        a whole premise, and for a scan view only inside its name universe."""
        universe = self._foreign_universe
        return universe is None or name in universe

    def _callback_rebinds_module_name(self, name: str) -> bool:
        if self._foreign_reads is not None:
            self._foreign_reads.add(name)
        names = self._callback_module_names
        return names is None or name in names or not self._foreign_name_known(name)

    def _ensure_module_read_slots(self) -> None:
        """Actual namespace writes must reach undeclared builtin/global reads."""
        if self._module_read_slots_prepared:
            return
        assert self._dependency_authority is not None
        for name in self._dependency_authority.module_read_names(self._module_tree):
            self._ensure_module_slot(name)
        self._module_read_slots_prepared = True

    def _callback_taint_domain(self) -> int:
        """Slots a callback can rebind: module globals, then captured cells."""
        module = (
            self.module_slot_mask
            if self._callback_module_names is None
            else self.callback_module_slot_mask
        )
        return module | self.callback_slot_mask

    def _taint_module_slots(self, state_id: int) -> int:
        # A new foreign slot would change whether this operation advances the
        # shared callback epoch, even if that particular name had no reader.
        if self._foreign_reads is not None and not self.callback_module_slot_mask:
            self._has_empty_module_expiry = True
        return self.states.taint_slots(state_id, self.module_slot_mask)

    def _configure_callback_namespace(
        self,
        tree: ast.Module,
        ast_digest_admission: _PythonAstDigestAdmission | None,
    ) -> None:
        """Bound the globals a callback can rebind, when the policy verifies it.

        Without a graph premise every global stays callback-exposed. With one,
        a callback rebinds only names declared ``global`` in deferred code,
        own-namespace item writes and the names other code writes. A module
        that reflects its own namespace dynamically keeps the whole namespace.
        """
        foreign = self.policy.foreign_namespace_writes
        own = (
            _own_namespace_names(
                tree,
                target_python=self.policy.target_python,
                ast_digest_admission=ast_digest_admission,
            )
            if foreign is not None
            else None
        )
        if foreign is None or own is None:
            return
        names = own | frozenset(foreign)
        self._callback_module_names = names
        for name in sorted(names):
            self.callback_module_slot_mask |= 1 << self._ensure_module_slot(name)

    def _module_slot(self, name: str) -> int | None:
        if self._foreign_reads is not None:
            self._foreign_reads.add(name)
        assert self.module_scope is not None
        return self.module_scope.slots.get(name)

    def _builtins_mapping_rebindable(self) -> bool:
        """This module's own code or a named foreign writer binds ``__builtins__``.

        An open premise (no foreign-writer list) already invalidates builtin
        reads after every callback; it does not change pristine entry reads.
        """
        foreign = self.policy.foreign_namespace_writes
        return self._module_slot("__builtins__") is not None or (
            foreign is not None and "__builtins__" in foreign
        )

    def _module_metadata_borrowing_snapshot(
        self, state_id: int, scope: _Scope
    ) -> PythonModuleMetadataProof:
        """Read callback custody and loader-pristine slots from raw storage."""
        assert self.module_scope is not None
        module_slots = self.module_scope.slots
        epoch = self.states.get(state_id).taint_epoch
        clean: set[str] = set()
        pristine: set[str] = set()
        for name in MODULE_IMPORT_METADATA_NAMES:
            # The foreign-write footprint already includes this entire family.
            slot = module_slots.get(name)
            if slot is None:
                # A bounded namespace gains no name a callback could bind.
                if epoch == 0 or self._callback_module_names is not None:
                    clean.add(name)
                pristine.add(name)
            else:
                resolution = self.states._binding_resolution(state_id, slot)
                if resolution.clean:
                    clean.add(name)
                if not resolution.present:
                    pristine.add(name)
        result = PythonModuleMetadataProof(
            scope.activation_namespace_stable, frozenset(clean), frozenset(pristine)
        )
        if self._initializer_premise_reads is not None:
            self._initializer_premise_reads.metadata[(state_id, scope.scope_id)] = (
                result
            )
        return result

    def _slot_uses_lexical_storage(self, slot: int) -> bool:
        if self._class_cell_slot_mask & (1 << slot):
            return True
        owner = self.scopes[self.slot_owner_scope_ids[slot]]
        return owner.kind not in ("module", "class")

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
        elif scope.kind == "module":
            slot = self._module_slot(name)
        else:
            slot = scope.slots.get(name)
            if slot is None:
                slot = self._enclosing_slot(
                    scope.parent,
                    name,
                    annotation_owner=scope.parent
                    if scope.kind == "annotation"
                    else None,
                )
        self._scope_slot_cache[cache_key] = slot
        return slot

    def _enclosing_slot(
        self, parent: _Scope | None, name: str, *, annotation_owner: _Scope | None
    ) -> int | None:
        """The slot `name` names in the enclosing scopes, innermost first.

        A class namespace is invisible to the scopes nested in it, with two
        exceptions: an annotation scope sees its owning class, and every nested
        scope (nested classes included) sees the class's implicit `__class__`
        cell, as CPython's symtable passes `__class__` to a class's children.
        """
        while parent is not None:
            if parent.kind == "module":
                return self._module_slot(name)
            if parent.kind != "class" or parent is annotation_owner:
                slot = parent.slots.get(name)
                if slot is not None:
                    return slot
            if parent.kind == "class" and name == "__class__":
                return self._class_cell_slot(parent)
            parent = parent.parent
        return None

    def _class_cell_slot(self, class_scope: _Scope) -> int:
        """The class's implicit `__class__` cell, a closure cell of its methods.

        Class creation fills it, outside the analyzed flow, so it is exposed to
        callbacks: a read before class creation may find it unbound.
        """
        slot = self._class_cell_slots.get(class_scope.scope_id)
        if slot is None:
            slot = len(self.slot_names)
            self.slot_names.append(f"{class_scope.scope_id}:__class__ cell")
            self.slot_owner_scope_ids.append(class_scope.scope_id)
            self._class_cell_slots[class_scope.scope_id] = slot
            self._class_cell_slot_mask |= 1 << slot
            self.callback_slot_mask |= 1 << slot
            self.states.set_taint_domain(self._callback_taint_domain())
        return slot

    def _native_class_namespace_slot(self, scope: _Scope) -> int:
        slot = self._class_namespace_slots.get(scope.scope_id)
        if slot is None:
            slot = len(self.slot_names)
            self.slot_names.append(f"{scope.scope_id}:native namespace")
            self.slot_owner_scope_ids.append(scope.scope_id)
            self._class_namespace_slots[scope.scope_id] = slot
            self.callback_slot_mask |= 1 << slot
            self.states.set_taint_domain(self._callback_taint_domain())
        return slot

    def _namespace_can_call(
        self, state_id: int, scope: _Scope, name: str, *, write: bool = False
    ) -> bool:
        owner = scope.namespace_lookup_owner(name, write=write)
        slot = self._class_namespace_slots.get(owner.scope_id) if owner else None
        # The marker is reset in the same state as the new native dictionary.
        # Existing state joins/taint domains retain every path after its birth;
        # a shared source scope carries no mutable last-creation epoch.
        tainted = (
            not self.states._binding_resolution(state_id, slot).clean
            if slot is not None
            else self.states.get(state_id).taint_epoch != 0
        )
        return scope.namespace_can_call(name, write=write, namespace_tainted=tainted)

    def _activation_group_is_opaque(self, sources: frozenset[PythonSourceKey]) -> bool:
        opaque = self._opaque_activation_sources
        assert opaque is not None
        answer = self._opaque_activation_reads.get(sources)
        if answer is None:
            answer = not sources.isdisjoint(opaque)
            self._opaque_activation_reads[sources] = answer
        if self._recorded_activation_groups is not None:
            self._recorded_activation_groups.setdefault(sources)
        return answer

    def _lexical_activation_is_opaque(self, scope: _Scope) -> bool:
        cached = self._opaque_lexical_activations.get(scope.scope_id)
        if cached is None:
            sources: set[PythonSourceKey] = set()
            owner: _Scope | None = scope
            while owner is not None and owner.kind != "module":
                sources.add(owner.source)
                owner = owner.parent
            group = frozenset(sources)
            answer = self._activation_group_is_opaque(group)
            self._opaque_lexical_activations[scope.scope_id] = (group, answer)
        else:
            group, answer = cached
            if self._recorded_activation_groups is not None:
                self._recorded_activation_groups.setdefault(group)
        return answer

    def _name_uses_activation_namespace(
        self, state_id: int, scope: _Scope, name: str
    ) -> bool:
        if scope.activation_namespace_stable:
            return False
        slot = self._slot_for_name(scope, name)
        if slot is None or self.module_slot_mask & (1 << slot):
            if self._callback_rebinds_module_name(name):
                return True
            # Initialization histories already join the current module's
            # values. Another globals mapping needs both a possible code
            # rebinder and an exposed definition or enclosing activation.
            return self.policy.code_rebinding and (
                self._opaque_activation_sources is None
                or self._lexical_activation_is_opaque(scope)
            )
        owner = scope.namespace_lookup_owner(name)
        return owner is not None and bool(
            self.states._binding_resolution(state_id, slot).identities
            & UNBOUND_IDENTITY
        )

    def _record_decoration_annotation_heads(
        self,
        node: ast.ClassDef,
        state_id: int,
        scope: _Scope,
        class_scope: _Scope,
    ) -> None:
        """Each top-level annotation head's value when the decorators run.

        A decorator such as ``@dataclass`` reads the annotations then. A
        deferred annotation (3.14+) evaluates in an annotation scope that sees
        the class namespace, then globals; ``dataclasses._is_type`` resolves a
        ``from __future__`` string's head in the module namespace. An eager
        annotation already carries the fact of its own evaluation.
        """
        deferred = not self._future_annotations and self.policy.target_python >= (3, 14)
        if deferred:
            lookup = class_scope
        elif self._future_annotations and scope.kind == "module":
            lookup = scope
        else:
            return
        for item in node.body:
            if not isinstance(item, ast.AnnAssign) or not isinstance(
                item.target, ast.Name
            ):
                continue
            annotation = item.annotation
            head = (
                annotation.value
                if isinstance(annotation, ast.Subscript)
                else annotation
            )
            if isinstance(head, ast.Name):
                identities = self._resolve_name(state_id, lookup, head.id)[0]
            elif isinstance(head, ast.Attribute) and isinstance(head.value, ast.Name):
                base = self._resolve_name(state_id, lookup, head.value.id)[0]
                identities = self._member_value(state_id, base, head.attr)
            else:
                continue
            key = self._node_key(head)
            self.transfer.decoration_annotation_heads[key] = (
                self.transfer.decoration_annotation_heads.get(key, NO_IDENTITIES)
                | identities
            )

    def _plain_class_value(self, node: ast.expr) -> PythonSourceKey | None:
        """Resolve a bound local class declaration whose metaclass is type."""
        fact = self.transfer.expressions.get(self._node_key(node))
        if (
            fact is None
            or fact.binding_invalidated
            or fact.star_export_lookup
            or fact.name_lookup not in {"global", "lexical"}
        ):
            return None
        result = fact.result
        if result.identities & ~(
            possible_identity(PythonIdentity.USER_CLASS) | UNBOUND_IDENTITY
        ) or not result.identities & int(PythonIdentity.USER_CLASS):
            return None
        if len(result.source_origins) != 1:
            return None
        owner = next(iter(result.source_origins))
        cls = self.transfer.source_classes.get(owner)
        return owner if cls is not None and cls.plain_metaclass else None

    def _read_name_resolution(
        self,
        state_id: int,
        scope: _Scope,
        name: str,
        storage: _BindingResolution | None,
    ) -> _BindingResolution:
        unproven_result = (
            expression_result_without_value_facts(storage.result)
            if storage is not None
            else UNKNOWN_EXPRESSION_RESULT
        )
        if self._namespace_can_call(state_id, scope, name):
            # __prepare__ may return an arbitrary mapping, and an initially
            # exact dictionary can gain callback-bearing keys via reflection.
            # A prior STORE_NAME cannot establish the next lookup's behavior.
            return _BindingResolution(
                OTHER_IDENTITY | UNBOUND_IDENTITY,
                None,
                unproven_result,
                False,
                present=storage is not None and storage.present,
            )
        own = storage.public() if storage is not None else _UNBOUND_BINDING_RESOLUTION
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
            return _BindingResolution(
                OTHER_IDENTITY | UNBOUND_IDENTITY,
                None,
                unproven_result,
                False,
                present=storage is not None and storage.present,
            )
        if (
            scope.kind == "class"
            and name in scope.locals
            and own.identities & UNBOUND_IDENTITY
        ):
            # LOAD_NAME: an unbound class-local falls back to the module, not
            # the enclosing class or an identically named closure cell.
            global_slot = self._module_slot(name)
            fallback = (
                self.states._binding_resolution(state_id, global_slot).public()
                if global_slot is not None
                else _UNBOUND_BINDING_RESOLUTION
            )
            if own.identities == UNBOUND_IDENTITY:
                return fallback
            static_value, result = _join_binding_payloads(
                (
                    (own.identities, own.static_value, own.result),
                    (
                        fallback.identities,
                        fallback.static_value,
                        fallback.result,
                    ),
                )
            )
            return _BindingResolution(
                (own.identities & ~UNBOUND_IDENTITY) | fallback.identities,
                static_value,
                result,
                own.clean and fallback.clean,
                present=own.present or fallback.present,
            )
        return own

    def _resolve_name(
        self, state_id: int, scope: _Scope, name: str
    ) -> tuple[
        IdentityMask, PythonStaticValue, StaticExpressionResult, bool, bool, int
    ]:
        """One source-point lookup owns value, cleanliness and storage custody."""
        slot = self._slot_for_name(scope, name)
        storage = (
            self.states._binding_resolution(state_id, slot)
            if slot is not None
            else None
        )
        activation_lookup = self._name_uses_activation_namespace(state_id, scope, name)
        self.states.binding_lookups += 1
        resolution = self._read_name_resolution(state_id, scope, name, storage)
        value = resolution.identities
        static_value = resolution.static_value
        result = resolution.result
        lexical_cell = slot is not None and self._slot_uses_lexical_storage(slot)
        if value & UNBOUND_IDENTITY and not lexical_cell:
            declaration = BUILTIN_IDENTITIES.get(name)
            fallback = (
                self._guarded_member_identity(
                    state_id,
                    int(PythonIdentity.BUILTINS_MODULE),
                    PythonIdentity.BUILTINS_MODULE,
                    *declaration,
                )
                if declaration is not None
                else OTHER_IDENTITY | UNBOUND_IDENTITY
            )
            if self._builtins_mapping_rebindable():
                # A frame's builtins come from globals["__builtins__"] when the
                # function or module frame is created (CPython
                # _PyEval_BuiltinsFromGlobals). A writer of that key selects
                # another mapping, so the canonical member proves nothing.
                fallback |= OTHER_IDENTITY
            static_value, result = _join_binding_payloads(
                (
                    (value, static_value, result),
                    (fallback, None, UNKNOWN_EXPRESSION_RESULT),
                )
            )
            value = (value & ~UNBOUND_IDENTITY) | fallback
        callback_exposed = (
            self._callback_module_names is None
            if slot is None
            else self.states.slot_in_taint_domain(slot)
        )
        invalidated = (
            callback_exposed
            and self.states.get(state_id).taint_epoch != 0
            and not resolution.clean
        )
        if self._callback_module_names is None and (
            invalidated
            or activation_lookup
            or self._namespace_can_call(state_id, scope, name)
        ):
            # Source availability is distinct from exact activation identity.
            # An unverified callback may install any module-owned callable.
            # Closed source writers publish their actual value origins at the
            # write/argument boundary instead of inventing a whole namespace.
            # Storage taint alone grants no callable source availability.
            # An opaque FunctionType activation alone proves no such writer.
            result = replace(
                result,
                deferred=result.deferred
                | {DeferredExecution(self._module_source, "escape")},
            )
        if activation_lookup:
            value |= OTHER_IDENTITY
            invalidated = True
        bound = lexical_cell or resolution.identities != UNBOUND_IDENTITY
        return (
            value,
            static_value if resolution.clean and not activation_lookup else None,
            result
            if resolution.clean and not activation_lookup
            else expression_result_without_value_facts(result),
            invalidated,
            bound,
            storage.owner_token
            if storage is not None and storage.clean and not invalidated
            else 0,
        )

    def _record_state(self, state_id: int) -> None:
        # Record a transfer when it happens, never when a completion partition
        # later republishes its summary. Replaying old exceptional states after
        # a definition would fabricate impossible future closure environments.
        for observed in self._observed_stack:
            observed.states.append(state_id)
        if self._active_module_states is None:
            self._module_history.append(state_id)

    def _record_operation_execution(
        self,
        scope: _Scope,
        refs: frozenset[DeferredExecution],
        *,
        module_metadata_effects: EffectMask = NO_EFFECTS,
    ) -> None:
        scope.deferred_execution |= refs
        scope.module_metadata_effects |= module_metadata_effects
        if self._observed_stack:
            self._observed_stack[-1].deferred_execution |= refs
            self._observed_stack[-1].module_metadata_effects |= module_metadata_effects

    def _can_select_builtin_environment(self, operand: ast.expr) -> bool:
        fact = self.transfer.expressions.get(self._node_key(operand))
        name = None if fact is None else fact.static_value
        fields = {"__builtins__", "f_builtins"}
        if isinstance(name, str):
            return name in fields
        if isinstance(name, PythonStringAlternatives):
            return not fields.isdisjoint(name.values)
        if isinstance(name, PythonStringBound):
            return any(name.admits(field) for field in fields)
        return True

    def _project_mapping_write(
        self,
        scope: _Scope,
        operation: ast.expr,
        origins: frozenset[PythonSourceKey],
        key: PythonStaticValue,
    ) -> None:
        if not origins:
            return
        write = python_source_projection_key(
            operation, PythonSourceProjection.MAPPING_WRITE
        )
        prior = self.transfer.source_elements.get(write)
        self.transfer.source_elements[write] = PythonSourceElement(
            write,
            PythonElementProtocol.MAPPING_WRITE,
            origins | (prior.origins if prior else frozenset()),
            key
            if isinstance(key, str) and (prior is None or prior.key == key)
            else None,
        )
        self._record_operation_execution(
            scope, frozenset({DeferredExecution(write, "mutate")})
        )

    def _publish_source_attribute(
        self, site: PythonSourceKey, name: str, owners: frozenset[PythonSourceKey]
    ) -> None:
        selections = self.transfer.source_attributes.setdefault(site, {})
        selections[name] = selections.get(name, frozenset()) | owners

    def _source_call_argument(
        self,
        node: ast.expr,
        selector: int | str | None,
        result: StaticExpressionResult,
    ) -> PythonSourceArgument:
        direct = result.source_origins
        exposed = result.exposed_source_origins
        if selector is not None and current_globals_dict_is_exact(
            result.identities, result
        ):
            # Transport the capability to a reached formal. The ordinary
            # expression transfer still accounts for evaluation and escape.
            source = python_source_projection_key(
                node, PythonSourceProjection.ARGUMENT_REFLECTED_GLOBALS
            )
            self.transfer.source_elements[source] = PythonSourceElement(
                source,
                PythonElementProtocol.REFLECTED_GLOBALS,
                frozenset({self._module_source}),
            )
            direct |= {source}
            exposed |= {source}
        return selector, direct, exposed

    def _publish_source_call(self, call: PythonSourceCall) -> None:
        previous = self.transfer.source_calls.get(call.owner)
        self.transfer.source_calls[call.owner] = (
            call if previous is None else call.join(previous)
        )

    def _apply_source_decorator(
        self,
        expression: ast.expr,
        decorator: StaticExpressionResult,
        subject: StaticExpressionResult,
        scope: _Scope,
    ) -> StaticExpressionResult:
        if not decorator.source_origins:
            return subject
        site = python_source_projection_key(
            expression, PythonSourceProjection.DECORATOR_APPLY
        )
        self._publish_source_call(
            PythonSourceCall(
                site,
                decorator.source_origins,
                NO_IDENTITIES,
                ((0, subject.source_origins, subject.exposed_source_origins),),
                (None,),
            )
        )
        # An unused result still executes the decorator with its actual.
        self._record_operation_execution(
            scope, frozenset({DeferredExecution(site, "read")})
        )
        return replace(subject, source_origins=subject.source_origins | {site})

    def _deferred_function_result(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda
    ) -> StaticExpressionResult:
        declaration = self._declare_function(node)
        return StaticExpressionResult(
            identities=int(PythonIdentity.USER_FUNCTION),
            source_origins=frozenset({declaration.owner}),
            deferred=frozenset(
                {
                    DeferredExecution(
                        declaration.owner,
                        "call" if declaration.kind is FunctionKind.SYNC else "factory",
                    )
                }
            ),
            deferred_complete=True,
            attribute_hooks=frozenset(),
        )

    def _fresh_function_wrapper_result(
        self,
        constructor: StaticExpressionResult,
        function: StaticExpressionResult,
        node: ast.FunctionDef | ast.AsyncFunctionDef,
        expression: ast.expr,
        scope: _Scope,
        *,
        defaults: tuple[tuple[str, StaticExpressionResult], ...],
    ) -> StaticExpressionResult | None:
        # `property(fget)` retains `fget` as the namespace would retain the
        # plain function, annotations included; it runs no Python code when
        # constructed (HF-847). The other wrappers keep their capture rule.
        capture = (
            (
                PythonIdentity.RETAINED_CALLABLE_WRAPPER,
                PythonElementProtocol.PROPERTY.value,
            )
            if constructor.identities == int(PythonIdentity.BUILTIN_PROPERTY)
            else BUILTIN_RETURN_CAPTURES.get(constructor.identities)
        )
        if (
            capture is None
            or function.identities != int(PythonIdentity.USER_FUNCTION)
            or not function.deferred_complete
            or any(
                not result.value_known or literal_identity_key(result.value) is None
                for _parameter, result in defaults
            )
            or (
                capture[1] != PythonElementProtocol.PROPERTY
                and function_annotation_expressions(node)
            )
        ):
            return None
        assert self._dependency_authority is not None
        # The evaluated defaults have exact immutable literal identity, so
        # retaining/releasing them cannot execute Python. Other retained cells
        # need their independent owner proof. The function is newly created at
        # this application point; older callable values stay generic.
        for name in self._dependency_authority.summary(node).body.lexical:
            slot = self._enclosing_slot(scope, name, annotation_owner=None)
            if slot is not None and slot not in self.module_slots:
                return None
        owner = self._node_key(expression)
        protocol = PythonElementProtocol(capture[1])
        previous = self.transfer.source_elements.get(owner)
        origins = function.source_origins | (
            previous.origins if previous else frozenset()
        )
        self.transfer.source_elements[owner] = PythonSourceElement(
            owner, protocol, origins
        )
        return StaticExpressionResult(
            identities=int(capture[0]),
            source_origins=frozenset({owner}),
            deferred_complete=True,
            evaluation_required=True,
        )

    def _source_element_result(
        self,
        source: PythonSourceKey,
        owner: StaticExpressionResult,
        selected: StaticExpressionResult,
        *,
        protocol: PythonElementProtocol,
        key: str | None = None,
    ) -> StaticExpressionResult:
        # Exact tuple/list projections already carry their selected element.
        # Unknown source-call values need a typed contents projection rather
        # than inheriting the container/callee identity as the selected value.
        if (
            owner.kind != "unknown"
            or not owner.source_origins
            or (
                selected.source_origins
                and not selected.source_origins & owner.source_origins
            )
        ):
            return selected
        previous = self.transfer.source_elements.get(source)
        self.transfer.source_elements[source] = PythonSourceElement(
            source,
            protocol,
            owner.source_origins
            | (previous.origins if previous is not None else frozenset()),
            key if previous is None or previous.key == key else None,
        )
        return replace(selected, source_origins=frozenset({source}))

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
        result_override: StaticExpressionResult | None = None,
        module_metadata: PythonModuleMetadataProof = NO_MODULE_METADATA_PROOF,
        deferred_execution: frozenset[DeferredExecution] = frozenset(),
        module_metadata_effects: EffectMask = NO_EFFECTS,
    ) -> StaticExpressionResult:
        key = self._node_key(node)
        result = (
            result_override
            if result_override is not None
            else static_expression_result(
                node, fact_result=self._known_expression_result
            )
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
            elif result_override is None:
                result = UNKNOWN_EXPRESSION_RESULT
        kind = result.kind
        if scope.activation_namespace_stable and identities == int(
            PythonIdentity.CURRENT_GLOBALS
        ):
            # Eager scopes inherit the module's exact globals dictionary.
            # Deferred activations can receive a FunctionType dict subclass.
            kind = "dict"
        origins = result.source_origins
        if isinstance(node, ast.Attribute) and origins:
            receiver_origins = self._known_expression_result(
                node.value
            ).exposed_source_origins
            self._publish_source_attribute(key, node.attr, receiver_origins)
        if callable_identities := identities & SOURCE_CALLABLE_IDENTITIES:
            # Positive provenance alone cannot rule out OTHER.
            self.transfer.source_callables[key] = (
                self.transfer.source_callables.get(key, NO_IDENTITIES)
                | callable_identities
            )
            origins |= {key}
        name_lookup: PythonNameLookup = "none"
        star_export_lookup = False
        if isinstance(node, ast.Name):
            slot = self._slot_for_name(scope, node.id)
            if (owner := scope.namespace_lookup_owner(node.id)) is not None:
                if scope.annotation_evaluator and not owner.needs_annotation_namespace:
                    self._record_deferred_scope_write(owner)
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
                if (
                    isinstance(node.ctx, ast.Load)
                    and not scope.activation_namespace_stable
                    and scope.kind in {"function", "lambda", "comprehension"}
                    and node.id in BUILTIN_BINDING_GUARD_BITS
                ):
                    self.transfer._builtin_guard_reads[key] = node.id
            if (
                self._star_import_origins
                and isinstance(node.ctx, ast.Load)
                and name_lookup in {"global", "class_global"}
            ):
                # The read may find a star-bound name: it selects that name
                # from the star-imported modules, as an attribute read would.
                star_export_lookup = True
                origins |= {key}
                self._publish_source_attribute(key, node.id, self._star_import_origins)
        # A rooted value can retire without calling Python. This fact, lookup
        # identity and provenance all travel on the source expression result.
        # Seal them together so a copy normalizes and hashes the result once.
        release_may_call = result.release_may_call
        publication_release_stable = result._publication_release_stable
        if identities and not _identity_can_release(identities):
            release_may_call = False
            publication_release_stable = True
        if (
            kind != result.kind
            or identities != result.identities
            or origins != result.source_origins
            or release_may_call != result.release_may_call
            or publication_release_stable != result._publication_release_stable
        ):
            result = replace(
                result,
                kind=kind,
                identities=identities,
                source_origins=origins,
                release_may_call=release_may_call,
                _publication_release_stable=publication_release_stable,
            )
        previous = self.transfer.expressions.get(key)
        self.transfer.expressions[key] = PythonExpressionFact(
            key,
            scope.scope_id,
            effects | (previous.effects if previous is not None else NO_EFFECTS),
            static_value
            if previous is None
            else join_python_static_values(previous.static_value, static_value),
            binding_invalidated
            or (previous is not None and previous.binding_invalidated),
            binding_is_bound and (previous is None or previous.binding_is_bound),
            module_namespace_observable
            or (previous is not None and previous.module_namespace_observable),
            result
            if previous is None
            else join_static_expression_results((previous.result, result)),
            previous.truth_effects if previous is not None else NO_EFFECTS,
            name_lookup,
            previous.binding_capture_required if previous is not None else False,
            module_metadata
            if previous is None
            else module_metadata.intersect(previous.module_metadata),
            deferred_execution
            | (previous.deferred_execution if previous is not None else frozenset()),
            module_metadata_effects
            | (
                previous.module_metadata_effects if previous is not None else NO_EFFECTS
            ),
            star_export_lookup=star_export_lookup
            or (previous is not None and previous.star_export_lookup),
        )
        scope.deferred_execution |= deferred_execution
        scope.module_metadata_effects |= module_metadata_effects
        return self.transfer.expressions[key].result

    def _known_expression_result(self, node: ast.expr) -> StaticExpressionResult:
        fact = self.transfer.expressions.get(self._node_key(node))
        return UNKNOWN_EXPRESSION_RESULT if fact is None else fact.result

    def _binding_held(
        self, node: ast.expr, read: Callable[[ast.expr], bool] = _binding_read
    ) -> bool:
        """Whether every reference ``node`` yields stays held by a binding (or
        an owner it reads) until the caller releases the temporary, given that
        no Python runs in between (the caller proves it): a ``read`` the
        caller admits (``_binding_read`` by default), or ``type(x)`` of one
        with the exact builtin ``type``, which runs no Python. The class ``type(x)`` answers is held by ``x``'s own class
        edge while ``x`` stays bound, so releasing that temporary cannot drop
        the class's last reference and runs no code, whatever its metaclass
        (HF-971). The proof is the temporary's, never the result's: a stored
        ``type(x)`` can outlive ``x``."""
        if read(node):
            return True
        if (
            not isinstance(node, ast.Call)
            or node.keywords
            or len(node.args) != 1
            or not read(node.args[0])
        ):
            return False
        callee = self.transfer.expressions.get(self._node_key(node.func))
        return (
            callee is not None
            and not callee.binding_invalidated
            and callee.identities == int(PythonIdentity.BUILTIN_TYPE)
        )

    def _widen_exposed_bindings(self, state_id: int) -> int:
        self._namespace_observation_epoch += 1
        return self.states.taint_exposed_bindings(state_id)

    def _apply_effects(
        self,
        state_id: int,
        effects: EffectMask,
        *,
        object_write_may_invalidate_members: bool = True,
        object_write_content_exempt_slots: frozenset[int] = frozenset(),
    ) -> int:
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
            WRITES_FRAME_STATE | EXECUTES_ARBITRARY_PYTHON | callback_effects
        ):
            observed = self._initializer_premise_reads
            if observed is not None and not observed.frame_borrows:
                observed.frame_borrows = any(self._binding_read_scopes)
            if frame_locals_exposed(
                self.policy.target_python, self.policy.frame_locals_writer
            ):
                # PEP 667 permits a callback to replace a live optimized binding.
                # Truth, descriptor and iterator callbacks obey the same boundary.
                self._capture_pending_binding_reads()
        if effects & (
            WRITES_GLOBAL_NAMESPACE
            | WRITES_FRAME_STATE
            | EXECUTES_ARBITRARY_PYTHON
            | callback_effects
        ):
            state_id = self._widen_exposed_bindings(state_id)
        callback_boundary = bool(
            effects & (EXECUTES_ARBITRARY_PYTHON | callback_effects)
        )
        if effects & WRITES_OBJECT_STATE or callback_boundary:
            state_id = self.states.invalidate_mutable_contents(
                state_id,
                except_slots=(
                    frozenset()
                    if callback_boundary
                    else object_write_content_exempt_slots
                ),
            )
        if (
            effects
            & (EXECUTES_ARBITRARY_PYTHON | INVOKES_IMPORT_SYSTEM | callback_effects)
            or effects & WRITES_OBJECT_STATE
            and object_write_may_invalidate_members
        ):
            state_id = self.states.invalidate_members(
                state_id, self.policy.callback_member_writes
            )
        return state_id

    def _guarded_member_identity(
        self,
        state_id: int,
        base: IdentityMask,
        owner: PythonIdentity,
        identity: PythonIdentity,
        guard: PythonMember,
    ) -> IdentityMask:
        state = self.states.get(state_id)
        if state.definitely_invalidated_members & int(guard):
            return OTHER_IDENTITY
        if base == int(owner) and not state.maybe_invalidated_members & int(guard):
            return int(identity)
        return possible_identity(identity)

    def _member_value(
        self,
        state_id: int,
        base: IdentityMask,
        member: str,
        owner_result: StaticExpressionResult = UNKNOWN_EXPRESSION_RESULT,
    ) -> IdentityMask:
        value = NO_IDENTITIES
        for owner, identity, guard in _canonical_member_declarations(base, member):
            member_identity = self._guarded_member_identity(
                state_id, base, owner, identity, guard
            )
            if self.policy.noncanonical_imports and any(
                provider_identity == identity
                and not self._import_provider_is_canonical(provider)
                for provider, provider_identity in CANONICAL_IMPORT_IDENTITIES.items()
            ):
                # A redirected submodule attribute may be the provider or
                # whatever replaced it.
                member_identity |= OTHER_IDENTITY
            value |= member_identity
        if member in {"f_globals", "__globals__"}:
            owner = (
                PythonIdentity.CURRENT_FRAME
                if member == "f_globals"
                else PythonIdentity.USER_FUNCTION
            )
            if base & int(owner):
                value |= int(PythonIdentity.CURRENT_GLOBALS)
                if base != int(owner):
                    value |= OTHER_IDENTITY
        elif member in {"__setattr__", "__delattr__"} and base & int(
            PythonIdentity.BUILTIN_TYPE
        ):
            # The builtin metaclass's descriptors cannot be reassigned.
            value |= int(
                PythonIdentity.TYPE_SETATTR
                if member == "__setattr__"
                else PythonIdentity.TYPE_DELATTR
            )
            if base != int(PythonIdentity.BUILTIN_TYPE):
                value |= OTHER_IDENTITY
        elif member in {"__setitem__", "__delitem__"}:
            if base & int(PythonIdentity.CURRENT_GLOBALS):
                value |= int(
                    PythonIdentity.GLOBALS_SETITEM
                    if member == "__setitem__"
                    else PythonIdentity.GLOBALS_DELITEM
                )
                if not current_globals_dict_is_exact(base, owner_result):
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
        return self.states.invalidate_members(
            state_id, _member_write_mask(base, member), definite=True
        )

    def _current_globals_item(
        self,
        state_id: int,
        key: PythonStaticValue,
        default: StaticExpressionResult | None = None,
    ) -> StaticExpressionResult | None:
        """Select namespace contents through the existing source-point storage.

        A bounded namespace has no escaping mapping or computed-key writer.
        Unknown loader-supplied values remain unknown; absence is not invented.
        An actual stored self-reference retains its mapping provenance.
        """
        if isinstance(key, PythonStringAlternatives):
            selections = tuple(
                self._current_globals_item(state_id, name, default)
                for name in sorted(key.values)
            )
            if not selections or any(value is None for value in selections):
                return None
            return join_static_expression_results(
                tuple(value for value in selections if value is not None)
            )
        if self._callback_module_names is None or type(key) is not str:
            return None
        slot = self._module_slot(key)
        resolution = (
            self.states._binding_resolution(state_id, slot).public()
            if slot is not None
            else _UNBOUND_BINDING_RESOLUTION
        )
        if (
            self.states.get(state_id).namespace_values_unknown
            or self._callback_rebinds_module_name(key)
        ) and (not resolution.present or not resolution.clean):
            return None
        result = (
            replace(resolution.result, identities=resolution.identities)
            if resolution.present
            else UNKNOWN_EXPRESSION_RESULT
        )
        if default is not None:
            if not resolution.present or resolution.identities == UNBOUND_IDENTITY:
                result = default
            elif resolution.identities & UNBOUND_IDENTITY:
                result = join_static_expression_results(
                    (
                        replace(
                            result, identities=result.identities & ~UNBOUND_IDENTITY
                        ),
                        default,
                    )
                )
        return expression_result_for_publication(result)

    def _current_globals_method(
        self,
        state_id: int,
        scope: _Scope,
        node: ast.Call,
        receiver: StaticExpressionResult,
        arguments: tuple[StaticExpressionResult, ...],
        prior_effects: EffectMask,
    ) -> _CallSemantics | None:
        """Native namespace selection/mutation shares ordinary slot transfer."""
        assert isinstance(node.func, ast.Attribute)
        if (
            not current_globals_dict_is_exact(receiver.identities, receiver)
            or any(isinstance(argument, ast.Starred) for argument in node.args)
            or len(arguments) != len(node.args) + len(node.keywords)
        ):
            return None
        method = node.func.attr
        positional = arguments[: len(node.args)]
        inert_none = StaticExpressionResult.scalar(None, evaluation_required=True)
        result = inert_none
        effects = READS_OBJECT_STATE | RAISES

        def callbacks() -> None:
            nonlocal state_id, effects
            if self.states.get(state_id).namespace_values_unknown:
                boundary = (
                    EXECUTES_ARBITRARY_PYTHON | INVOKES_COMPARISON_CALLBACK | RAISES
                )
                state_id = self._apply_effects(state_id, boundary)
                effects |= boundary

        keyed = method in {"get", "__getitem__", "pop", "setdefault"}
        if keyed:
            maximum = 1 if method == "__getitem__" else 2
            if node.keywords or not 1 <= len(positional) <= maximum:
                return None
            callbacks()
            default = (
                positional[1]
                if len(positional) == 2
                else None
                if method in {"__getitem__", "pop"}
                else inert_none
            )
            key_fact = self.transfer.expressions.get(self._node_key(node.args[0]))
            key = None if key_fact is None else key_fact.static_value
            selected = self._current_globals_item(state_id, key, default)
            if selected is None:
                if method in {"get", "__getitem__"}:
                    return None
                selected = join_static_expression_results(
                    (
                        replace(
                            UNKNOWN_EXPRESSION_RESULT,
                            exposes_module_globals=receiver.exposes_module_globals,
                        ),
                        *((default,) if default is not None else ()),
                    )
                )
            result = selected
            if method in {"pop", "setdefault"}:
                replacement = UNKNOWN_EXPRESSION_RESULT if method == "pop" else default
                assert replacement is not None
                state_id, written = self._store_namespace_key(
                    state_id,
                    key,
                    UNBOUND_IDENTITY if method == "pop" else replacement.identities,
                    None,
                    replacement,
                    scope=scope,
                    only_if_absent=method == "setdefault",
                )
                effects |= written
        elif method in {"update", "__ior__", "__init__"}:
            if len(positional) > 1 or (
                method == "__ior__" and (len(positional) != 1 or node.keywords)
            ):
                return None
            callbacks()

            def publish(key: PythonStaticValue, value: StaticExpressionResult) -> None:
                nonlocal state_id, effects
                state_id, written = self._store_namespace_key(
                    state_id, key, value.identities, None, value, scope=scope
                )
                effects |= written

            def publish_mapping(mapping: ast.expr) -> None:
                if isinstance(mapping, ast.Dict):
                    for key, value in zip(mapping.keys, mapping.values, strict=True):
                        if key is None:
                            publish_mapping(value)
                        else:
                            fact = self._known_expression_result(key)
                            publish(
                                fact.value
                                if fact.value_known and type(fact.value) is str
                                else None,
                                self._known_expression_result(value),
                            )
                else:
                    publish(None, UNKNOWN_EXPRESSION_RESULT)

            for mapping in node.args:
                if prior_effects & (
                    _CALLEE_RETENTION_FORBIDDEN_EFFECTS | WRITES_OBJECT_STATE
                ):
                    publish(None, UNKNOWN_EXPRESSION_RESULT)
                else:
                    publish_mapping(mapping)
            for index, keyword in enumerate(node.keywords, start=len(node.args)):
                if keyword.arg is None:
                    if prior_effects & (
                        _CALLEE_RETENTION_FORBIDDEN_EFFECTS | WRITES_OBJECT_STATE
                    ):
                        publish(None, UNKNOWN_EXPRESSION_RESULT)
                    else:
                        publish_mapping(keyword.value)
                else:
                    publish(keyword.arg, arguments[index])
            if method == "__ior__":
                result = receiver
        elif builtin_method_writes_receiver("dict", method) and method in {
            "clear",
            "popitem",
        }:
            if positional or node.keywords:
                return None
            state_id, written = self._store_namespace_key(
                state_id, None, OTHER_IDENTITY, None, scope=scope
            )
            effects |= written
            if method == "popitem":
                result = replace(
                    UNKNOWN_EXPRESSION_RESULT,
                    exposes_module_globals=receiver.exposes_module_globals,
                )
        else:
            return None
        key = self._node_key(node)
        self.transfer.assignment_effects[key] = (
            self.transfer.assignment_effects.get(key, NO_EFFECTS) | effects
        )
        return _CallSemantics(
            result.identities,
            effects,
            state_id,
            result,
            True,
            effects,
            callee_release_safe=True,
        )

    def _call_semantics(
        self,
        state_id: int,
        callee_state_id: int,
        scope: _Scope,
        node: ast.Call,
        callee: IdentityMask,
        callee_value: StaticExpressionResult,
        argument_results: tuple[StaticExpressionResult, ...],
        captured_receiver_owner_token: int,
        argument_owner_token_floor: int,
        prior_effects: EffectMask,
        intrinsic_result: IdentityMask,
    ) -> _CallSemantics:
        effects = NO_EFFECTS
        result = OTHER_IDENTITY
        exact = callee.bit_count() == 1
        builtin_name = next(
            (
                name
                for name, identity in BUILTIN_SHAPE_IDENTITIES.items()
                if callee == int(identity)
            ),
            None,
        )
        result_fact = UNKNOWN_EXPRESSION_RESULT
        specialization_valid = False
        retained_argument_indices: frozenset[int] = frozenset()
        argument_protocol_indices: frozenset[int] | None = None
        argument_escape_indices: frozenset[int] = frozenset()
        selected_deferred: frozenset[DeferredExecution] = frozenset()
        source_result_projected = False
        borrowed_module_operand: int | None = None
        if (
            callee
            in {
                int(PythonIdentity.BUILTIN_ID),
                int(PythonIdentity.BUILTIN_CALLABLE),
                int(PythonIdentity.BUILTIN_TYPE),
            }
            and len(node.args) == 1
            and not node.keywords
            and not isinstance(node.args[0], ast.Starred)
        ):
            # These one-argument forms inspect the object, without returning it
            # or passing it to Python. Any id audit hook receives the integer.
            borrowed_module_operand = 0
        method_shape = None
        receiver_node: ast.expr | None = None
        receiver_slot: int | None = None
        receiver_write_exempt_slots: frozenset[int] = frozenset()
        if isinstance(node.func, ast.Attribute):
            receiver_node = node.func.value
            receiver_result = self._known_expression_result(receiver_node)
            if (
                receiver_result.identities == int(PythonIdentity.BUILTIN_DICT)
                and node.func.attr == "get"
                and not node.keywords
                and len(node.args) in {2, 3}
                and not any(isinstance(arg, ast.Starred) for arg in node.args)
            ):
                # The evaluated immutable builtin descriptor bypasses subclass
                # overrides. On a normal return its explicit receiver is a dict.
                owner = argument_results[0]
                normalized = ast.Call(func=node.func, args=node.args[1:], keywords=[])
                shape = builtin_method_call_shape(
                    replace(owner, kind="dict"), "get", normalized, argument_results[1:]
                )
                assert shape is not None
                selected = python_source_projection_key(
                    node, PythonSourceProjection.CALL_MAPPING_VALUE
                )
                key = (
                    argument_results[1].value
                    if argument_results[1].value_known
                    and type(argument_results[1].value) is str
                    else None
                )
                prior = self.transfer.source_elements.get(selected)
                self.transfer.source_elements[selected] = PythonSourceElement(
                    selected,
                    PythonElementProtocol.SUBSCRIPT,
                    owner.source_origins | (prior.origins if prior else frozenset()),
                    key if prior is None or prior.key == key else None,
                )
                value = replace(
                    shape.result,
                    source_origins=shape.result.source_origins | {selected},
                )
                if len(argument_results) == 3:
                    value = join_static_expression_results((value, argument_results[2]))
                return _CallSemantics(
                    value.identities,
                    shape.invocation_effects,
                    self._apply_effects(state_id, shape.invocation_effects),
                    value,
                    shape.specialization_valid,
                    shape.argument_effects,
                    argument_protocol_indices=frozenset({0, 1}),
                    source_result_projected=True,
                )
            if (
                receiver_result.identities == int(PythonIdentity.BUILTIN_DICT)
                and builtin_mapping_write_scope(node.func.attr)
                is MappingWriteScope.UPDATE
                and node.args
                and not isinstance(node.args[0], ast.Starred)
            ):
                # The immutable descriptor receives the dictionary as an
                # explicit operand without exporting it. Source call transfer
                # still models the actual receiver's writes and releases.
                return _CallSemantics(
                    OTHER_IDENTITY,
                    UNKNOWN_EFFECTS,
                    self._apply_effects(state_id, UNKNOWN_EFFECTS),
                    UNKNOWN_EXPRESSION_RESULT,
                    False,
                    UNKNOWN_EFFECTS,
                    argument_protocol_indices=frozenset(),
                    argument_escape_indices=frozenset(range(1, len(argument_results))),
                    borrowed_module_operand=0,
                )
            receiver_boundary = (
                WRITES_OBJECT_STATE
                | EXECUTES_ARBITRARY_PYTHON
                | INVOKES_DESCRIPTOR
                | INVOKES_ITERATION_CALLBACK
                | INVOKES_CONTEXT_CALLBACK
                | INVOKES_COMPARISON_CALLBACK
                | RUNS_FINALIZER
                | RUNS_WEAKREF_CALLBACK
            )
            receiver_rebound = False
            if isinstance(receiver_node, ast.Name):
                candidate_slot = self._slot_for_name(scope, receiver_node.id)
                current_receiver = (
                    self.states._binding_resolution(state_id, candidate_slot).public()
                    if candidate_slot is not None
                    else _UNBOUND_BINDING_RESOLUTION
                )
                receiver_rebound = candidate_slot is not None and (
                    self.states.slot_updated_between(
                        callee_state_id, state_id, candidate_slot
                    )
                )
                if (
                    candidate_slot is not None
                    and current_receiver.owner_token != 0
                    and current_receiver.owner_token >= argument_owner_token_floor
                ):
                    # The evaluated bound method owns the pre-argument receiver.
                    # The evaluated argument transfer proves that the slot's
                    # latest owner is freshly allocated and therefore cannot
                    # alias that receiver. Other aliases still lose contents.
                    receiver_write_exempt_slots = frozenset((candidate_slot,))
                if (
                    candidate_slot is not None
                    and not receiver_rebound
                    and not prior_effects & receiver_boundary
                ):
                    current_result = current_receiver.result
                    if current_result.kind == receiver_result.kind:
                        receiver_result = current_result
                        receiver_slot = candidate_slot
            if prior_effects & receiver_boundary:
                receiver_result = expression_result_without_mutable_contents(
                    receiver_result
                )
            namespace_call = self._current_globals_method(
                state_id, scope, node, receiver_result, argument_results, prior_effects
            )
            if namespace_call is not None:
                return namespace_call
            method_shape = builtin_method_call_shape(
                receiver_result,
                node.func.attr,
                node,
                argument_results,
            )
        if method_shape is not None:
            result_fact = method_shape.result
            result = result_fact.identities
            effects |= method_shape.invocation_effects
            specialization_valid = method_shape.specialization_valid
            state_id = self._apply_effects(
                state_id,
                effects,
                object_write_may_invalidate_members=(
                    receiver_result.kind == "dict"
                    and captured_receiver_owner_token == 0
                ),
                object_write_content_exempt_slots=receiver_write_exempt_slots,
            )
            if method_shape.receiver_after is not None and receiver_slot is not None:
                current_receiver = self.states._binding_resolution(
                    state_id, receiver_slot
                ).public()
                self.states.binding_lookups += 1
                state_id = self.states.set_binding(
                    state_id,
                    receiver_slot,
                    current_receiver.identities,
                    current_receiver.static_value,
                    method_shape.receiver_after,
                    current_receiver.owner_token,
                )
            return _CallSemantics(
                result,
                effects,
                state_id,
                result_fact,
                specialization_valid,
                method_shape.argument_effects,
                # Releasing the evaluated builtin bound method is safe when a
                # live binding still owns its receiver, or when the exact
                # post-call receiver can itself retire without callbacks.
                method_shape.receiver_after is not None
                and (
                    receiver_slot is not None
                    or not method_shape.receiver_after.release_may_call
                ),
                method_shape.retained_argument_indices,
            )
        if callee & INTRINSIC_ACQUISITION_IDENTITIES:
            result = intrinsic_result
            if not exact:
                result |= OTHER_IDENTITY
            if callee & OPTIONAL_INTRINSIC_ACQUISITION_IDENTITIES:
                result |= int(PythonIdentity.INERT_VALUE)
            result_fact = StaticExpressionResult(
                identities=result, evaluation_required=True
            )
            # Preserve the acquisition's existing effects and all operand
            # lifetime rules. Only its successful result identity is known.
            effects |= UNKNOWN_EFFECTS
        elif exact and callee & self.policy.normal_none_intrinsics:
            # Knowing a normal result says nothing about evaluation: the
            # provider may audit, raise, call Python, and retire its operands.
            result_fact = StaticExpressionResult.scalar(None, evaluation_required=True)
            result = result_fact.identities
            effects |= UNKNOWN_EFFECTS
            source_result_projected = True
        elif (
            self.policy.molt_runtime_linked
            and callee == int(PythonIdentity.INTRINSICS_RUNTIME_ACTIVE)
            and not node.args
            and not node.keywords
        ):
            # The compiler-owned callable returns the immediate True value
            # without consulting Python namespaces or allocating. Ordinary
            # module attribute lookup does not establish this identity.
            result_fact = StaticExpressionResult.scalar(True)
            result = result_fact.identities
            specialization_valid = True
        elif (
            (target_property := INTRINSIC_TARGET_PROPERTIES.get(callee)) is not None
            and not node.args
            and not node.keywords
        ):
            result_fact = _target_property_result(
                target_property,
                self.policy.target_sys_platform,
                self.policy.target_python,
            )
            result = int(PythonIdentity.INERT_VALUE)
            effects |= ALLOCATES | RAISES
        elif (
            (attribute_read := INTRINSIC_ATTRIBUTE_READS.get(callee)) is not None
            and not node.keywords
            and len(node.args) == len(argument_results) == 1
            and not isinstance(node.args[0], ast.Starred)
        ):
            owner = argument_results[attribute_read[0]]
            selected = python_source_projection_key(
                node, PythonSourceProjection.CALL_ATTRIBUTE
            )
            self._publish_source_attribute(
                selected, attribute_read[1], owner.source_origins
            )
            source_result_projected = True
            result_fact = StaticExpressionResult(source_origins=frozenset({selected}))
            effects |= READS_OBJECT_STATE | INVOKES_DESCRIPTOR | RAISES
            argument_protocol_indices = frozenset()
            selected_deferred = self._attribute_dispatch(owner, effects)
        elif (
            (entry_read := INTRINSIC_NAMESPACE_ENTRY_READS.get(callee)) is not None
            and not node.keywords
            and len(node.args) == len(argument_results) == 3
            and not any(isinstance(arg, ast.Starred) for arg in node.args)
        ):
            # ``owner.__dict__.get(name, missing)``, as ``dict.get`` on the
            # attribute mapping models it: the same projections and keyed
            # read, with no lookup hook (the runtime reads the static cell,
            # then the mapping) and no operand escape.
            owner_index, name_index, missing_index = entry_read
            borrowed_module_operand = owner_index
            owner = argument_results[owner_index]
            mapping = python_source_projection_key(
                node, PythonSourceProjection.CALL_ATTRIBUTE
            )
            self._publish_source_attribute(mapping, "__dict__", owner.source_origins)
            selected = python_source_projection_key(
                node, PythonSourceProjection.CALL_MAPPING_VALUE
            )
            name = argument_results[name_index]
            key = name.value if name.value_known and type(name.value) is str else None
            prior = self.transfer.source_elements.get(selected)
            self.transfer.source_elements[selected] = PythonSourceElement(
                selected,
                PythonElementProtocol.SUBSCRIPT,
                frozenset({mapping}) | (prior.origins if prior else frozenset()),
                key if prior is None or prior.key == key else None,
            )
            result_fact = join_static_expression_results(
                (
                    StaticExpressionResult(source_origins=frozenset({selected})),
                    argument_results[missing_index],
                )
            )
            result = result_fact.identities
            source_result_projected = True
            effects |= READS_OBJECT_STATE | RAISES
            argument_protocol_indices = frozenset()
        elif (
            INTRINSIC_NATIVE_INITIALIZERS.get(callee)
            is PythonNativeInitializer.DATACLASS
            and not node.keywords
            and len(node.args) == len(argument_results) == 3
            and not any(isinstance(arg, ast.Starred) for arg in node.args)
            and argument_results[2].kind == "tuple"
            and (items := argument_results[2].items) is not None
            and len(items) == len(DataclassInitSpecField)
            and not any(item.expanded for item in items)
        ):
            # Construction retains function metadata. The returned plan owns
            # invocation callbacks; an environment dictionary is not a callback.
            selected = python_source_projection_key(
                node, PythonSourceProjection.CALL_NATIVE_INITIALIZER
            )
            builtins = python_source_projection_key(
                node, PythonSourceProjection.CALL_NATIVE_BUILTINS
            )
            global_sources = items[DataclassInitSpecField.GLOBALS].result.source_origins
            explicit_builtins = python_source_projection_key(
                node, PythonSourceProjection.CALL_MAPPING_VALUE
            )
            previous = self.transfer.source_elements.get(explicit_builtins)
            self.transfer.source_elements[explicit_builtins] = PythonSourceElement(
                explicit_builtins,
                PythonElementProtocol.SUBSCRIPT,
                global_sources | (previous.origins if previous else frozenset()),
                "__builtins__",
            )
            attributes = (
                ("__builtins__", frozenset({builtins, explicit_builtins})),
                ("__globals__", global_sources),
                (
                    "__defaults__",
                    items[
                        DataclassInitSpecField.DEFAULTS
                    ].result.exposed_source_origins,
                ),
                (
                    "__kwdefaults__",
                    items[
                        DataclassInitSpecField.KWDEFAULTS
                    ].result.exposed_source_origins,
                ),
                (
                    "__annotations__",
                    items[
                        DataclassInitSpecField.ANNOTATIONS
                    ].result.exposed_source_origins,
                ),
                (
                    "__closure__",
                    items[
                        DataclassInitSpecField.CAPTURES
                    ].result.exposed_source_origins,
                ),
            )

            def tuple_values(result):
                if (
                    result.kind != "tuple"
                    or result.items is None
                    or any(item.expanded for item in result.items)
                ):
                    return None
                return tuple(item.result for item in result.items)

            def literal(result, kind):
                return (
                    result.value if result.kind == kind and result.value_known else None
                )

            params = tuple_values(items[DataclassInitSpecField.PARAMETERS].result)
            captures = tuple_values(items[DataclassInitSpecField.CAPTURES].result)
            stores = tuple_values(items[DataclassInitSpecField.STORES].result)
            post = tuple_values(items[DataclassInitSpecField.POST_INIT_INPUTS].result)
            store_rows = (
                None if stores is None else tuple(tuple_values(row) for row in stores)
            )
            post_rows = (
                None if post is None else tuple(tuple_values(row) for row in post)
            )
            projection = PythonNativeInitializerSource(
                selected,
                argument_results[0].source_origins,
                items[DataclassInitSpecField.CAPTURES].result.exposed_source_origins,
                attributes,
                tuple(str(value.value) for value in params)
                if params is not None
                and all(value.kind == "str" and value.value_known for value in params)
                else None,
                tuple(
                    (value.source_origins, value.exposed_source_origins)
                    for value in captures
                )
                if captures is not None
                else None,
                tuple(
                    (
                        literal(row[0], "str"),
                        literal(row[1], "int"),
                        literal(row[2], "int"),
                        literal(row[3], "int"),
                        literal(row[4], "bool"),
                    )
                    for row in store_rows
                )
                if store_rows is not None
                and all(row is not None and len(row) == 5 for row in store_rows)
                else None,
                tuple(
                    (literal(row[0], "str"), literal(row[1], "int"))
                    for row in post_rows
                )
                if post_rows is not None
                and all(row is not None and len(row) == 2 for row in post_rows)
                else None,
            )
            previous = self.transfer.source_native_initializers.get(selected)
            if previous is not None:
                projection = projection._replace(
                    receiver=projection.receiver | previous.receiver,
                    captures=projection.captures | previous.captures,
                    attributes=tuple(
                        (name, origins | prior[1])
                        for (name, origins), prior in zip(
                            attributes, previous.attributes, strict=True
                        )
                    ),
                    parameters=projection.parameters
                    if projection.parameters == previous.parameters
                    else None,
                    capture_values=projection.capture_values
                    if projection.capture_values == previous.capture_values
                    else None,
                    stores=projection.stores
                    if projection.stores == previous.stores
                    else None,
                    post_inputs=projection.post_inputs
                    if projection.post_inputs == previous.post_inputs
                    else None,
                )
            self.transfer.source_native_initializers[selected] = projection
            source_result_projected = True
            result_fact = StaticExpressionResult(source_origins=frozenset({selected}))
            # Preserve construction/factory, failure and retirement effects.
            effects |= UNKNOWN_EFFECTS
            argument_protocol_indices = frozenset()
        elif builtin_name is not None:
            shape = builtin_call_shape(builtin_name, node, argument_results)
            result_fact = shape.result
            result = result_fact.identities
            effects |= shape.invocation_effects
            specialization_valid = shape.specialization_valid
            retained_argument_indices = shape.retained_argument_indices
        elif (
            callee_value.class_call_proofs & ClassCallProof.DEFAULT_CONSTRUCTOR
            and not prior_effects & _CALLEE_RETENTION_FORBIDDEN_EFFECTS
        ):
            # This fact comes from the completed class namespace, not the
            # callee spelling. Arguments still evaluate and retire normally;
            # invalid arity still raises before publishing any result.
            effects |= ALLOCATES | RAISES
        elif (
            callee & _NAMESPACE_READER_CALL_IDENTITIES
            and not node.args
            and not node.keywords
        ):
            result = NO_IDENTITIES
            if callee & int(PythonIdentity.BUILTIN_GLOBALS) or scope.kind == "module":
                result |= int(PythonIdentity.CURRENT_GLOBALS)
            if scope.kind != "module" and callee & _LOCALS_READER_CALL_IDENTITIES:
                result |= int(PythonIdentity.CURRENT_LOCALS)
            if not exact:
                # Deferred/rebound activation lookup can still select the
                # canonical builtin. Transport that alternative, never promote
                # it to an exact namespace or a callback-free operation.
                result |= OTHER_IDENTITY
                effects |= UNKNOWN_EFFECTS
            else:
                effects |= REFLECTS_NAMESPACE
                effects |= (
                    READS_GLOBAL_NAMESPACE
                    if callee == int(PythonIdentity.BUILTIN_GLOBALS)
                    else READS_FRAME_STATE
                )
        elif exact and callee in {
            int(PythonIdentity.GLOBALS_SETITEM),
            int(PythonIdentity.GLOBALS_DELITEM),
        }:
            deleting = callee == int(PythonIdentity.GLOBALS_DELITEM)
            if globals_mutation_call_identity(node, callee) is not None:
                key_fact = self.transfer.expressions.get(self._node_key(node.args[0]))
                value_fact = (
                    None
                    if deleting
                    else self.transfer.expressions.get(self._node_key(node.args[1]))
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
                    UNKNOWN_EXPRESSION_RESULT
                    if value_fact is None
                    else value_fact.result,
                    scope=scope,
                )
                key = self._node_key(node)
                self.transfer.assignment_effects[key] = (
                    self.transfer.assignment_effects.get(key, NO_EFFECTS)
                    | mutation_effects
                )
                # Callee/arguments have executed; publication applies only its
                # own release effects. Reapplying aggregate namespace writes
                # here would erase the exact replacement we just established.
                return _CallSemantics(
                    int(PythonIdentity.INERT_VALUE),
                    mutation_effects,
                    state_id,
                    StaticExpressionResult.scalar(None, evaluation_required=True),
                    specialization_valid,
                    mutation_effects,
                    # This identity requires the evaluated receiver's exact
                    # builtin dict proof. Its active frame roots that receiver;
                    # releasing the builtin bound method cannot retire it.
                    callee_release_safe=True,
                    retained_argument_indices=(
                        frozenset({1})
                        if not deleting
                        and not mutation_effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
                        else frozenset()
                    ),
                )
            else:
                effects |= UNKNOWN_EFFECTS
            result = int(PythonIdentity.INERT_VALUE)
        elif (
            callee & int(PythonIdentity.INSPECT_CURRENTFRAME)
            and not node.args
            and not node.keywords
        ):
            result = int(PythonIdentity.CURRENT_FRAME)
            effects |= READS_FRAME_STATE | REFLECTS_NAMESPACE | ALLOCATES
            if not exact:
                result |= OTHER_IDENTITY
                effects |= UNKNOWN_EFFECTS
        elif callee & int(PythonIdentity.MODULE_SPEC_CLASS):
            result = exact_identity(PythonIdentity.MODULE_SPEC_INSTANCE)
            member_state = self.states.get(state_id)
            if not exact or member_state.maybe_invalidated_members & int(
                PythonMember.MODULE_SPEC_CLASS
            ):
                result |= OTHER_IDENTITY
            effects |= ALLOCATES | RAISES
        elif callee & _IMPORT_MACHINERY_CALL_IDENTITIES:
            result = OTHER_IDENTITY
            effects |= (
                EXECUTES_ARBITRARY_PYTHON | INVOKES_IMPORT_SYSTEM | ALLOCATES | RAISES
            )
        elif (
            (
                (may_getattr := bool(callee & int(PythonIdentity.BUILTIN_GETATTR)))
                or callee
                in {
                    int(PythonIdentity.BUILTIN_HASATTR),
                    int(PythonIdentity.BUILTIN_DELATTR),
                }
            )
            and not node.keywords
            and not any(isinstance(arg, ast.Starred) for arg in node.args)
            and len(node.args) in ((2, 3) if may_getattr else (2,))
            and (
                members := finite_python_strings(
                    self.transfer.expressions[self._node_key(node.args[1])].static_value
                )
                or (
                    (argument_results[1].value,)
                    if argument_results[1].value_known
                    and type(argument_results[1].value) is str
                    else None
                )
            )
        ):
            # Finite evaluated names use the same member graph as obj.member.
            # The selected result, including getattr's default, may escape.
            if exact and callee != int(PythonIdentity.BUILTIN_DELATTR):
                borrowed_module_operand = 0
            owner = argument_results[0]
            # A projection is separate from the name operand's own evaluation
            # and from a replacement callee's ordinary result.
            selected = python_source_projection_key(
                node, PythonSourceProjection.CALL_ATTRIBUTE
            )
            source_result_projected = exact
            for member in members:
                self._publish_source_attribute(selected, member, owner.source_origins)
            if exact:
                argument_protocol_indices = frozenset()
                effects |= READS_OBJECT_STATE | INVOKES_DESCRIPTOR | RAISES
            else:
                effects |= UNKNOWN_EFFECTS
            selected_deferred = self._attribute_dispatch(owner, effects)
            if may_getattr:
                selected_deferred |= frozenset({DeferredExecution(selected, "read")})
                if exact:
                    result = NO_IDENTITIES
                    for member in members:
                        result |= self._member_value(
                            state_id, owner.identities, member, owner
                        )
                result_fact = StaticExpressionResult(
                    source_origins=frozenset({selected}), evaluation_required=True
                )
                if len(argument_results) == 3:
                    result |= argument_results[2].identities
                    result_fact = join_static_expression_results(
                        (result_fact, argument_results[2])
                    )
            else:
                result = int(PythonIdentity.INERT_VALUE)
                if callee == int(PythonIdentity.BUILTIN_DELATTR):
                    effects |= WRITES_OBJECT_STATE | _RELEASE_CALLBACK_EFFECTS
                    mutations = NO_INVALID_MEMBERS
                    for member in members:
                        mutations |= _member_write_mask(owner.identities, member)
                    state_id = self.states.invalidate_members(
                        state_id, mutations, definite=len(members) == 1
                    )
                    result_fact = StaticExpressionResult.scalar(None)
                    selected_deferred |= frozenset(
                        {DeferredExecution(selected, "release")}
                    ) | frozenset(
                        DeferredExecution(source, "mutate")
                        for source in owner.source_origins
                    )
                else:
                    result_fact = StaticExpressionResult(kind="bool")
                    selected_deferred |= frozenset(
                        {
                            DeferredExecution(selected, "read"),
                            DeferredExecution(selected, "release"),
                        }
                    )
        elif (
            callee & int(PythonIdentity.BUILTIN_GETATTR)
            and not node.keywords
            and len(node.args) in {2, 3}
            and not any(isinstance(arg, ast.Starred) for arg in node.args)
            and (owners := argument_results[0].source_origins)
            and self._can_select_builtin_environment(node.args[1])
        ):
            # The selected name can be unknown without erasing its receiver.
            selected = python_source_projection_key(
                node, PythonSourceProjection.CALL_REFLECTED_BUILTINS
            )
            prior = self.transfer.source_elements.get(selected)
            self.transfer.source_elements[selected] = PythonSourceElement(
                selected,
                PythonElementProtocol.REFLECTED_BUILTINS,
                owners | (prior.origins if prior else frozenset()),
            )
            result_fact = StaticExpressionResult(source_origins=frozenset({selected}))
            if len(argument_results) == 3:
                result_fact = join_static_expression_results(
                    (result_fact, argument_results[2])
                )
            effects |= UNKNOWN_EFFECTS
            selected_deferred = self._attribute_dispatch(
                argument_results[0], effects
            ) | frozenset({DeferredExecution(selected, "read")})
            if exact:
                # Descriptor and retirement effects still follow the bounded
                # getattr protocol. A read need not expose the operand as an
                # arbitrary argument to unknown code.
                argument_protocol_indices = frozenset(range(len(argument_results)))
        elif (
            exact
            and callee == int(PythonIdentity.BUILTIN_ISINSTANCE)
            and len(node.args) == 2
            and not node.keywords
            and not any(isinstance(arg, ast.Starred) for arg in node.args)
            and self._plain_class_value(node.args[1]) is not None
        ):
            # The default instance check reads the real type and may look up
            # the subject's __class__. The class operand has no custom
            # __instancecheck__, so this call does not publish either operand.
            # Keep callback and retirement effects, including the subject's
            # attribute hooks on a miss.
            borrowed_module_operand = 0
            result_fact = StaticExpressionResult(kind="bool")
            effects |= UNKNOWN_EFFECTS
            argument_protocol_indices = frozenset()
            selected_deferred |= _operand_dispatch_deferred(
                INVOKES_DESCRIPTOR | _RELEASE_CALLBACK_EFFECTS, argument_results[0]
            ) | _operand_dispatch_deferred(
                _RELEASE_CALLBACK_EFFECTS, argument_results[1]
            )
            if argument_results[0].exposes_module_globals:
                selected_deferred |= frozenset(
                    {DeferredExecution(self._module_source, "escape")}
                )
        elif (
            exact
            and callee == int(PythonIdentity.BUILTIN_TYPE)
            and len(node.args) == 1
            and not node.keywords
            and not isinstance(node.args[0], ast.Starred)
        ):
            # `type(x)` reads the class edge of `x`'s header: it runs no
            # Python, cannot fail, and neither calls nor publishes `x`
            # (HF-971). The result is a class object of unknown identity.
            argument_protocol_indices = frozenset()
        elif exact and callee in NATIVE_SELECTOR_CONSTRUCTORS:
            # The immutable native class stores its operands in the callable.
            # Metadata and callback effects belong to the later invocation.
            effects |= ALLOCATES | RAISES
            argument_protocol_indices = frozenset()
            retained_argument_indices = frozenset(range(len(argument_results)))
        elif (
            callee == int(PythonIdentity.BUILTIN_ITER)
            and len(node.args) == 1
            and not node.keywords
            and not isinstance(node.args[0], ast.Starred)
        ):
            # The source call transfer owns iterator acquisition and the
            # value retained by the resulting iterator. The operand does not
            # escape to a separate unknown function.
            argument_protocol_indices = frozenset()
            argument = argument_results[0]
            selected_deferred = frozenset(
                DeferredExecution(ref.source, "get_iter")
                for ref in argument.deferred
                if ref.phase in {"resume", "escape"}
            )
            effects |= UNKNOWN_EFFECTS
        elif exact and callee == int(PythonIdentity.BUILTIN_SETATTR):
            effects |= (
                WRITES_OBJECT_STATE
                | INVOKES_DESCRIPTOR
                | EXECUTES_ARBITRARY_PYTHON
                | _RELEASE_CALLBACK_EFFECTS
                | RAISES
            )
            if (
                len(node.args) == 3
                and not node.keywords
                and not any(isinstance(arg, ast.Starred) for arg in node.args)
                and _literal_string(node.args[1]) is not None
            ):
                argument_protocol_indices = frozenset({0, 1})
                # Attribute storage may publish the value to an opaque consumer.
                argument_escape_indices = frozenset({2})
            if node.args and argument_results:
                selected_deferred |= frozenset(
                    DeferredExecution(source, "mutate")
                    for source in argument_results[0].source_origins
                )
                stored_member = (
                    _literal_string(node.args[1]) if len(node.args) >= 2 else None
                )
                if stored_member is not None and not callee_value.source_origins:
                    selected_deferred |= self._member_store_demand(
                        self._node_key(node),
                        stored_member,
                        argument_results[0],
                    )
                else:
                    # An unknown member can shadow any class attribute.
                    selected_deferred |= frozenset(
                        DeferredExecution(source, "escape")
                        for source in argument_results[0].exposed_source_origins
                    )
            if len(node.args) >= 2:
                owner = self._expression_identity(node.args[0])
                member = _literal_string(node.args[1])
                if member is not None:
                    state_id = self._invalidate_member_target(state_id, owner, member)
                if owner & _MODULE_OR_UNKNOWN_IDENTITIES and (
                    member is None or member in MODULE_IMPORT_METADATA_NAMES
                ):
                    # A failed exact-owner proof cannot rule out sys.modules.get
                    # (or another lookup) returning this module.
                    effects |= WRITES_MODULE_METADATA | WRITES_GLOBAL_NAMESPACE
        elif callee & _CODE_EXECUTION_CALL_IDENTITIES:
            effects |= UNKNOWN_EFFECTS
        else:
            effects |= UNKNOWN_EFFECTS
        return _CallSemantics(
            result,
            effects,
            self._apply_effects(state_id, effects),
            result_fact,
            specialization_valid,
            effects,
            retained_argument_indices=retained_argument_indices,
            argument_protocol_indices=argument_protocol_indices,
            argument_escape_indices=argument_escape_indices,
            deferred_execution=selected_deferred,
            source_result_projected=source_result_projected,
            borrowed_module_operand=borrowed_module_operand,
        )

    @staticmethod
    def _attribute_dispatch(
        owner: StaticExpressionResult, effects: EffectMask
    ) -> frozenset[DeferredExecution]:
        # Source owners dispatch lookup hooks. Legacy unknown-value provenance
        # keeps its conservative transfer; it is not proof of a module object.
        legacy = replace(
            owner, source_origins=frozenset(), exposed_source_origins=frozenset()
        )
        return _operand_dispatch_deferred(
            effects, legacy, reaches_contents=True
        ) | frozenset(
            DeferredExecution(source, "descriptor") for source in owner.source_origins
        )

    def _member_store_demand(
        self,
        store: PythonSourceKey,
        member: str,
        owner: StaticExpressionResult,
    ) -> frozenset[DeferredExecution]:
        """Name the member a store writes on a source owner: an instance
        attribute that shadows a class attribute changes what a load of that
        name on the instance selects, so the source demands must see it."""
        if not owner.exposed_source_origins:
            return frozenset()
        self._publish_source_attribute(store, member, owner.exposed_source_origins)
        return frozenset({DeferredExecution(store, "mutate")})

    def _super_receiver_origins(
        self,
        node: ast.Call,
        source_callee: bool,
        state_id: int,
        scope: _Scope,
    ) -> frozenset[PythonSourceKey]:
        """The origins of the first argument a `super().name` load binds, or
        none when the call is not that form: possibly the builtin and no
        source callable, called with no argument as an attribute receiver, in
        a function whose first positional parameter has source origins. A
        rebound `super` keeps its own generic call transfer beside this."""
        if (
            source_callee
            or node.args
            or node.keywords
            or id(node) not in self._super_attribute_receivers
            or scope.kind != "function"
        ):
            return frozenset()
        owner = self.transfer.executed_function_nodes.get(scope.source)
        if owner is None:
            return frozenset()
        positional = (*owner.args.posonlyargs, *owner.args.args)
        if not positional:
            return frozenset()
        slot = scope.slots.get(positional[0].arg)
        if slot is None:
            return frozenset()
        return self.states.result(state_id, slot).source_origins

    def _expression_identity(self, node: ast.AST) -> IdentityMask:
        fact = self.transfer.expressions.get(self._node_key(node))
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
            if result.result.is_exact_scalar
            or self._known_expression_result(node).kind
            not in {"unknown", "file_text", "file_bytes"}
            or result.identities
            in {
                int(PythonIdentity.INERT_VALUE),
                int(PythonIdentity.STATIC_FALSE),
            }
            else INVOKES_COMPARISON_CALLBACK | RAISES
        )
        key = self._node_key(node)
        fact = self.transfer.expressions[key]
        deferred_execution = _operand_dispatch_deferred(
            truth_effects, result.result, protocol="truth"
        )
        scope.deferred_execution |= deferred_execution
        self.transfer.expressions[key] = replace(
            fact,
            truth_effects=fact.truth_effects | truth_effects,
            deferred_execution=fact.deferred_execution | deferred_execution,
        )
        return _ExpressionResult(
            self._apply_effects(result.state_id, truth_effects),
            result.identities,
            result.effects | truth_effects,
            result.static_value,
            result.assignment_target,
            result.result,
        )

    def eval_expr(
        self, node: ast.expr, state_id: int, scope: _Scope
    ) -> _ExpressionResult:
        if self._binding_read_scopes:
            return self._eval_expr(node, state_id, scope)
        self._binding_read_scopes.append({})
        try:
            return self._eval_expr(node, state_id, scope)
        finally:
            self._binding_read_scopes.pop()

    def _capture_pending_binding_reads(self, slot: int | None = None) -> None:
        for reads in self._binding_read_scopes:
            if slot is None:
                captured = tuple(key for keys in reads.values() for key in keys)
                reads.clear()
            else:
                captured = reads.pop(slot, ())
            for key in captured:
                fact = self.transfer.expressions[key]
                if not fact.binding_capture_required:
                    self.transfer.expressions[key] = replace(
                        fact, binding_capture_required=True
                    )

    def _eval_expr(
        self, node: ast.expr, state_id: int, scope: _Scope
    ) -> _ExpressionResult:
        observation_before = self._namespace_observation_epoch
        deferred_execution: frozenset[DeferredExecution] = frozenset()
        module_metadata_effects = NO_EFFECTS
        effects = NO_EFFECTS
        identities = OTHER_IDENTITY
        static_value: PythonStaticValue = None
        owner_token = 0
        binding_invalidated = False
        binding_is_bound = False
        result_override: StaticExpressionResult | None = None
        # Child evaluation has already transferred its effects. Each branch
        # applies only its own operation boundary; the aggregate below is an
        # observation for consumers, never a second state-transfer program.
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
            if self._namespace_can_call(
                state_id, scope, node.id
            ) or self._name_uses_activation_namespace(state_id, scope, node.id):
                effects |= READS_OBJECT_STATE | EXECUTES_ARBITRARY_PYTHON | RAISES
            (
                identities,
                static_value,
                result_override,
                binding_invalidated,
                binding_is_bound,
                owner_token,
            ) = self._resolve_name(state_id, scope, node.id)
            if identities & UNBOUND_IDENTITY or binding_invalidated:
                effects |= RAISES
            state_id = self._apply_effects(state_id, effects)
        elif isinstance(node, ast.Attribute):
            if isinstance(node.value, ast.Call) and isinstance(node.ctx, ast.Load):
                self._super_attribute_receivers.add(id(node.value))
            assignment_target, state_id, effects = self._evaluate_member_target(
                node, state_id, scope
            )
            owner_identities = assignment_target.owner_identities
            result_override = StaticExpressionResult(
                source_origins=(
                    frozenset({self._node_key(node)})
                    if assignment_target.owner_result.exposed_source_origins
                    else frozenset()
                ),
                deferred=frozenset(
                    DeferredExecution(
                        ref.source if ref.phase == "resume" else self._node_key(node),
                        ref.phase
                        if node.attr == "__call__"
                        else "factory"
                        if ref.phase == "resume"
                        and node.attr
                        in {
                            "__iter__",
                            "__aiter__",
                            "__await__",
                            "__anext__",
                            "asend",
                            "athrow",
                            "aclose",
                        }
                        else "escape",
                    )
                    for ref in assignment_target.owner_result.exposed_deferred
                ),
                deferred_complete=assignment_target.owner_result.deferred_complete,
            )
            identities = self._member_value(
                state_id, owner_identities, node.attr, assignment_target.owner_result
            )
            if (
                node.attr == "platform"
                and owner_identities == int(PythonIdentity.SYS_MODULE)
                and self.policy.target_sys_platform is not None
                and not self.states.get(state_id).maybe_invalidated_members
                & int(PythonMember.SYS_PLATFORM)
            ):
                identities = int(PythonIdentity.INERT_VALUE)
                static_value = self.policy.target_sys_platform
            member_effects = READS_OBJECT_STATE | RAISES
            owner_result = assignment_target.owner_result
            if identities & OTHER_IDENTITY and not builtin_method_descriptor_known(
                owner_result.kind, node.attr
            ):
                hooks = owner_result.attribute_hooks
                if hooks is None:
                    boundary = INVOKES_DESCRIPTOR | EXECUTES_ARBITRARY_PYTHON
                    deferred_execution |= _operand_dispatch_deferred(
                        boundary, owner_result, reaches_contents=True
                    )
                    member_effects |= boundary
                elif hooks:
                    member_effects |= INVOKES_DESCRIPTOR | EXECUTES_ARBITRARY_PYTHON
                    deferred_execution |= hooks
            state_id = self._apply_effects(state_id, member_effects)
            effects |= member_effects
        elif isinstance(node, ast.Subscript):
            assignment_target, state_id, effects = self._evaluate_member_target(
                node, state_id, scope
            )
            owner_identities = assignment_target.owner_identities
            member_effects = READS_OBJECT_STATE | RAISES
            if (
                owner_identities & int(PythonIdentity.SYS_MODULES)
                and isinstance(node.slice, ast.Name)
                and node.slice.id == "__name__"
            ):
                # The key or cache entry can be rebound. Keep the possible
                # current-module effects, and let the cache graph decide which
                # source value the exact dictionary read can actually select.
                identities = _MODULE_OR_UNKNOWN_IDENTITIES
                if owner_identities == int(PythonIdentity.SYS_MODULES):
                    result_override = StaticExpressionResult(
                        identities=identities,
                        source_origins=frozenset({self._node_key(node)}),
                    )
            else:
                shape = static_subscription_shape(
                    assignment_target.owner_result,
                    assignment_target.index_result,
                    slice_parts=assignment_target.slice_parts,
                )
                inert_alias = subscription_is_inert_alias(
                    owner_identities,
                    assignment_target.index_identities,
                    assignment_target.static_index,
                )
                if inert_alias:
                    shape = StaticSubscriptionShape(shape.result, invokes_python=False)
                if assignment_target.slice_parts is not None:
                    result_override = shape.result
                elif owner_identities == int(PythonIdentity.SYS_MODULES):
                    # The source scan publishes this lookup's cache-key
                    # projection. The selected value does not own the cache.
                    result_override = StaticExpressionResult(
                        source_origins=frozenset({self._node_key(node)})
                    )
                else:
                    result_override = self._source_element_result(
                        self._node_key(node),
                        assignment_target.owner_result,
                        shape.result,
                        protocol=PythonElementProtocol.SUBSCRIPT,
                        key=assignment_target.static_index
                        if type(assignment_target.static_index) is str
                        else None,
                    )
                if assignment_target.is_exact_globals_dict:
                    selected = self._current_globals_item(
                        state_id, assignment_target.static_index
                    )
                    if selected is not None:
                        result_override = selected
                identities = result_override.identities
                if result_override.value_known and type(result_override.value) in {
                    str,
                    int,
                }:
                    static_value = cast(str | int, result_override.value)
                if shape.invokes_python:
                    member_effects |= EXECUTES_ARBITRARY_PYTHON
                    subscription_owner = assignment_target.owner_result
                    deferred_execution |= frozenset(
                        DeferredExecution(source, "class_subscription")
                        for source in subscription_owner.source_origins
                        | frozenset(
                            ref.source
                            for ref in subscription_owner.deferred
                            if ref.phase in {"class", "escape"}
                        )
                    )
                    deferred_execution |= _operand_dispatch_deferred(
                        EXECUTES_ARBITRARY_PYTHON,
                        subscription_owner,
                        *(
                            assignment_target.slice_parts
                            or (assignment_target.index_result,)
                        ),
                        reaches_contents=True,
                    )
                # BINARY_SUBSCR releases its evaluated operands. An inert
                # selected element does not prove inert retirement of siblings.
                member_effects |= RELEASES_REFERENCE
                # An inert alias runs no Python, so an operand read from a
                # binding is still held by that binding when it is released.
                bound_operands = (
                    inert_alias
                    and not effects & EXECUTES_ARBITRARY_PYTHON
                    and self._binding_held(node.value)
                    and self._binding_held(node.slice)
                )
                if not bound_operands and (
                    assignment_target.owner_result.release_may_call
                    or any(
                        part.release_may_call
                        for part in (
                            assignment_target.slice_parts
                            or (assignment_target.index_result,)
                        )
                    )
                ):
                    member_effects |= _RELEASE_CALLBACK_EFFECTS
                    deferred_execution |= _operand_dispatch_deferred(
                        _RELEASE_CALLBACK_EFFECTS,
                        assignment_target.owner_result,
                        *(
                            assignment_target.slice_parts
                            or (assignment_target.index_result,)
                        ),
                        reaches_contents=True,
                    )
                    # Operand retirement follows selection and may mutate a
                    # selected alias without changing its own object identity.
                    result_override = expression_result_without_mutable_contents(
                        result_override
                    )
            state_id = self._apply_effects(state_id, member_effects)
            effects |= member_effects
        elif isinstance(node, ast.BinOp):
            left = self.eval_expr(node.left, state_id, scope)
            right = self.eval_expr(node.right, left.state_id, scope)
            retained_left = _result_after_retained_boundary(left.result, right.effects)
            boundary = binary_operation_effects(retained_left, right.result)
            deferred_execution |= _operand_dispatch_deferred(
                boundary, retained_left, right.result, reaches_contents=True
            )
            result_override = static_binary_result(retained_left, node.op, right.result)
            identities = result_override.identities
            if result_override.value_known and type(result_override.value) in {
                str,
                int,
            }:
                static_value = cast(str | int, result_override.value)
            effects = left.effects | right.effects | boundary
            state_id = self._apply_effects(right.state_id, boundary)
        elif isinstance(node, ast.Call):
            callee_result = self.eval_expr(node.func, state_id, scope)
            state_id = callee_result.state_id
            callee_state_id = state_id
            evaluation_effects = callee_result.effects
            argument_owner_token_floor = self._next_owner_token
            captured_receiver_owner_token = 0
            if (
                isinstance(node.func, ast.Attribute)
                and isinstance(node.func.value, ast.Name)
                and (receiver_slot := self._slot_for_name(scope, node.func.value.id))
                is not None
            ):
                captured_receiver_owner_token = self.states.owner_token(
                    state_id, receiver_slot
                )
            (
                state_id,
                argument_effects,
                argument_results,
                argument_crossed_boundaries,
            ) = self._eval_call_arguments(node, state_id, scope)
            evaluation_effects |= argument_effects
            metadata_at_invocation = (
                self._module_metadata_borrowing_snapshot(state_id, scope)
                if callee_result.identities & _IMPORT_MACHINERY_CALL_IDENTITIES
                else NO_MODULE_METADATA_PROOF
            )
            receiver_result = (
                self._known_expression_result(node.func.value)
                if isinstance(node.func, ast.Attribute)
                else UNKNOWN_EXPRESSION_RESULT
            )
            argument_nodes = (*node.args, *node.keywords)
            for argument_node, argument in zip(argument_nodes, argument_results):
                if isinstance(argument_node, ast.Starred) or (
                    isinstance(argument_node, ast.keyword) and argument_node.arg is None
                ):
                    deferred_execution |= frozenset(
                        ref
                        for ref in argument.deferred
                        if ref.phase in {"resume", "escape"}
                    )
            receives_module_globals = receiver_result.exposes_module_globals or any(
                result.exposes_module_globals for result in argument_results
            )
            deferred_execution |= frozenset(
                DeferredExecution(
                    ref.source,
                    "escape" if ref.source == self._module_source else "call",
                )
                for ref in callee_result.result.deferred
                if ref.phase in {"call", "escape"}
            )
            callee_fact = self.transfer.expressions.get(self._node_key(node.func))
            incomplete_module_callee = not callee_result.result.deferred_complete and (
                bool(callee_result.identities & _USER_CALLABLE_IDENTITIES)
                or bool(callee_result.result.deferred)
                or callee_fact is not None
                and callee_fact.binding_invalidated
                and callee_fact.name_lookup in {"global", "class_global"}
            )
            deferred_factory_only = (
                not incomplete_module_callee
                and callee_result.identities == int(PythonIdentity.USER_FUNCTION)
                and bool(callee_result.result.deferred)
                and not evaluation_effects & _CALLEE_RETENTION_FORBIDDEN_EFFECTS
                and all(ref.phase == "factory" for ref in callee_result.result.deferred)
            )
            intrinsic_result = (
                _intrinsic_acquisition_identity(node, argument_results)
                if callee_result.result.source_origins
                or callee_result.identities & INTRINSIC_ACQUISITION_IDENTITIES
                else OTHER_IDENTITY
            )
            if deferred_factory_only:
                call_semantics = _CallSemantics(
                    OTHER_IDENTITY,
                    ALLOCATES | RAISES,
                    state_id,
                    StaticExpressionResult(
                        exposes_module_globals=any(
                            result.exposes_module_globals for result in argument_results
                        ),
                        exposed_deferred=frozenset().union(
                            *(result.exposed_deferred for result in argument_results)
                        ),
                    ),
                    False,
                    ALLOCATES | RAISES,
                    retained_argument_indices=frozenset(range(len(argument_results))),
                )
            else:
                call_semantics = self._call_semantics(
                    state_id,
                    callee_state_id,
                    scope,
                    node,
                    callee_result.identities,
                    callee_result.result,
                    argument_results,
                    captured_receiver_owner_token,
                    argument_owner_token_floor,
                    evaluation_effects,
                    intrinsic_result,
                )
            identities = call_semantics.result_identities
            invocation_effects = call_semantics.invocation_effects
            state_id = call_semantics.state_id
            result_override = call_semantics.result
            deferred_execution |= call_semantics.deferred_execution
            if (
                callee_result.result.source_origins
                and not call_semantics.source_result_projected
            ):
                # A call's value is not its callee. Retain the source relation
                # while deferred jobs publish their actual return origins.
                call_source = self._node_key(node)
                dispatch = PythonSourceCallDispatch.OPEN
                if (
                    isinstance(node.func, ast.Call)
                    and len(node.args) == 1
                    and not node.keywords
                    and isinstance(node.args[0], ast.Tuple)
                ):
                    dispatch = PythonSourceCallDispatch.TUPLE_LITERAL_RECEIVER
                elif (
                    callee_result.identities == int(PythonIdentity.OPERATOR_ITEMGETTER)
                    and node.args
                    and not node.keywords
                    and all(
                        result.value_known and type(result.value) is int
                        for result in argument_results
                    )
                    and not any(isinstance(arg, ast.Starred) for arg in node.args)
                ):
                    dispatch = PythonSourceCallDispatch.ITEMGETTER_INTEGER_KEYS
                elif (
                    callee_result.identities == int(PythonIdentity.USER_FUNCTION)
                    and not node.args
                    and not node.keywords
                    and isinstance(node.func, ast.Name)
                    and callee_fact is not None
                    and not callee_fact.binding_invalidated
                    and not callee_fact.star_export_lookup
                ):
                    dispatch = PythonSourceCallDispatch.SOURCE_FUNCTION
                first_starred = next(
                    (
                        index
                        for index, argument in enumerate(node.args)
                        if isinstance(argument, ast.Starred)
                    ),
                    len(node.args),
                )
                source_arguments = tuple(
                    self._source_call_argument(
                        argument, index if index < first_starred else None, result
                    )
                    for index, (argument, result) in enumerate(
                        zip(node.args, argument_results[: len(node.args)], strict=True)
                    )
                ) + tuple(
                    self._source_call_argument(keyword.value, keyword.arg, result)
                    for keyword, result in zip(
                        node.keywords, argument_results[len(node.args) :], strict=True
                    )
                )
                self._publish_source_call(
                    PythonSourceCall(
                        call_source,
                        callee_result.result.source_origins,
                        intrinsic_result,
                        source_arguments,
                        tuple(
                            result.value
                            if result.value_known and type(result.value) is str
                            else None
                            for result in argument_results
                        ),
                        bound_update_keys=literal_mapping_update_keys(node),
                        unbound_update_keys=literal_mapping_update_keys(
                            node, unbound=True
                        ),
                        dispatch=dispatch,
                    )
                )
                result_override = replace(
                    result_override,
                    source_origins=result_override.source_origins | {call_source},
                )
            if callee_result.identities & int(PythonIdentity.BUILTIN_SUPER):
                receiver_origins = self._super_receiver_origins(
                    node, bool(callee_result.result.source_origins), state_id, scope
                )
                if receiver_origins:
                    super_source = self._node_key(node)
                    previous_super = self.transfer.source_elements.get(super_source)
                    self.transfer.source_elements[super_source] = PythonSourceElement(
                        super_source,
                        PythonElementProtocol.SUPER,
                        receiver_origins
                        | (
                            previous_super.origins
                            if previous_super is not None
                            else frozenset()
                        ),
                    )
                    result_override = replace(
                        result_override, source_origins=frozenset({super_source})
                    )
                else:
                    # An unmodelled `super` keeps every enclosing class, and
                    # through it each base, reachable by exposure.
                    owner_scope: _Scope | None = scope
                    while owner_scope is not None:
                        if owner_scope.kind == "class":
                            deferred_execution |= frozenset(
                                {DeferredExecution(owner_scope.source, "escape")}
                            )
                        owner_scope = owner_scope.parent
            if callee_result.identities & _DYNAMIC_IMPORT_CALL_IDENTITIES:
                # An import call returns a module, as an import statement binds
                # one: the call itself is the source the module value flows from.
                result_override = replace(
                    result_override,
                    source_origins=result_override.source_origins
                    | {self._node_key(node)},
                )
            # Exact builtins follow their invocation effects: copying retains
            # deferred values; hashing/repr/index protocols can reach elements.
            # Foreign calls may invoke or resume every value they receive.
            builtin = callee_result.identities in {
                int(identity) for identity in BUILTIN_SHAPE_IDENTITIES.values()
            }
            builtin |= (
                isinstance(node.func, ast.Attribute)
                and call_semantics.specialization_valid
                and not invocation_effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
            )
            protocol_indices = call_semantics.argument_protocol_indices
            escape_indices = call_semantics.argument_escape_indices
            if len(argument_results) == 1 and callee_result.identities & (
                int(PythonIdentity.BUILTIN_TYPE) | int(PythonIdentity.BUILTIN_VARS)
            ):
                # The result is the operand's class or its attribute mapping:
                # a store through either is a writer no member store names.
                escape_indices = escape_indices | {0}
            if (
                protocol_indices is None
                and callee_result.identities in IMPORT_CALLABLES
            ):
                # Canonical import APIs inspect their operands; they do not call
                # every function stored in a module or fallback they receive.
                protocol_indices = frozenset(range(len(argument_results)))
            if (
                protocol_indices is None
                and callee_result.identities == int(PythonIdentity.USER_FUNCTION)
                and self._opaque_activation_sources is not None
                and callee_result.result.source_origins
                and all(
                    source[-1] in {"FunctionDef", "AsyncFunctionDef", "Lambda"}
                    for source in callee_result.result.source_origins
                )
                and not self._activation_group_is_opaque(
                    callee_result.result.source_origins
                )
            ):
                # The existing call/formal graph owns uses of these arguments.
                # Recreated/escaped code and opaque callees retain generic exposure.
                protocol_indices = frozenset()
            truth_test = (
                callee_result.identities == int(PythonIdentity.BUILTIN_BOOL)
                and call_semantics.specialization_valid
                and len(argument_results) == 1
            )
            executes_protocol = not truth_test and bool(
                invocation_effects & INVOKES_ITERATION_CALLBACK
            )
            fallback_operand = import_call_fallback_operand(
                node, IMPORT_CALLABLES.get(callee_result.identities)
            )
            fallback_index = (
                next(
                    (
                        index
                        for index, operand in enumerate(
                            (*node.args, *(keyword.value for keyword in node.keywords))
                        )
                        if operand is fallback_operand
                    ),
                    None,
                )
                if fallback_operand is not None
                else None
            )
            opaque_argument_dispatch = not builtin and protocol_indices is None
            argument_call = None
            if (
                opaque_argument_dispatch
                and isinstance(node.func, ast.Call)
                and self._node_key(node) in self.transfer.source_calls
            ):
                argument_call = self._node_key(node)
            if (
                opaque_argument_dispatch
                and isinstance(node.func, ast.Attribute)
                and isinstance(node.func.value, ast.Name)
                and (
                    receiver_fact := self.transfer.expressions.get(
                        self._node_key(node.func.value)
                    )
                )
                is not None
                and receiver_fact.name_lookup == "global"
                and not receiver_fact.star_export_lookup
                and not receiver_fact.binding_invalidated
                and callee_result.result.source_origins == {self._node_key(node.func)}
                and self._node_key(node) in self.transfer.source_calls
                and not node.keywords
                and not any(isinstance(arg, ast.Starred) for arg in node.args)
            ):
                argument_call = self._node_key(node)
            for argument_index, argument in enumerate(
                () if deferred_factory_only else argument_results
            ):
                if argument_index == fallback_index:
                    # Evaluation and retirement still run. This API only
                    # returns its fallback; it does not call or expose it.
                    continue
                if (
                    protocol_indices is not None
                    and argument_index not in protocol_indices
                    and argument_index not in escape_indices
                ):
                    continue
                if argument_index not in escape_indices and (
                    builtin or protocol_indices is not None
                ):
                    deferred_execution |= _operand_dispatch_deferred(
                        invocation_effects,
                        argument,
                        reaches_contents=not truth_test,
                        protocol="truth" if truth_test else "protocol",
                    )
                    if executes_protocol:
                        deferred_execution |= frozenset(
                            DeferredExecution(ref.source, "iterate")
                            for ref in argument.deferred
                            if ref.phase in {"resume", "escape"}
                        )
                else:
                    deferred_execution |= frozenset(
                        DeferredExecution(ref.source, "escape")
                        for ref in argument.exposed_deferred
                    )
                    # Primary module/import sources need the same generic-call
                    # exposure as deferred objects. The scanner must not repeat
                    # this transfer for proven builtin/source callees.
                    phase: SourceDemandPhase = (
                        "opaque_argument" if opaque_argument_dispatch else "expose"
                    )
                    deferred_execution |= frozenset(
                        DeferredExecution(source, phase, argument_call)
                        for source in argument.exposed_source_origins
                    )
                    if argument.exposes_module_globals:
                        deferred_execution |= frozenset(
                            {DeferredExecution(self._module_source, "escape")}
                        )
            if not builtin:
                # The attribute selection binds a source receiver to the first
                # parameter of each function it selects, as a call binds an
                # argument to its formal; the source demands escape it only
                # where that selection is incomplete. Contents stay escaped.
                method_source = (
                    self._node_key(node.func)
                    if isinstance(node.func, ast.Attribute)
                    and receiver_result.source_origins
                    else None
                )
                bound_receivers = (
                    receiver_result.source_origins
                    if method_source in self.transfer.source_attributes
                    else frozenset()
                )
                deferred_execution |= frozenset(
                    DeferredExecution(method_source, "method")
                    if (
                        method_source is not None
                        and ref.source in bound_receivers
                        and ref.source[-1] != "Module"
                    )
                    else DeferredExecution(ref.source, "escape")
                    for ref in receiver_result.exposed_deferred
                )
            if callee_result.identities == int(
                PythonIdentity.BUILTIN_SETATTR
            ) or current_globals_dict_is_exact(
                receiver_result.identities, receiver_result
            ):
                module_metadata_effects |= invocation_effects & WRITES_MODULE_METADATA
            namespace_escape = (
                not deferred_factory_only
                and receives_module_globals
                and (
                    not builtin
                    or bool(invocation_effects & NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS)
                )
            )
            if (
                namespace_escape
                or callee_result.identities & _CODE_EXECUTION_CALL_IDENTITIES
            ):
                module_metadata_effects |= WRITES_MODULE_METADATA
            returned_deferred = frozenset(
                DeferredExecution(
                    ref.source if ref.phase == "factory" else self._node_key(node),
                    "resume" if ref.phase == "factory" else "escape",
                )
                for ref in callee_result.result.deferred
            )
            if returned_deferred:
                yielded_candidates = frozenset(
                    DeferredExecution(ref.source, "escape")
                    for ref in returned_deferred
                    if ref.phase == "resume"
                )
                result_override = replace(
                    result_override,
                    deferred=returned_deferred,
                    deferred_complete=callee_result.result.deferred_complete,
                    attribute_hooks=(
                        frozenset()
                        if deferred_factory_only
                        else callee_result.result.instance_attribute_hooks
                    ),
                    exposes_module_globals=result_override.exposes_module_globals
                    or callee_result.result.exposes_module_globals,
                    element_result=(
                        StaticExpressionResult(deferred=yielded_candidates)
                        if yielded_candidates
                        else result_override.element_result
                    ),
                )
            if receives_module_globals or deferred_execution or returned_deferred:
                self._module_import_flow_required = True
            specialization_valid = call_semantics.specialization_valid
            argument_cleanup_effects = (
                RELEASES_REFERENCE if argument_results else NO_EFFECTS
            )
            argument_nodes = tuple(
                argument.value if isinstance(argument, ast.Starred) else argument
                for argument in node.args
            ) + tuple(keyword.value for keyword in node.keywords)
            argument_boundary_before_cleanup = call_semantics.argument_effects
            callee_boundary_before_cleanup = evaluation_effects | invocation_effects
            if any(
                index not in call_semantics.retained_argument_indices
                and _result_after_retained_boundary(
                    result, argument_boundary_before_cleanup
                ).release_may_call
                and not (
                    isinstance(argument, ast.Name)
                    and not crossed_boundary
                    and not argument_boundary_before_cleanup
                    & _CALLEE_ELISION_FORBIDDEN_EFFECTS
                )
                for index, (argument, result, crossed_boundary) in enumerate(
                    zip(
                        argument_nodes,
                        argument_results,
                        argument_crossed_boundaries,
                        strict=True,
                    )
                )
            ):
                argument_cleanup_effects |= _RELEASE_CALLBACK_EFFECTS
            callee_elision_safe = (
                specialization_valid
                and isinstance(node.func, (ast.Name, ast.Attribute))
                and not (
                    evaluation_effects | invocation_effects | argument_cleanup_effects
                )
                & _CALLEE_ELISION_FORBIDDEN_EFFECTS
            )
            callee_retention_safe = (
                call_semantics.callee_release_safe
                and not argument_cleanup_effects & _CALLEE_RETENTION_FORBIDDEN_EFFECTS
            ) or (
                isinstance(node.func, (ast.Name, ast.Attribute))
                and not (
                    evaluation_effects | invocation_effects | argument_cleanup_effects
                )
                & _CALLEE_RETENTION_FORBIDDEN_EFFECTS
            )
            cleanup_effects = argument_cleanup_effects
            if not callee_elision_safe:
                cleanup_effects |= RELEASES_REFERENCE
                if (
                    _result_after_retained_boundary(
                        callee_result.result, callee_boundary_before_cleanup
                    ).release_may_call
                    and not callee_retention_safe
                ):
                    cleanup_effects |= _RELEASE_CALLBACK_EFFECTS
            if argument_cleanup_effects & _RELEASE_CALLBACK_EFFECTS:
                deferred_execution |= _operand_dispatch_deferred(
                    argument_cleanup_effects,
                    *(
                        result
                        for index, result in enumerate(argument_results)
                        if index not in call_semantics.retained_argument_indices
                    ),
                    reaches_contents=True,
                )
            if (
                not callee_retention_safe
                and cleanup_effects & _RELEASE_CALLBACK_EFFECTS
            ):
                deferred_execution |= _operand_dispatch_deferred(
                    cleanup_effects, callee_result.result, reaches_contents=True
                )
            state_id = self._apply_effects(state_id, cleanup_effects)
            if cleanup_effects & _CALLEE_ELISION_FORBIDDEN_EFFECTS:
                result_override = expression_result_without_mutable_contents(
                    result_override, preserve_owner=result_override.fresh_container
                )
            effects = evaluation_effects | invocation_effects | cleanup_effects
            if effects and not result_override.evaluation_required:
                # A result proof can prune successors but never erase work
                # required to acquire or invoke the callee or retire operands.
                result_override = replace(result_override, evaluation_required=True)
            key = self._node_key(node)
            previous_call = self.transfer.calls.get(key)
            builtin_specializations = NO_IDENTITIES
            candidates = callee_result.identities & _BUILTIN_SHAPE_IDENTITY_MASK
            while candidates:
                identity = candidates & -candidates
                candidates ^= identity
                if (
                    call_semantics.specialization_valid
                    if callee_result.identities == identity
                    else builtin_call_shape(
                        BUILTIN_SHAPE_NAMES_BY_IDENTITY[identity],
                        node,
                        argument_results,
                    ).specialization_valid
                ):
                    builtin_specializations |= identity
            if previous_call is not None:
                builtin_specializations &= previous_call.builtin_specializations
            self.transfer.calls[key] = PythonCallSiteFact(
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
                evaluation_effects
                | (
                    previous_call.evaluation_effects
                    if previous_call is not None
                    else NO_EFFECTS
                ),
                invocation_effects
                | (
                    previous_call.invocation_effects
                    if previous_call is not None
                    else NO_EFFECTS
                ),
                cleanup_effects
                | (
                    previous_call.cleanup_effects
                    if previous_call is not None
                    else NO_EFFECTS
                ),
                callee_elision_safe
                and (previous_call is None or previous_call.callee_elision_safe),
                callee_retention_safe
                and (previous_call is None or previous_call.callee_retention_safe),
                metadata_at_invocation
                if previous_call is None
                else metadata_at_invocation.intersect(
                    previous_call.module_metadata_at_invocation
                ),
                builtin_specializations=builtin_specializations,
                borrowed_module_operand=(
                    call_semantics.borrowed_module_operand
                    if previous_call is None
                    or previous_call.borrowed_module_operand
                    == call_semantics.borrowed_module_operand
                    else None
                ),
            )
        elif isinstance(node, ast.NamedExpr):
            if self._target_may_write_import_metadata(node.target):
                self._module_import_flow_required = True
            value = self.eval_expr(node.value, state_id, scope)
            state_id, target_effects = self.assign_target(
                node.target,
                value.identities,
                value.state_id,
                scope,
                static_value=value.static_value,
                result=value.result,
                owner_token=value.owner_token,
            )
            effects = value.effects | target_effects
            identities = value.identities
            static_value = value.static_value
            owner_token = value.owner_token
        elif isinstance(node, ast.Lambda):
            state_id, defaults_effect, defaults = self._eval_arguments(
                node.args, state_id, scope
            )
            effects |= defaults_effect | ALLOCATES
            identities = exact_identity(PythonIdentity.USER_FUNCTION)
            self._queue_function(node, scope, state_id, defaults)
            result_override = self._deferred_function_result(node)
        elif isinstance(node, ast.UnaryOp):
            if isinstance(node.op, ast.Not):
                operand = self._eval_truth_test(node.operand, state_id, scope)
                state_id, effects = operand.state_id, operand.effects
                identities = int(PythonIdentity.INERT_VALUE)
            else:
                operand = self.eval_expr(node.operand, state_id, scope)
                result_override = static_expression_result(
                    node, fact_result=self._known_expression_result
                )
                boundary = unary_operation_effects(operand.result, node.op)
                if result_override.is_exact_scalar:
                    identities = int(PythonIdentity.INERT_VALUE)
                    if (
                        result_override.value_known
                        and type(result_override.value) is int
                    ):
                        static_value = result_override.value
                deferred_execution |= _operand_dispatch_deferred(
                    boundary, operand.result
                )
                effects = operand.effects | boundary
                state_id = self._apply_effects(operand.state_id, boundary)
        elif isinstance(node, ast.IfExp):
            test = self._eval_truth_test(node.test, state_id, scope)
            truth = self._known_expression_result(node.test).truth
            if truth is not None:
                selected = self.eval_expr(
                    node.body if truth else node.orelse, test.state_id, scope
                )
                state_id = selected.state_id
                identities = selected.identities
                effects = test.effects | selected.effects
                static_value = selected.static_value
                result_override = selected.result
            else:
                left = self.eval_expr(node.body, test.state_id, scope)
                right = self.eval_expr(node.orelse, test.state_id, scope)
                state_id = self.states.join(left.state_id, right.state_id)
                identities = left.identities | right.identities
                effects = test.effects | left.effects | right.effects
                static_value = join_python_static_values(
                    left.static_value, right.static_value
                )
                result_override = join_static_expression_results(
                    (left.result, right.result)
                )
        elif isinstance(node, ast.BoolOp):
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
                left_fact = _result_after_retained_boundary(left_fact, right.effects)
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
                    and self._binding_held(left_node, _plain_load)
                    and self._binding_held(comparator, _plain_load)
                    and not (right.effects | boundary)
                    & _CALLEE_ELISION_FORBIDDEN_EFFECTS
                )
                left_release_fact = _result_after_retained_boundary(left_fact, boundary)
                right_release_fact = _result_after_retained_boundary(
                    right_fact, boundary
                )
                if not stable_loads and (
                    _release_may_call(left.identities, left_release_fact)
                    or _release_may_call(right.identities, right_release_fact)
                ):
                    boundary |= _RELEASE_CALLBACK_EFFECTS
                right_release_fact = _result_after_retained_boundary(
                    right_release_fact, boundary
                )
                deferred_execution |= _operand_dispatch_deferred(
                    boundary, left_fact, right_fact, reaches_contents=True
                )
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
                left, left_fact = right, right_release_fact
            state_id = self.states.join(*exits)
        elif isinstance(
            node, (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)
        ):
            state_id, comp_effects = self._eval_comprehension(node, state_id, scope)
            identities = exact_identity(PythonIdentity.OTHER)
            effects |= comp_effects | ALLOCATES
            if isinstance(node, ast.GeneratorExp):
                result_override = replace(
                    static_expression_result(
                        node, fact_result=self._known_expression_result
                    ),
                    source_origins=frozenset({self._node_key(node)}),
                    deferred=frozenset(
                        {DeferredExecution(self._node_key(node), "resume")}
                    ),
                    deferred_complete=True,
                    attribute_hooks=frozenset(),
                )
        elif isinstance(node, ast.Starred):
            value = self.eval_expr(node.value, state_id, scope)
            boundary = iterable_unpack_effects(
                node.value,
                fact_result=self._known_expression_result,
            )
            deferred_execution |= PythonIterationFact.from_result(
                value.result, boundary
            ).deferred_execution
            result_override = value.result
            effects = value.effects | boundary
            state_id = self._apply_effects(value.state_id, boundary)
        elif isinstance(node, (ast.Tuple, ast.List, ast.Set)):
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
                    deferred_execution |= _operand_dispatch_deferred(
                        hashing, key_result, reaches_contents=True
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
                deferred_execution |= _operand_dispatch_deferred(
                    boundary,
                    self._known_expression_result(value if key is None else key),
                    reaches_contents=True,
                )
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
                if isinstance(node, (ast.Yield, ast.YieldFrom)):
                    scope.escaped_deferred |= result.result.exposed_deferred
                    yielded = (
                        self._source_element_result(
                            python_source_projection_key(
                                node, PythonSourceProjection.YIELD_FROM_ELEMENTS
                            ),
                            result.result,
                            PythonIterationFact.from_result(
                                result.result, NO_EFFECTS
                            ).element_result,
                            protocol=PythonElementProtocol.ITERATE,
                        )
                        if isinstance(node, ast.YieldFrom)
                        else result.result
                    )
                    scope.yielded_source_origins |= yielded.source_origins
                    scope.yielded_exposed_source_origins |= (
                        yielded.exposed_source_origins
                    )
                    if result.result.exposes_module_globals:
                        scope.escaped_metadata_effects |= WRITES_MODULE_METADATA
                if isinstance(node, (ast.Await, ast.YieldFrom)):
                    result_override = StaticExpressionResult(
                        deferred=frozenset(
                            DeferredExecution(ref.source, "escape")
                            for ref in result.result.deferred
                        )
                    )
                    if result.result.source_origins:
                        source = self._node_key(node)
                        self.transfer.source_completions[source] = (
                            self.transfer.source_completions.get(source, frozenset())
                            | result.result.source_origins
                        )
                        result_override = replace(
                            result_override, source_origins=frozenset({source})
                        )
                    deferred_execution |= frozenset(
                        DeferredExecution(
                            source,
                            "await" if isinstance(node, ast.Await) else "iterate",
                        )
                        for source in result.result.source_origins
                    ) | frozenset(
                        DeferredExecution(
                            ref.source,
                            "escape"
                            if ref.source[-1] == "Module"
                            else "await"
                            if isinstance(node, ast.Await)
                            else "iterate",
                        )
                        for ref in result.result.deferred
                        if ref.phase in {"resume", "escape"}
                    )
            boundary = EXECUTES_ARBITRARY_PYTHON | SUSPENDS | RAISES
            state_id = self._apply_effects(state_id, boundary)
            effects |= boundary
            identities = OTHER_IDENTITY
        else:
            for child in ast.iter_child_nodes(node):
                if isinstance(child, ast.expr):
                    result = self.eval_expr(child, state_id, scope)
                    state_id = result.state_id
                    effects |= result.effects
            identities = OTHER_IDENTITY
            boundary = EXECUTES_ARBITRARY_PYTHON | RAISES
            deferred_execution |= _operand_dispatch_deferred(
                boundary,
                *(
                    self._known_expression_result(child)
                    for child in ast.iter_child_nodes(node)
                    if isinstance(child, ast.expr)
                ),
                reaches_contents=True,
            )
            state_id = self._apply_effects(state_id, boundary)
            effects |= boundary
            if isinstance(node, ast.JoinedStr):
                static_value = python_string_spelling(node)
        if identities & _FRAME_NAMESPACE_IDENTITIES:
            self._namespace_observation_epoch += 1
        expression_result = self._record_expression(
            node,
            scope,
            identities,
            effects,
            static_value,
            binding_invalidated,
            binding_is_bound,
            self._namespace_observation_epoch != observation_before,
            result_override,
            self._module_metadata_borrowing_snapshot(state_id, scope)
            if isinstance(node, ast.Name) and node.id in MODULE_IMPORT_METADATA_NAMES
            else NO_MODULE_METADATA_PROOF,
            deferred_execution=deferred_execution,
            module_metadata_effects=module_metadata_effects,
        )
        if (
            expression_result.fresh_container
            and owner_token == 0
            and isinstance(
                node,
                (
                    ast.Call,
                    ast.Dict,
                    ast.DictComp,
                    ast.List,
                    ast.ListComp,
                    ast.Set,
                    ast.SetComp,
                ),
            )
        ):
            owner_token = self._next_owner_token
            self._next_owner_token += 1
        self._record_state(state_id)
        if isinstance(node, ast.Name):
            key = self._node_key(node)
            fact = self.transfer.expressions[key]
            slot = self._slot_for_name(scope, node.id)
            if (
                slot is not None
                and fact.name_lookup == "lexical"
                and not fact.binding_capture_required
            ):
                self._binding_read_scopes[-1].setdefault(slot, set()).add(key)
        return _ExpressionResult(
            state_id,
            identities,
            effects,
            static_value,
            assignment_target,
            expression_result,
            owner_token,
        )

    def _eval_call_arguments(
        self, node: ast.Call | ast.ClassDef, state_id: int, scope: _Scope
    ) -> tuple[
        int,
        EffectMask,
        tuple[StaticExpressionResult, ...],
        tuple[bool, ...],
    ]:
        effects = NO_EFFECTS
        keys = AccumulatedKeyEffects()
        plan = self._call_argument_plan(node)
        # Evaluation always visits consecutive operand indices. Assembly may
        # consume them later, but custody and results share those indices.
        evaluated: list[StaticExpressionResult] = []
        published_release = False
        pending_release = False
        # Release effects already contributed to the surrounding observation.
        release_observed = 0
        # Publication observes every already evaluated operand as one prefix.
        publication_cursor = 0

        def record_exceptional_cleanup(boundary: EffectMask) -> None:
            nonlocal effects, publication_cursor, published_release, pending_release
            nonlocal release_observed
            publishes = bool(boundary & _CALLEE_ELISION_FORBIDDEN_EFFECTS)
            if not evaluated or not (boundary & RAISES or publishes):
                return
            if publishes:
                # Reconsider operands only when publication widens their
                # facts. Each value crosses this prefix boundary once.
                release_observed = min(release_observed, publication_cursor)
                for index in range(publication_cursor, len(evaluated)):
                    result = expression_result_without_mutable_contents(
                        evaluated[index]
                    )
                    evaluated[index] = result
                    published_release |= result.release_may_call
                publication_cursor = len(evaluated)
                pending_release = False
            if boundary & RAISES:
                cleanup = RELEASES_REFERENCE
                if published_release or pending_release:
                    cleanup |= _RELEASE_CALLBACK_EFFECTS
                effects |= cleanup
                if cleanup & _RELEASE_CALLBACK_EFFECTS:
                    # This state is reachable only if the current argument step
                    # fails, but statement exception flow must still observe
                    # callbacks triggered while releasing earlier temporaries.
                    if release_observed < len(evaluated):
                        self._record_operation_execution(
                            scope,
                            _operand_dispatch_deferred(
                                cleanup,
                                *evaluated[release_observed:],
                                reaches_contents=True,
                            ),
                        )
                        release_observed = len(evaluated)
                    self._record_state(self._apply_effects(state_id, cleanup))

        for step in plan.steps:
            if step.action == "evaluate":
                result = self.eval_expr(step.expression, state_id, scope)
                state_id = result.state_id
                record_exceptional_cleanup(result.effects)
                effects |= result.effects
                evaluated.append(result.result)
                pending_release |= (
                    result.result.release_may_call
                    and not plan.name_operands[step.index]
                )
            elif step.action == "kw":
                boundary = keys.add(StaticExpressionResult.scalar(step.name))
                record_exceptional_cleanup(boundary)
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
                iteration_key = self._node_key(plan.operands[step.index])
                iteration = PythonIterationFact.from_result(
                    evaluated[step.index], boundary
                )
                scope.deferred_execution |= iteration.deferred_execution
                previous = self.transfer.iterations.get(iteration_key)
                self.transfer.iterations[iteration_key] = (
                    iteration if previous is None else previous.merge(iteration)
                )
                if step.action == "kwstar":
                    boundary |= keys.extend(
                        static_expression_result(
                            step.expression, fact_result=self._known_expression_result
                        )
                    )
                if step.action == "kwstar":
                    self._record_operation_execution(
                        scope,
                        _operand_dispatch_deferred(
                            boundary, evaluated[step.index], reaches_contents=True
                        ),
                    )
                record_exceptional_cleanup(boundary)
                effects |= boundary
                state_id = self._apply_effects(state_id, boundary)
        return (
            state_id,
            effects,
            tuple(evaluated),
            tuple(index < publication_cursor for index in range(len(evaluated))),
        )

    def _eval_arguments(
        self, arguments: ast.arguments, state_id: int, scope: _Scope
    ) -> tuple[int, EffectMask, tuple[tuple[str, StaticExpressionResult], ...]]:
        effects = NO_EFFECTS
        defaults: list[tuple[str, StaticExpressionResult]] = []
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
            defaults.append((parameter.arg, result.result))
        return state_id, effects, tuple(defaults)

    def _eval_comprehension(
        self, node: ast.expr, state_id: int, parent: _Scope
    ) -> tuple[int, EffectMask]:
        generators = tuple(getattr(node, "generators"))
        declarations = PythonScopeDeclarations(
            frozenset(
                name
                for generator in generators
                for name in _target_names(generator.target)
            ),
            frozenset(),
            frozenset(),
        )
        deferred = isinstance(node, ast.GeneratorExp)
        generator_kind = FunctionKind.GENERATOR
        if deferred:
            assert isinstance(node, ast.GeneratorExp)
            assert self._dependency_authority is not None
            if (
                any(generator.is_async for generator in generators)
                or self._dependency_authority.summary(node).contains_await
            ):
                generator_kind = FunctionKind.ASYNC_GENERATOR
            source = self._node_key(node)
            self.transfer.source_callable_declarations[source] = (
                PythonSourceCallableDeclaration(source, generator_kind)
            )
            self.transfer.source_function_results.setdefault(
                source,
                PythonFunctionResult(
                    source, frozenset(), frozenset(), frozenset(), frozenset()
                ),
            )
        scope = self._new_scope(
            parent=parent,
            source_node=node,
            kind="comprehension",
            name="<comprehension>",
            declarations=declarations,
            activation_namespace_stable=False
            if deferred
            else parent.activation_namespace_stable,
        )
        activation_lexical_bindings: tuple[tuple[int, IdentityMask], ...] = ()
        if deferred:
            _module_bindings, activation_lexical_bindings = (
                self._activation_slot_bindings(node, parent, state_id)
            )
        first_iterable = self.eval_expr(generators[0].iter, state_id, parent)

        def iteration_for(
            generator: ast.comprehension, result: StaticExpressionResult
        ) -> PythonIterationFact:
            effects = iterable_unpack_effects(
                generator.iter, fact_result=self._known_expression_result
            )
            if generator.is_async:
                effects = (
                    INVOKES_ITERATION_CALLBACK
                    | EXECUTES_ARBITRARY_PYTHON
                    | SUSPENDS
                    | RAISES
                )
            iteration = PythonIterationFact.from_result(
                result, effects, asynchronous=bool(generator.is_async)
            )
            iteration = replace(
                iteration,
                element_result=self._source_element_result(
                    self._node_key(generator.target),
                    result,
                    iteration.element_result,
                    protocol=PythonElementProtocol.ASYNC_ITERATE
                    if generator.is_async
                    else PythonElementProtocol.ITERATE,
                ),
            )
            scope.deferred_execution |= iteration.deferred_execution
            return iteration

        first_iteration = iteration_for(generators[0], first_iterable.result)
        if deferred:
            self._record_operation_execution(
                parent,
                frozenset(
                    DeferredExecution(
                        ref.source,
                        "escape"
                        if ref.phase == "escape"
                        else "get_async_iter"
                        if generators[0].is_async
                        else "get_iter",
                    )
                    for ref in first_iteration.deferred_execution
                ),
            )
        immediate_effects = first_iterable.effects | first_iteration.effects
        immediate_state = self._apply_effects(
            first_iterable.state_id, first_iteration.effects
        )
        immediate_observation = self._namespace_observation_epoch
        entry = self.states.set_bindings(
            immediate_state,
            self._activation_input_updates(
                immediate_state, activation_lexical_bindings
            ),
        )
        keys = AccumulatedKeyEffects()

        def payload(incoming: int) -> PythonCompletionFlow[int]:
            current = incoming
            effects = NO_EFFECTS
            payloads = (
                (node.key, node.value)
                if isinstance(node, ast.DictComp)
                else (getattr(node, "elt"),)
            )
            key_result = UNKNOWN_EXPRESSION_RESULT
            for index, expression in enumerate(payloads):
                evaluated = self.eval_expr(expression, current, scope)
                current = evaluated.state_id
                effects |= evaluated.effects
                if index == 0:
                    key_result = evaluated.result
            if isinstance(node, ast.GeneratorExp):
                source = self._node_key(node)
                previous = self.transfer.source_function_results.get(
                    source,
                    PythonFunctionResult(
                        source,
                        frozenset(),
                        frozenset(),
                        frozenset(),
                        frozenset(),
                    ),
                )
                self.transfer.source_function_results[source] = previous._replace(
                    yielded=previous.yielded | key_result.source_origins,
                    yielded_exposed=previous.yielded_exposed
                    | key_result.exposed_source_origins,
                )
            if isinstance(node, (ast.DictComp, ast.SetComp)):
                # Insertion hashes after dict value evaluation, and equality can
                # revisit keys retained from an earlier comprehension iteration.
                insertion = keys.add(key_result)
                self._record_operation_execution(
                    scope,
                    _operand_dispatch_deferred(
                        insertion, key_result, reaches_contents=True
                    ),
                )
                current = self._apply_effects(current, insertion)
                effects |= insertion
            return self._normal_flow(current, effects)

        def generate(index: int, incoming: int) -> PythonCompletionFlow[int]:
            generator = generators[index]
            if index:
                iterable = self.eval_expr(generator.iter, incoming, scope)
                iteration = iteration_for(generator, iterable.result)
                entry = self._apply_effects(iterable.state_id, iteration.effects)
                prefix = self._normal_flow(
                    entry, iterable.effects | iteration.effects
                ).without_normal()
            else:
                iterable = first_iterable
                iteration = first_iteration
                entry = incoming
                prefix = PythonCompletionFlow()
            entry_epoch = self.states.get(entry).taint_epoch
            retained_boundary = NO_EFFECTS

            def filters(
                condition_index: int, current: int
            ) -> PythonCompletionFlow[int]:
                if condition_index == len(generator.ifs):
                    return (
                        generate(index + 1, current)
                        if index + 1 < len(generators)
                        else payload(current)
                    )
                condition = self._eval_truth_test(
                    generator.ifs[condition_index], current, scope
                )
                truth = condition.result.truth
                flow = self._normal_flow(
                    condition.state_id, condition.effects
                ).without_normal()
                if truth is not True:
                    flow = self._merge_flows(
                        flow, PythonCompletionFlow(continued=condition.state_id)
                    )
                if truth is not False:
                    flow = self._merge_flows(
                        flow, filters(condition_index + 1, condition.state_id)
                    )
                return flow

            def advance(header: int) -> tuple[int | None, PythonCompletionFlow[int]]:
                nonlocal retained_boundary
                current = self._apply_effects(header, iteration.effects)
                header_flow = self._normal_flow(current, iteration.effects)
                if iteration.empty:
                    return current, header_flow.without_normal()
                strings, element = iteration.element_strings, iteration.element_result
                tainted = self.states.get(current).taint_epoch != entry_epoch
                if tainted or retained_boundary & WRITES_OBJECT_STATE:
                    if iteration.mutable and (
                        tainted or not _owned_iterable_display(generator.iter)
                    ):
                        strings = None
                        element = expression_result_without_value_facts(element)
                    else:
                        element = expression_result_without_mutable_contents(element)
                current, assigned = self.assign_target(
                    generator.target,
                    element.identities,
                    current,
                    scope,
                    static_value=strings,
                    result=element,
                )
                body = self._merge_flows(
                    header_flow.without_normal(),
                    self._normal_flow(current, assigned).without_normal(),
                    filters(0, current),
                )
                retained_boundary |= body.effects
                return header_flow.normal, body

            def release_effects() -> EffectMask:
                boundary = iteration.effects | retained_boundary
                result = _result_after_iteration_boundary(iterable.result, boundary)
                owner = parent if index == 0 else scope
                rooted = (
                    isinstance(generator.iter, ast.Name)
                    and owner.kind != "class"
                    and not boundary & _CALLEE_ELISION_FORBIDDEN_EFFECTS
                )
                return (
                    _RELEASE_CALLBACK_EFFECTS
                    if result.release_may_call and not rooted
                    else NO_EFFECTS
                )

            def finalize(current: int) -> PythonCompletionFlow[int]:
                effects = release_effects()
                self._record_operation_execution(
                    scope,
                    _operand_dispatch_deferred(
                        effects, iterable.result, reaches_contents=True
                    ),
                )
                return self._normal_flow(self._apply_effects(current, effects), effects)

            loop = PythonCompletionFlow.loop(
                entry,
                advance,
                lambda exhausted: PythonCompletionFlow(normal=exhausted),
                join_states=self.states.join,
                equivalent_states=lambda left, right: (
                    self.states.equivalent(left, right)
                    and self.states.owner_tokens_equal(left, right)
                ),
                widen_state=lambda header: self.states.invalidate_members(
                    self._widen_exposed_bindings(header),
                    self.policy.callback_member_writes,
                ),
                finalize=finalize,
            )
            iteration = replace(
                iteration,
                release_effects=release_effects(),
                deferred_execution=iteration.deferred_execution
                | _operand_dispatch_deferred(
                    release_effects(), iterable.result, reaches_contents=True
                ),
            )
            key = self._node_key(generator)
            previous = self.transfer.iterations.get(key)
            self.transfer.iterations[key] = (
                iteration if previous is None else previous.merge(iteration)
            )
            return self._merge_flows(prefix, loop)

        if deferred:
            # Keep future writes out of enclosing exception observations, read
            # custody, and closure histories. Nested deferred definitions reuse
            # the same activation-history machinery as ordinary function jobs.
            previous_observers = self._observed_stack
            previous_modules = self._active_module_states
            previous_history = self._active_lexical_history
            previous_reads = self._binding_read_scopes
            observed = [entry]
            self._observed_stack = [_ObservedStateFrame(observed)]
            self._active_module_states = (entry,)
            self._active_lexical_history = observed
            self._binding_read_scopes = []
            try:
                generate(0, entry)
            finally:
                self._observed_stack = previous_observers
                self._active_module_states = previous_modules
                self._active_lexical_history = previous_history
                self._binding_read_scopes = previous_reads
                self._namespace_observation_epoch = immediate_observation
            return immediate_state, immediate_effects
        flow = generate(0, entry)
        parent.module_metadata_effects |= scope.module_metadata_effects
        parent.deferred_execution |= scope.deferred_execution
        assert flow.normal is not None
        return flow.normal, immediate_effects | flow.effects

    def _replace_binding(
        self,
        state_id: int,
        slot: int,
        value: IdentityMask,
        static_value: PythonStaticValue = None,
        result: StaticExpressionResult = UNKNOWN_EXPRESSION_RESULT,
        owner_token: int = 0,
        *,
        scope: _Scope,
    ) -> tuple[int, EffectMask]:
        return self._replace_bindings(
            state_id, ((slot, value, static_value, result, owner_token),), scope=scope
        )

    def _replace_bindings(
        self,
        state_id: int,
        bindings: Sequence[
            tuple[int, IdentityMask, PythonStaticValue, StaticExpressionResult, int]
        ],
        *,
        may_write: bool = False,
        scope: _Scope,
    ) -> tuple[int, EffectMask]:
        # STORE/DELETE publishes before releasing the old value. For a finite
        # choice of namespace keys, the non-relational state domain joins each
        # slot independently: retain its old value as a may-write, and publish
        # the whole abstract update once instead of constructing N branches.
        updates: list[
            tuple[int, IdentityMask, PythonStaticValue, StaticExpressionResult, int]
        ] = []
        releases_previous = False
        for slot, value, static_value, result, owner_token in bindings:
            self._capture_pending_binding_reads(slot)
            previous_binding = self.states._binding_resolution(state_id, slot).public()
            self.states.binding_lookups += 1
            previous = previous_binding.identities
            previous_result = previous_binding.result
            previous_has_value = bool(previous & ~UNBOUND_IDENTITY)
            # Rooted identities and exact result shapes are independent safety
            # proofs. Callback cleanup is required only when both are unknown.
            previous_can_release = previous_has_value and _release_may_call(
                previous, previous_result
            )
            releases_previous |= previous_can_release
            if previous_can_release:
                self._record_operation_execution(
                    scope,
                    _operand_dispatch_deferred(
                        _RELEASE_CALLBACK_EFFECTS,
                        previous_result,
                        reaches_contents=True,
                    ),
                )
            result = (
                expression_result_for_owned_binding(result)
                if owner_token
                else expression_result_for_publication(result)
            )
            if may_write:
                previous_static_value = previous_binding.static_value
                static_value, result = _join_binding_payloads(
                    (
                        (previous, previous_static_value, previous_result),
                        (value, static_value, result),
                    )
                )
                value |= previous
                owner_token = 0
            updates.append((slot, value, static_value, result, owner_token))
        state_id = self.states.set_bindings(state_id, updates, record_writes=True)
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
        index_result = UNKNOWN_EXPRESSION_RESULT
        index_identities = OTHER_IDENTITY
        slice_parts = None
        owner_result = owner.result
        if isinstance(target, ast.Subscript):
            index_effects = NO_EFFECTS
            if isinstance(target.slice, ast.Slice):
                parts = []
                for part in (target.slice.lower, target.slice.upper, target.slice.step):
                    if part is None:
                        parts.append(StaticExpressionResult.scalar(None))
                    else:
                        evaluated = self.eval_expr(part, state_id, scope)
                        state_id = evaluated.state_id
                        parts.append(evaluated.result)
                        index_effects |= evaluated.effects
                slice_parts = tuple(parts)
                index_effects |= ALLOCATES
            else:
                index = self.eval_expr(target.slice, state_id, scope)
                state_id = index.state_id
                index_effects = index.effects
                static_index = index.static_value
                index_result = index.result
                index_identities = index.identities
            effects |= index_effects
            if index_effects & _CALLEE_ELISION_FORBIDDEN_EFFECTS:
                owner_result = expression_result_without_mutable_contents(owner.result)
        return (
            _EvaluatedMemberTarget(
                target,
                owner.identities,
                owner_result,
                static_index,
                index_result,
                slice_parts,
                index_identities,
            ),
            state_id,
            effects,
        )

    def _store_evaluated_member(
        self,
        target: _EvaluatedMemberTarget,
        state_id: int,
        scope: _Scope,
        value: IdentityMask = OTHER_IDENTITY,
        static_value: PythonStaticValue = None,
        result: StaticExpressionResult = UNKNOWN_EXPRESSION_RESULT,
    ) -> tuple[int, EffectMask]:
        # Receiver/index identity and exact builtin type are retained across
        # RHS and in-place operator callbacks. Do not re-evaluate either source.
        if isinstance(target.node, ast.Subscript) and target.is_exact_globals_dict:
            state_id, effects = self._store_namespace_key(
                state_id, target.static_index, value, static_value, result, scope=scope
            )
            # Exact dict ownership removes receiver dispatch, not hashing or
            # comparison of the already evaluated key. Stores and deletes use
            # this same boundary, including augmented assignment publication.
            self._record_operation_execution(
                scope,
                _operand_dispatch_deferred(
                    effects, target.index_result, reaches_contents=True
                ),
            )
            return state_id, effects
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
            self._record_operation_execution(
                scope,
                frozenset(
                    DeferredExecution(source, "mutate")
                    for source in target.owner_result.source_origins
                )
                | self._member_store_demand(
                    self._node_key(target.node),
                    target.node.attr,
                    target.owner_result,
                ),
            )
            state_id = self._invalidate_member_target(
                state_id, target.owner_identities, target.node.attr
            )
            effects |= INVOKES_DESCRIPTOR
            if (
                target.owner_identities & int(PythonIdentity.CURRENT_MODULE)
                and target.node.attr in MODULE_IMPORT_METADATA_NAMES
            ):
                effects |= WRITES_MODULE_METADATA | WRITES_GLOBAL_NAMESPACE
        else:
            self._project_mapping_write(
                scope,
                target.node,
                target.owner_result.source_origins,
                target.static_index,
            )
            if (
                target.owner_identities & int(PythonIdentity.CURRENT_GLOBALS)
                or target.owner_result.exposes_module_globals
            ):
                effects |= WRITES_GLOBAL_NAMESPACE
                # An unknown key can select any import-metadata member.
                if strings is None or strings & MODULE_IMPORT_METADATA_NAMES:
                    effects |= WRITES_MODULE_METADATA
        self._record_operation_execution(
            scope,
            _operand_dispatch_deferred(
                effects,
                target.owner_result,
                target.index_result,
                result,
                *(target.slice_parts or ()),
                reaches_contents=True,
            ),
        )
        return self._apply_effects(state_id, effects), effects

    def _store_namespace_key(
        self,
        state_id: int,
        key: PythonStaticValue,
        value: IdentityMask,
        static_value: PythonStaticValue,
        result: StaticExpressionResult = UNKNOWN_EXPRESSION_RESULT,
        *,
        scope: _Scope,
        only_if_absent: bool = False,
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
            self._ensure_module_read_slots()
            state_id = self.states.invalidate_namespace_values(state_id)
            state_id = self._taint_module_slots(state_id)
            return self._apply_effects(state_id, UNKNOWN_EFFECTS), UNKNOWN_EFFECTS
        effects = RAISES
        if self.states.get(state_id).namespace_values_unknown:
            boundary = EXECUTES_ARBITRARY_PYTHON | INVOKES_COMPARISON_CALLBACK | RAISES
            state_id = self._apply_effects(state_id, boundary)
            effects |= boundary
        bindings = []
        for name in sorted(strings):
            slot = self._ensure_module_slot(name)
            replacement = result
            replacement_identity = value
            if only_if_absent:
                present = self.states._binding_resolution(state_id, slot).public()
                if present.present and not present.identities & UNBOUND_IDENTITY:
                    continue
                # A possibly present value survives setdefault. Select that
                # key's existing/default join, not the return union of all keys.
                replacement = self._current_globals_item(state_id, name, result)
                if replacement is None:
                    replacement = join_static_expression_results(
                        (UNKNOWN_EXPRESSION_RESULT, result)
                    )
                replacement_identity = replacement.identities
            bindings.append((slot, replacement_identity, static_value, replacement, 0))
        if not bindings:
            return state_id, effects
        effects |= WRITES_GLOBAL_NAMESPACE | WRITES_OBJECT_STATE
        if strings & MODULE_IMPORT_METADATA_NAMES:
            effects |= WRITES_MODULE_METADATA
        state_id, release_effects = self._replace_bindings(
            state_id,
            bindings,
            # Count every selectable key, including a present key for which
            # setdefault writes nothing. Filtering writes must not turn a
            # conditional insertion into an unconditional binding.
            may_write=len(strings) > 1,
            scope=scope,
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
        result: StaticExpressionResult = UNKNOWN_EXPRESSION_RESULT,
        owner_token: int = 0,
    ) -> tuple[int, EffectMask]:
        if result.exposes_module_globals:
            # Storage transports access without mutating the namespace. Keep
            # its later mutation/escape points on the same strict projection.
            self._module_import_flow_required = True
        assignment = self._assign_target(
            target,
            value,
            state_id,
            scope,
            static_value=static_value,
            result=result,
            owner_token=owner_token,
        )
        key = self._node_key(target)
        self.transfer.assignment_effects[key] = (
            self.transfer.assignment_effects.get(key, NO_EFFECTS) | assignment[1]
        )
        scope.module_metadata_effects |= assignment[1] & WRITES_MODULE_METADATA
        if isinstance(target, (ast.Tuple, ast.List)):
            iteration = PythonIterationFact.from_result(result, assignment[1])
            scope.deferred_execution |= iteration.deferred_execution
            previous = self.transfer.iterations.get(key)
            self.transfer.iterations[key] = (
                iteration if previous is None else previous.merge(iteration)
            )
        return assignment

    def _assign_target(
        self,
        target: ast.AST,
        value: IdentityMask,
        state_id: int,
        scope: _Scope,
        *,
        static_value: PythonStaticValue = None,
        result: StaticExpressionResult = UNKNOWN_EXPRESSION_RESULT,
        owner_token: int = 0,
    ) -> tuple[int, EffectMask]:
        effects = NO_EFFECTS
        if isinstance(target, ast.Name):
            return self._write_name(
                state_id,
                scope,
                target.id,
                value,
                static_value,
                result,
                owner_token,
            )
        if isinstance(target, (ast.Tuple, ast.List)):
            starred = next(
                (
                    index
                    for index, element in enumerate(target.elts)
                    if isinstance(element, ast.Starred)
                ),
                None,
            )
            selected = static_unpack_results(result, len(target.elts), starred)
            if (
                selected is None
                and isinstance(static_value, tuple)
                and (len(static_value) == len(target.elts) and starred is None)
            ):
                selected = tuple(
                    StaticExpressionResult.scalar(item) for item in static_value
                )
            if selected is not None:
                if result.kind == "unknown":
                    # Candidate item provenance does not specialize unpacking.
                    # Preserve the same dispatch boundary as the unknown path.
                    effects |= (
                        INVOKES_ITERATION_CALLBACK | EXECUTES_ARBITRARY_PYTHON | RAISES
                    )
                    state_id = self._apply_effects(state_id, effects)
                if (
                    result.length is None
                    or starred is None
                    and result.length != len(target.elts)
                    or starred is not None
                    and result.length < len(target.elts) - 1
                ):
                    effects |= RAISES
                for element, item_result in zip(target.elts, selected, strict=True):
                    if not isinstance(element, ast.Starred):
                        item_result = self._source_element_result(
                            self._node_key(element),
                            result,
                            item_result,
                            protocol=PythonElementProtocol.ITERATE,
                        )
                    item_static = (
                        cast(str | int, item_result.value)
                        if item_result.value_known
                        and type(item_result.value) in {str, int}
                        else None
                    )
                    state_id, element_effects = self.assign_target(
                        element.value if isinstance(element, ast.Starred) else element,
                        item_result.identities,
                        state_id,
                        scope,
                        static_value=item_static,
                        result=item_result,
                    )
                    effects |= element_effects
                return state_id, effects
            effects |= INVOKES_ITERATION_CALLBACK | EXECUTES_ARBITRARY_PYTHON | RAISES
            state_id = self._apply_effects(state_id, effects)
            for element in target.elts:
                state_id, element_effects = self.assign_target(
                    element,
                    OTHER_IDENTITY,
                    state_id,
                    scope,
                    result=self._source_element_result(
                        self._node_key(element),
                        result,
                        UNKNOWN_EXPRESSION_RESULT,
                        protocol=PythonElementProtocol.ITERATE,
                    ),
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
                evaluated, state_id, scope, value, static_value, result
            )
            return state_id, evaluation_effects | store_effects
        return self._apply_effects(state_id, UNKNOWN_EFFECTS), UNKNOWN_EFFECTS

    def delete_target(
        self, target: ast.AST, state_id: int, scope: _Scope
    ) -> tuple[int, EffectMask]:
        result = self._delete_target(target, state_id, scope)
        key = self._node_key(target)
        self.transfer.assignment_effects[key] = (
            self.transfer.assignment_effects.get(key, NO_EFFECTS) | result[1]
        )
        scope.module_metadata_effects |= result[1] & WRITES_MODULE_METADATA
        return result

    def _delete_target(
        self, target: ast.AST, state_id: int, scope: _Scope
    ) -> tuple[int, EffectMask]:
        if isinstance(target, ast.Name):
            identities, _value, _result, invalidated, bound, _owner = (
                self._resolve_name(state_id, scope, target.id)
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

    def _import_provider_is_canonical(self, module: str) -> bool:
        return not any(
            module == excluded or module.startswith(excluded + ".")
            for excluded in self.policy.noncanonical_imports
        )

    def _import_identity(self, module: str, state_id: int) -> IdentityMask:
        identity = (
            PythonIdentity.INTRINSICS_MODULE
            if is_compiler_intrinsics_module(module)
            else CANONICAL_IMPORT_IDENTITIES.get(module)
        )
        if identity is None:
            return OTHER_IDENTITY
        # A redirect writer may replace the provider, so the import yields the
        # canonical module or another object: never exact, still possible.
        return (
            exact_identity(identity)
            if self._import_provider_is_canonical(module)
            and self._canonical_imports_available(state_id)
            else possible_identity(identity)
        )

    def _from_import_identity(
        self, module: str | None, name: str, state_id: int
    ) -> IdentityMask:
        # Import callbacks and earlier member replacement constrain acquisition
        # exactly as they constrain a module.attribute read. Do not resurrect a
        # canonical member merely because its module was imported again.
        if module is None or not self._import_provider_is_canonical(f"{module}.{name}"):
            return OTHER_IDENTITY
        return self._member_value(
            state_id, self._import_identity(module, state_id), name
        )

    def _bind_name(
        self,
        name: str,
        value: IdentityMask,
        state_id: int,
        scope: _Scope,
        *,
        store_can_fail: bool = True,
    ) -> tuple[int, EffectMask]:
        return self._write_name(
            state_id, scope, name, value, store_can_fail=store_can_fail
        )

    def _write_name(
        self,
        state_id: int,
        scope: _Scope,
        name: str,
        value: IdentityMask,
        static_value: PythonStaticValue = None,
        result: StaticExpressionResult = UNKNOWN_EXPRESSION_RESULT,
        owner_token: int = 0,
        *,
        store_can_fail: bool = True,
    ) -> tuple[int, EffectMask]:
        slot = self._slot_for_name(scope, name)
        if (
            self._active_module_states is None
            and slot is not None
            and self.module_slot_mask & (1 << slot)
        ):
            # Another importer may observe a publication before a later overwrite
            # or deletion. Deferred function writes are admitted by body demand.
            if result.exposed_source_origins:
                namespace = self.transfer.source_namespaces.setdefault(
                    self._module_source, {}
                )
                namespace[name] = (
                    namespace.get(name, frozenset()) | result.exposed_source_origins
                )
            if name == "__getattr__":
                self._module_getattr_may_be_bound = True
        if (
            scope.kind == "class"
            and name not in scope.globals
            and name not in scope.nonlocals
        ):
            # Namespace publication is the value authority, including aliases,
            # descriptors and conditional assignments, not just method syntax.
            # A custom mapping can discard or replace the value, so this is
            # candidate availability only. Preserve its named publication even
            # when the storage/type authority must forget the selected value.
            namespace = self.transfer.source_namespaces.setdefault(scope.source, {})
            namespace[name] = (
                namespace.get(name, frozenset()) | result.exposed_source_origins
            )
            scope.escaped_deferred |= result.exposed_deferred
            scope.escaped_source_origins |= result.exposed_source_origins
            if result.exposes_module_globals:
                scope.escaped_metadata_effects |= WRITES_MODULE_METADATA
        if self._namespace_can_call(state_id, scope, name, write=True):
            self._record_operation_execution(
                scope,
                frozenset(
                    DeferredExecution(ref.source, "escape")
                    for ref in result.exposed_deferred
                )
                | frozenset(
                    DeferredExecution(source, "escape")
                    for source in result.exposed_source_origins
                ),
                module_metadata_effects=(
                    WRITES_MODULE_METADATA
                    if result.exposes_module_globals
                    else NO_EFFECTS
                ),
            )
            if slot is not None:
                state_id = self.states.set_binding(
                    state_id, slot, OTHER_IDENTITY | UNBOUND_IDENTITY
                )
            effects = WRITES_OBJECT_STATE | EXECUTES_ARBITRARY_PYTHON | RAISES
            return self._apply_effects(state_id, effects), effects
        if slot is None:
            return state_id, NO_EFFECTS
        if (
            store_can_fail
            and scope.kind in _FRAME_LOCAL_SCOPE_KINDS
            and name in scope.locals
            and name not in scope.globals
            and name not in scope.nonlocals
        ):
            # Molt binds a frame local through FRAME_HOME_STORE. Its boxed view
            # of a raw integer may allocate, so op_kinds.toml declares the store
            # raising. The allocation precedes publication: a failed store
            # leaves this state, old binding included. A handler or a
            # suppressing __exit__ that resumes there must see the name as
            # possibly unbound: `with ... as name` binds first in its protected
            # region, and `try: name = other_local` raises nowhere else.
            self._note_failed_store(state_id)
        state_id, effects = self._replace_binding(
            state_id, slot, value, static_value, result, owner_token, scope=scope
        )
        if name in MODULE_IMPORT_METADATA_NAMES and self.module_slot_mask & (1 << slot):
            effects |= WRITES_MODULE_METADATA | WRITES_GLOBAL_NAMESPACE
            scope.module_metadata_effects |= WRITES_MODULE_METADATA
        return state_id, effects

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
            self._observed_stack[-1].exceptional_state(self.states)
            if effects & RAISES
            else None
        )
        failed_store = self._take_failed_store()
        if failed_store is not None:
            effects |= RAISES
            exceptional = (
                failed_store
                if exceptional is None
                else self.states.join(exceptional, failed_store)
            )
        return PythonCompletionFlow(
            normal=state_id, raised=exceptional, effects=effects
        )

    def _note_failed_store(self, state_id: int) -> None:
        frame = self._observed_stack[-1]
        frame.failed_store = (
            state_id
            if frame.failed_store is None
            else self.states.join(frame.failed_store, state_id)
        )

    def _take_failed_store(self) -> int | None:
        if not self._observed_stack:
            return None
        frame = self._observed_stack[-1]
        failed_store, frame.failed_store = frame.failed_store, None
        return failed_store

    def _raise_unrouted_failed_store(
        self, flow: PythonCompletionFlow[int], observation: _ObservedStateFrame
    ) -> PythonCompletionFlow[int]:
        # A store that no flow inside the statement routed fails the statement.
        if observation.failed_store is None:
            return flow
        return self._merge_flows(
            flow,
            PythonCompletionFlow(raised=observation.failed_store, effects=RAISES),
        )

    def exec_statements(
        self,
        body: Sequence[ast.stmt],
        state_id: int,
        scope: _Scope,
        *,
        continuation: _InitializerContinuation | None = None,
        capture: Callable[[PythonCompletionFlow[int]], None] | None = None,
    ) -> PythonCompletionFlow[int]:
        # Python represents an elif chain as nested If nodes. Schedule both
        # branches and their statement sequences explicitly: source depth must
        # not become Python evaluator call depth. Other compound statements
        # retain their own completion semantics and enter this same scheduler.
        frames: list[_StatementSequenceFrame | _ConditionalStatementFrame] = [
            _StatementSequenceFrame(
                body,
                PythonCompletionFlow(normal=state_id)
                if continuation is None
                else continuation.completion,
                0 if continuation is None else 1,
            )
        ]
        observation_depth = len(self._observed_stack)
        try:
            while frames:
                frame = frames[-1]
                if isinstance(frame, _StatementSequenceFrame):
                    if frame.flow.normal is not None and frame.position < len(
                        frame.body
                    ):
                        if (
                            capture is not None
                            and frame.position == 1
                            and len(frames) == 1
                        ):
                            capture(frame.flow)
                        node = frame.body[frame.position]
                        frame.position += 1
                        incoming = frame.flow.normal
                        observation_before = self._namespace_observation_epoch
                        self._observed_stack.append(_ObservedStateFrame([incoming]))
                        if isinstance(node, ast.If):
                            test = self._eval_truth_test(node.test, incoming, scope)
                            truth = self._known_expression_result(node.test).truth
                            if truth is None and test.identities == int(
                                PythonIdentity.STATIC_FALSE
                            ):
                                truth = False
                            # Capture test failures before either branch can
                            # contribute states to the enclosing observation.
                            prefix = self._normal_flow(test.state_id, test.effects)
                            branches = (
                                (node.body if truth else node.orelse,)
                                if truth is not None
                                else (node.body, node.orelse)
                            )
                            frames.append(
                                _ConditionalStatementFrame(
                                    node, prefix, branches, observation_before
                                )
                            )
                            continue
                        metadata_at_entry = (
                            self._module_metadata_borrowing_snapshot(incoming, scope)
                            if isinstance(node, ast.ImportFrom) and node.level
                            else NO_MODULE_METADATA_PROOF
                        )
                        try:
                            flow = self._exec_statement(node, incoming, scope)
                        finally:
                            observation = self._observed_stack.pop()
                        flow = self._raise_unrouted_failed_store(flow, observation)
                        self._record_statement_flow(
                            node,
                            scope,
                            flow,
                            observation_before,
                            metadata_at_entry,
                            executed_deferred=observation.deferred_execution,
                            executed_metadata_effects=observation.module_metadata_effects,
                        )
                        frame.flow = self._merge_flows(
                            frame.flow.without_normal(), flow
                        )
                        continue
                    flow = frame.flow
                else:
                    if frame.prefix.normal is not None and frame.position < len(
                        frame.branches
                    ):
                        branch = frame.branches[frame.position]
                        frame.position += 1
                        # Alternatives share the test's normal input, never
                        # the preceding alternative's output state.
                        frames.append(
                            _StatementSequenceFrame(
                                branch,
                                PythonCompletionFlow(normal=frame.prefix.normal),
                            )
                        )
                        continue
                    flow = self._merge_flows(frame.prefix.without_normal(), frame.flow)
                    observation = self._observed_stack.pop()
                    flow = self._raise_unrouted_failed_store(flow, observation)
                    self._record_statement_flow(
                        frame.node,
                        scope,
                        flow,
                        frame.observation_before,
                        executed_deferred=observation.deferred_execution,
                        executed_metadata_effects=observation.module_metadata_effects,
                    )
                frames.pop()
                if not frames:
                    return flow
                parent = frames[-1]
                if isinstance(parent, _StatementSequenceFrame):
                    parent.flow = self._merge_flows(parent.flow.without_normal(), flow)
                else:
                    parent.flow = self._merge_flows(parent.flow, flow)
        finally:
            # An exception in a test, branch, or consumer must not leak any
            # suspended conditional observations into the enclosing analysis.
            del self._observed_stack[observation_depth:]
        raise AssertionError("statement scheduler lost its root completion")

    def _record_statement_flow(
        self,
        node: ast.stmt,
        scope: _Scope,
        flow: PythonCompletionFlow[int],
        observation_before: int,
        module_metadata: PythonModuleMetadataProof = NO_MODULE_METADATA_PROOF,
        *,
        executed_deferred: frozenset[DeferredExecution] = frozenset(),
        executed_metadata_effects: EffectMask = NO_EFFECTS,
    ) -> None:
        if not flow.completions & PythonCompletion.NORMAL:
            self._module_import_flow_required = True
        key = self._node_key(node)
        previous = self.transfer.statements.get(key)
        deferred_execution = executed_deferred
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            for decorator in node.decorator_list:
                deferred_execution |= self._known_expression_result(
                    decorator
                ).exposed_deferred
        elif isinstance(node, ast.AugAssign):
            scope.module_metadata_effects |= (
                self.transfer.assignment_effects.get(
                    self._node_key(node.target), NO_EFFECTS
                )
                & WRITES_MODULE_METADATA
            )
        # Implicit context protocol calls are projected at their own execution
        # phases. Escaping the receiver here would execute unrelated methods
        # and erase the distinction between entry, binding and exit handlers.
        scope.deferred_execution |= deferred_execution
        self.transfer.statements[key] = PythonStatementFact(
            key,
            scope.scope_id,
            flow.effects | (previous.effects if previous is not None else NO_EFFECTS),
            self._namespace_observation_epoch != observation_before
            or (previous is not None and previous.module_namespace_observable),
            flow.completions
            | (previous.completions if previous is not None else PythonCompletion.NONE),
            self.transfer.iterations.get(key),
            module_metadata
            if previous is None
            else module_metadata.intersect(previous.module_metadata_at_entry),
            deferred_execution
            | (previous.deferred_execution if previous is not None else frozenset()),
            executed_metadata_effects
            | (
                previous.module_metadata_effects if previous is not None else NO_EFFECTS
            ),
        )

    def _exec_statement(
        self, node: ast.stmt, state_id: int, scope: _Scope
    ) -> PythonCompletionFlow[int]:
        self._binding_read_scopes.append({})
        try:
            return self._exec_statement_with_reads(node, state_id, scope)
        finally:
            self._binding_read_scopes.pop()

    def _exec_statement_with_reads(
        self, node: ast.stmt, state_id: int, scope: _Scope
    ) -> PythonCompletionFlow[int]:
        effects = NO_EFFECTS
        if isinstance(
            node, (ast.For, ast.AsyncFor, ast.With, ast.AsyncWith, ast.Match)
        ):
            self._module_import_flow_required = True
        elif isinstance(node, (ast.Import, ast.ImportFrom)) and any(
            alias.name == "*"
            or python_import_binding(alias) in MODULE_IMPORT_METADATA_NAMES
            for alias in node.names
        ):
            # Import bindings can replace the relative-import anchor just as
            # assignments do. Do not elide the source-ordered flow projection.
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
            if _release_may_call(result.identities, result.result):
                effects |= _RELEASE_CALLBACK_EFFECTS
                self._record_operation_execution(
                    scope,
                    _operand_dispatch_deferred(
                        _RELEASE_CALLBACK_EFFECTS, result.result, reaches_contents=True
                    ),
                )
                state_id = self._apply_effects(state_id, _RELEASE_CALLBACK_EFFECTS)
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
                    result=result.result,
                    owner_token=result.owner_token,
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
                    result=result.result,
                    owner_token=result.owner_token,
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
            retained_target = _result_after_retained_boundary(
                target_read.result, value.effects
            )
            operation_effects = binary_operation_effects(
                retained_target, value.result, inplace=True
            )
            if isinstance(node.op, ast.BitOr):
                self._project_mapping_write(
                    scope, node.target, target_read.result.source_origins, None
                )
            namespace_effects = NO_EFFECTS
            if target_read.result.exposes_module_globals:
                # __iop__ receives the retained object before the name/member is
                # rebound. A globals alias (including an exposing aggregate) can
                # mutate metadata even when its rooted retirement is harmless.
                namespace_effects = WRITES_GLOBAL_NAMESPACE | WRITES_MODULE_METADATA
                operation_effects |= namespace_effects
            self._record_operation_execution(
                scope,
                _operand_dispatch_deferred(
                    operation_effects,
                    retained_target,
                    value.result,
                    reaches_contents=True,
                ),
            )
            effects |= target_read.effects | value.effects | operation_effects
            state_id = self._apply_effects(value.state_id, operation_effects)
            result = static_binary_result(retained_target, node.op, value.result)
            static_value = (
                cast(str | int, result.value)
                if result.value_known and type(result.value) in {str, int}
                else None
            )
            if target_read.assignment_target is not None:
                state_id, target_effects = self._store_evaluated_member(
                    target_read.assignment_target,
                    state_id,
                    scope,
                    value=result.identities,
                    static_value=static_value,
                    result=result,
                )
            else:
                state_id, target_effects = self.assign_target(
                    node.target,
                    result.identities,
                    state_id,
                    scope,
                    static_value=static_value,
                    result=result,
                )
            # Operator callbacks belong to statement evaluation, not target
            # publication. Transport only actual namespace mutation together
            # with the store/release effects for names and retained members.
            target_key = self._node_key(node.target)
            self.transfer.assignment_effects[target_key] = (
                self.transfer.assignment_effects.get(target_key, NO_EFFECTS)
                | namespace_effects
                | target_effects
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
            canonical_statement = (
                canonical_before_import
                and all(
                    module in CANONICAL_IMPORT_IDENTITIES for module in identity_modules
                )
                and all(
                    self._import_provider_is_canonical(alias.name)
                    for alias in node.names
                )
            )
            if not canonical_statement:
                state_id = self._apply_effects(state_id, effects)
                state_id = self.states.invalidate_members(
                    state_id, self.policy.callback_member_writes
                )
            for alias in node.names:
                bound = python_import_binding(alias)
                identity_module = (
                    alias.name if alias.asname else alias.name.split(".", 1)[0]
                )
                state_id, bind_effects = self._write_name(
                    state_id,
                    scope,
                    bound,
                    self._import_identity(identity_module, state_id),
                    result=StaticExpressionResult(
                        source_origins=frozenset({self._node_key(alias)})
                    ),
                )
                effects |= bind_effects
        elif isinstance(node, ast.ImportFrom):
            # A star import of the synthetic module names nothing the analysis
            # can enumerate: it takes the generic IMPORT_STAR path below, and
            # the frontend owns its placement diagnostic.
            if (
                node.level == 0
                and is_compiler_intrinsics_module(node.module)
                and all(alias.name != "*" for alias in node.names)
            ):
                for alias in node.names:
                    helper = COMPILER_INTRINSIC_CALLABLES.get(alias.name)
                    if helper is not None:
                        self._record_state(state_id)
                        effects |= ALLOCATES | RAISES
                    state_id, bind_effects = self._write_name(
                        state_id,
                        scope,
                        alias.asname or alias.name,
                        int(helper[0])
                        if helper is not None
                        else int(PythonIdentity.INERT_VALUE),
                        result=StaticExpressionResult.scalar(None)
                        if helper is None
                        else StaticExpressionResult(),
                    )
                    effects |= bind_effects
                return self._normal_flow(state_id, effects)
            canonical_before_import = self._canonical_imports_available(state_id)
            effects |= EXECUTES_ARBITRARY_PYTHON | INVOKES_IMPORT_SYSTEM | RAISES
            canonical_statement = (
                canonical_before_import
                and node.level == 0
                and node.module is not None
                and self._import_provider_is_canonical(node.module)
                and all(
                    alias.name != "*"
                    and self._import_provider_is_canonical(
                        f"{node.module}.{alias.name}"
                    )
                    and (
                        node.module in CANONICAL_IMPORT_IDENTITIES
                        or f"{node.module}.{alias.name}" in CANONICAL_IMPORT_IDENTITIES
                    )
                    for alias in node.names
                )
            )
            if not canonical_statement:
                state_id = self._apply_effects(state_id, effects)
                state_id = self.states.invalidate_members(
                    state_id, self.policy.callback_member_writes
                )
            if any(alias.name == "*" for alias in node.names):
                # IMPORT_STAR writes this module now, independently of which
                # globals later callbacks are allowed to replace.
                self._namespace_observation_epoch += 1
                state_id = self._taint_module_slots(state_id)
            for alias in node.names:
                if alias.name == "*":
                    continue
                state_id, bind_effects = self._write_name(
                    state_id,
                    scope,
                    alias.asname or alias.name,
                    self._from_import_identity(
                        node.module if node.level == 0 else None,
                        alias.name,
                        state_id,
                    ),
                    result=StaticExpressionResult(
                        source_origins=frozenset({self._node_key(alias)})
                    ),
                )
                effects |= bind_effects
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
                branch = unmatched
                pattern_results = [subject.result]
                pending_patterns = [case.pattern]
                while pending_patterns:
                    pattern = pending_patterns.pop()
                    if isinstance(pattern, ast.MatchClass):
                        evaluated = self.eval_expr(pattern.cls, branch, scope)
                        branch = evaluated.state_id
                        pattern_effects |= evaluated.effects
                        pattern_results.append(evaluated.result)
                    elif isinstance(pattern, ast.MatchValue):
                        evaluated = self.eval_expr(pattern.value, branch, scope)
                        branch = evaluated.state_id
                        pattern_effects |= evaluated.effects
                        pattern_results.append(evaluated.result)
                    elif isinstance(pattern, ast.MatchMapping):
                        for expression in pattern.keys:
                            evaluated = self.eval_expr(expression, branch, scope)
                            branch = evaluated.state_id
                            pattern_effects |= evaluated.effects
                            pattern_results.append(evaluated.result)
                    pending_patterns.extend(
                        child
                        for child in ast.iter_child_nodes(pattern)
                        if isinstance(child, ast.pattern)
                    )
                self._record_operation_execution(
                    scope,
                    _operand_dispatch_deferred(
                        NO_EFFECTS if inert_capture else INVOKES_COMPARISON_CALLBACK,
                        *pattern_results,
                        reaches_contents=True,
                    ),
                )
                branch = self._apply_effects(branch, pattern_effects)
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
            decorators = []
            for decorator in node.decorator_list:
                result = self.eval_expr(decorator, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
                decorators.append((decorator, result.result))
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
            function_result = self._deferred_function_result(node)
            for expression, constructor in reversed(decorators):
                self._record_state(state_id)
                wrapped = self._fresh_function_wrapper_result(
                    constructor,
                    function_result,
                    node,
                    expression,
                    scope,
                    defaults=defaults,
                )
                if wrapped is not None:
                    effects |= ALLOCATES | RAISES
                    function_result = wrapped
                else:
                    decorator_effects = EXECUTES_ARBITRARY_PYTHON | RAISES
                    effects |= decorator_effects
                    self._record_operation_execution(
                        scope,
                        frozenset(
                            DeferredExecution(origin, "escape")
                            for origin in function_result.exposed_source_origins
                        ),
                    )
                    state_id = self._apply_effects(state_id, decorator_effects)
                    function_result = replace(
                        self._apply_source_decorator(
                            expression, constructor, function_result, scope
                        ),
                        identities=function_result.identities | OTHER_IDENTITY,
                        deferred_complete=False,
                        attribute_hooks=None,
                    )
            state_id, bind_effects = self._write_name(
                state_id,
                scope,
                node.name,
                function_result.identities,
                result=function_result,
            )
            effects |= bind_effects
            self._queue_function(node, definition_scope, state_id, defaults)
        elif isinstance(node, ast.ClassDef):
            decorators = []
            for expression in node.decorator_list:
                result = self.eval_expr(expression, state_id, scope)
                state_id = result.state_id
                effects |= result.effects
                decorators.append((expression, result.result))
            # LOAD_BUILD_CLASS is after decorator evaluation and before bases
            # and keywords. It bypasses globals and the class-local mapping.
            builder = self._guarded_member_identity(
                state_id,
                int(PythonIdentity.BUILTINS_MODULE),
                PythonIdentity.BUILTINS_MODULE,
                PythonIdentity.BUILTIN_BUILD_CLASS,
                PythonMember.BUILTINS_BUILD_CLASS,
            )
            custom_builtins_possible = (
                self._builtins_mapping_rebindable()
                or not scope.activation_namespace_stable
            )
            if custom_builtins_possible:
                builder |= OTHER_IDENTITY
            builder_key = self._node_key(node)
            self.transfer.class_builder_reads[builder_key] = (
                self.transfer.class_builder_reads.get(builder_key, NO_IDENTITIES)
                | builder
            )
            if builder != int(PythonIdentity.BUILTIN_BUILD_CLASS):
                # A custom builtins mapping can run __getitem__ at this read.
                lookup_effects = RAISES | (
                    EXECUTES_ARBITRARY_PYTHON
                    if custom_builtins_possible
                    else NO_EFFECTS
                )
                effects |= lookup_effects
                state_id = self._apply_effects(state_id, lookup_effects)
            state_id, definition_scope = self._type_parameter_scope(
                node, state_id, scope
            )
            (
                state_id,
                argument_effects,
                argument_results,
                _argument_crossed_boundaries,
            ) = self._eval_call_arguments(node, state_id, definition_scope)
            effects |= argument_effects
            owner_key = self._node_key(node)
            base_results = tuple(
                self._source_element_result(
                    self._node_key(base),
                    self._known_expression_result(base.value),
                    PythonIterationFact.from_result(
                        self._known_expression_result(base.value), NO_EFFECTS
                    ).element_result,
                    protocol=PythonElementProtocol.ITERATE,
                )
                if isinstance(base, ast.Starred)
                else self._known_expression_result(base)
                for base in node.bases
            )
            self.transfer.source_bases[owner_key] = self.transfer.source_bases.get(
                owner_key, frozenset()
            ) | frozenset().union(*(base.source_origins for base in base_results))
            assert self._dependency_authority is not None
            declarations = self._dependency_authority.declarations(node)
            class_scope = self._new_scope(
                parent=definition_scope,
                source_node=node,
                kind="class",
                name=node.name,
                declarations=declarations,
            )
            # An explicit, proven builtin object selects the same native
            # metaclass and namespace preparation as the implicit root.
            native_root = not node.bases or (
                len(node.bases) == 1
                and not isinstance(node.bases[0], ast.Starred)
                and base_results[0].identities == int(PythonIdentity.BUILTIN_OBJECT)
            )
            # Molt rejects a replacement builder before reaching the body.
            native_root_prepare = (
                self.policy.class_preparation is PythonClassPreparation.MOLT_NATIVE
                and scope.activation_namespace_stable
                and native_root
                and not node.keywords
            )
            # An already evaluated class with exact metaclass type cannot
            # change that metaclass. type.__prepare__ allocates a fresh dict.
            # An inherited __init_subclass__ runs after the class body.
            plain_preparation = native_root_prepare or (
                builder == int(PythonIdentity.BUILTIN_BUILD_CLASS)
                and not node.keywords
                and all(
                    not isinstance(base, ast.Starred)
                    and (
                        result.identities == int(PythonIdentity.BUILTIN_OBJECT)
                        or self._plain_class_value(base) is not None
                    )
                    for base, result in zip(node.bases, base_results, strict=True)
                )
            )
            default_init_subclass = plain_preparation and all(
                result.identities == int(PythonIdentity.BUILTIN_OBJECT)
                or result.class_call_proofs & ClassCallProof.DEFAULT_INIT_SUBCLASS
                for result in argument_results
            )
            class_scope.dynamic_class_namespace |= not plain_preparation
            class_scope.escaped_deferred |= frozenset().union(
                *(result.exposed_deferred for result in argument_results)
            )
            # Each execution creates a fresh namespace, despite sharing source
            # slots/facts with other visits to this class definition.
            state_id = self.states.set_bindings(
                state_id,
                tuple(
                    (slot, UNBOUND_IDENTITY, None, UNKNOWN_EXPRESSION_RESULT, 0)
                    for slot in class_scope.slots.values()
                )
                + (
                    (
                        (
                            self._native_class_namespace_slot(class_scope),
                            int(PythonIdentity.INERT_VALUE),
                            None,
                            StaticExpressionResult.scalar(True),
                            0,
                        ),
                    )
                    if plain_preparation
                    else ()
                ),
            )
            if not plain_preparation:
                # __mro_entries__, metaclass selection and __prepare__ all run
                # before body lookup; custom namespace initialization can call
                # Python before the first user statement as well.
                preparation_effects = EXECUTES_ARBITRARY_PYTHON | RAISES
                self._record_operation_execution(
                    scope,
                    frozenset(
                        DeferredExecution(source, "escape")
                        for result in argument_results
                        for source in result.exposed_source_origins
                        | {ref.source for ref in result.exposed_deferred}
                    ),
                )
                if any(result.exposes_module_globals for result in argument_results):
                    self._record_operation_execution(
                        scope,
                        frozenset(),
                        module_metadata_effects=WRITES_MODULE_METADATA,
                    )
                effects |= preparation_effects
                state_id = self._apply_effects(state_id, preparation_effects)
            prefix = self._normal_flow(state_id, effects)
            class_flow = self.exec_statements(node.body, state_id, class_scope)
            scope.module_metadata_effects |= class_scope.module_metadata_effects
            scope.deferred_execution |= class_scope.deferred_execution
            flow = self._merge_flows(
                prefix.without_normal(), class_flow.without_normal()
            )
            if class_flow.normal is not None:
                class_state = class_flow.normal
                if node.decorator_list:
                    self._record_decoration_annotation_heads(
                        node, class_state, scope, class_scope
                    )
                # Body failure never creates or binds a class. Plain creation
                # remains a projection of the successful namespace state.
                if not native_root:
                    default_init_subclass &= not (
                        class_flow.effects & _CALLEE_ELISION_FORBIDDEN_EFFECTS
                    )
                plain_creation = (
                    default_init_subclass
                    and not node.decorator_list
                    and not self._namespace_can_call(class_state, class_scope, "")
                    and all(
                        self.states.binding(class_state, slot)
                        in {
                            UNBOUND_IDENTITY,
                            int(PythonIdentity.INERT_VALUE),
                            int(PythonIdentity.STATIC_FALSE),
                            int(PythonIdentity.USER_FUNCTION),
                            int(PythonIdentity.RETAINED_CALLABLE_WRAPPER),
                        }
                        for slot in class_scope.slots.values()
                    )
                )
                namespace = {
                    name: self.states.result(class_state, slot)
                    for name, slot in class_scope.slots.items()
                    if self.states.binding(class_state, slot) != UNBOUND_IDENTITY
                }
                owner_key = self._node_key(node)
                plain_attributes = tuple(
                    sorted(
                        name
                        for name, value in namespace.items()
                        if value.kind
                        in {
                            "none",
                            "bool",
                            "int",
                            "float",
                            "complex",
                            "str",
                            "bytes",
                            "tuple",
                            "list",
                            "dict",
                            "set",
                            "frozenset",
                        }
                        or value.identities
                        in {
                            int(PythonIdentity.USER_FUNCTION),
                            int(PythonIdentity.RETAINED_CALLABLE_WRAPPER),
                        }
                        # A property is a data descriptor: an instance store of
                        # its name runs its setter.
                        and not any(
                            (element := self.transfer.source_elements.get(origin))
                            is not None
                            and element.protocol is PythonElementProtocol.PROPERTY
                            for origin in value.source_origins
                        )
                    )
                )
                prior_class = self.transfer.source_classes.get(owner_key)
                self.transfer.source_classes[owner_key] = PythonSourceClass(
                    owner_key,
                    plain_preparation
                    # Preparation describes the namespace. A replaced
                    # builder or decorator may publish a different class.
                    and builder == int(PythonIdentity.BUILTIN_BUILD_CLASS)
                    and not node.decorator_list
                    and (prior_class is None or prior_class.plain_metaclass),
                    tuple(
                        name
                        for name in plain_attributes
                        if prior_class is None or name in prior_class.plain_attributes
                    ),
                )
                source_namespace = self.transfer.source_namespaces.setdefault(
                    owner_key, {}
                )
                for name, result in namespace.items():
                    source_namespace[name] = (
                        source_namespace.get(name, frozenset())
                        | result.exposed_source_origins
                    )
                exposure = class_scope.escaped_source_origins
                if not plain_preparation or builder != int(
                    PythonIdentity.BUILTIN_BUILD_CLASS
                ):
                    # Arbitrary class construction may publish its inputs
                    # under unknown names. Plain bases have a separate edge.
                    exposure |= frozenset().union(
                        *(value.exposed_source_origins for value in argument_results)
                    )
                self.transfer.source_namespace_exposure[owner_key] = (
                    self.transfer.source_namespace_exposure.get(owner_key, frozenset())
                    | exposure
                )
                namespace_values = tuple(namespace.values())
                namespace_refs = frozenset().union(
                    *(result.exposed_deferred for result in namespace_values),
                    class_scope.escaped_deferred,
                )
                class_scope.escaped_deferred |= namespace_refs
                # A normal stored function uses Python's inert binding
                # descriptor. Other namespace objects may have __get__ and
                # __set_name__; preserve those values' actual provenance.
                descriptors = tuple(
                    result
                    for result in namespace_values
                    if not result.deferred_complete
                    or not result.identities
                    & int(
                        PythonIdentity.USER_FUNCTION
                        | PythonIdentity.RETAINED_CALLABLE_WRAPPER
                    )
                )
                descriptor_refs = frozenset(
                    DeferredExecution(ref.source, "escape")
                    for result in descriptors
                    for ref in result.exposed_deferred
                ) | frozenset(
                    DeferredExecution(ref.source, "escape")
                    for ref in namespace_refs
                    if ref.phase not in {"call", "factory"}
                )
                unknown_descriptor = any(
                    result.kind == "unknown" and not result.deferred_complete
                    for result in descriptors
                )
                # An instance load of a `property` member runs the property's
                # `__get__` hook, which calls its getter (the source demands
                # turn this descriptor phase into the getter's call).
                property_getters = frozenset(
                    DeferredExecution(owner, "descriptor")
                    for result in namespace_values
                    for owner in result.source_origins
                    if (element := self.transfer.source_elements.get(owner)) is not None
                    and element.protocol is PythonElementProtocol.PROPERTY
                )
                inherited_hooks = tuple(
                    result.instance_attribute_hooks for result in argument_results
                )
                hooks_known = (
                    not node.keywords
                    and not node.decorator_list
                    and not unknown_descriptor
                    and all(hooks is not None for hooks in inherited_hooks)
                )
                instance_hooks = (
                    descriptor_refs
                    | property_getters
                    | frozenset().union(
                        *(hooks for hooks in inherited_hooks if hooks is not None)
                    )
                    | frozenset().union(
                        *(
                            namespace[name].deferred
                            for name in ("__getattribute__", "__getattr__")
                            if name in namespace
                        )
                    )
                    if hooks_known
                    else None
                )
                class_hooks = descriptor_refs if hooks_known else None
                creation_effects = ALLOCATES | RAISES
                if not plain_creation:
                    creation_effects |= EXECUTES_ARBITRARY_PYTHON
                    self._record_operation_execution(scope, descriptor_refs)
                    if plain_preparation and not default_init_subclass:
                        self._record_operation_execution(
                            scope,
                            frozenset({DeferredExecution(owner_key, "init_subclass")}),
                        )
                    if any(result.exposes_module_globals for result in descriptors):
                        self._record_operation_execution(
                            scope,
                            frozenset(),
                            module_metadata_effects=WRITES_MODULE_METADATA,
                        )
                class_state = self._apply_effects(class_state, creation_effects)
                constructors = frozenset().union(
                    *(
                        namespace[name].deferred
                        for name in ("__new__", "__init__")
                        if name in namespace
                    ),
                    *(result.deferred for result in argument_results),
                )
                if class_scope.dynamic_class_namespace:
                    # A custom preparation mapping can replace constructor
                    # bindings while preserving source-owned alternatives.
                    constructors |= frozenset(
                        {DeferredExecution(self._node_key(node), "escape")}
                    )
                class_call_proofs = ClassCallProof.NONE
                if plain_creation:
                    if not constructors:
                        class_call_proofs |= ClassCallProof.DEFAULT_CONSTRUCTOR
                    if "__init_subclass__" not in namespace:
                        class_call_proofs |= ClassCallProof.DEFAULT_INIT_SUBCLASS
                class_result = StaticExpressionResult(
                    identities=possible_identity(PythonIdentity.USER_CLASS),
                    source_origins=frozenset({self._node_key(node)}),
                    deferred=constructors
                    | frozenset({DeferredExecution(self._node_key(node), "class")}),
                    exposed_deferred=namespace_refs
                    | frozenset().union(
                        *(result.exposed_deferred for result in argument_results)
                    ),
                    exposes_module_globals=any(
                        result.exposes_module_globals
                        for result in (*namespace_values, *argument_results)
                    ),
                    deferred_complete=not node.decorator_list
                    and all(result.deferred_complete for result in argument_results),
                    attribute_hooks=class_hooks,
                    instance_attribute_hooks=instance_hooks,
                    class_call_proofs=class_call_proofs,
                )
                for expression, decorator in reversed(decorators):
                    class_result = self._apply_source_decorator(
                        expression, decorator, class_result, scope
                    )
                class_state, bind_effects = self._write_name(
                    class_state,
                    scope,
                    node.name,
                    class_result.identities,
                    result=class_result,
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
                state_id, bind_effects = self._write_name(
                    state_id,
                    scope,
                    node.name.id,
                    exact_identity(PythonIdentity.INERT_VALUE),
                    result=StaticExpressionResult(
                        source_origins=frozenset({self._node_key(node)})
                    ),
                )
                effects |= bind_effects
            effects |= ALLOCATES
        elif isinstance(node, ast.Return):
            scope.returns_calls_only &= isinstance(node.value, ast.Call)
            if node.value is not None:
                result = self.eval_expr(node.value, state_id, scope)
                state_id, effects = result.state_id, result.effects
                scope.escaped_deferred |= result.result.exposed_deferred
                scope.returned_source_origins |= result.result.source_origins
                scope.returns_calls_only &= result.result.source_origins == {
                    self._node_key(node.value)
                }
                scope.returned_exposed_source_origins |= (
                    result.result.exposed_source_origins
                )
                if result.result.exposes_module_globals:
                    scope.escaped_metadata_effects |= WRITES_MODULE_METADATA
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
                self._record_operation_execution(
                    scope,
                    _operand_dispatch_deferred(
                        EXECUTES_ARBITRARY_PYTHON,
                        *(
                            self._known_expression_result(value)
                            for value in (node.exc, node.cause)
                            if value is not None
                        ),
                        reaches_contents=True,
                    ),
                )
                effects |= EXECUTES_ARBITRARY_PYTHON
                state_id = self._apply_effects(
                    state_id, EXECUTES_ARBITRARY_PYTHON | RAISES
                )
            self._record_state(state_id)
            return PythonCompletionFlow(
                raised=self._observed_stack[-1].exceptional_state(self.states),
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
        iterable_result = UNKNOWN_EXPRESSION_RESULT
        retained_boundary = NO_EFFECTS
        iteration_key: PythonSourceKey | None = None
        if isinstance(node, (ast.For, ast.AsyncFor)):
            iterable = self.eval_expr(node.iter, entry, scope)
            entry = iterable.state_id
            iterable_result = self._known_expression_result(node.iter)
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
            iteration = PythonIterationFact.from_result(
                iterable_result,
                iteration_effects,
                asynchronous=isinstance(node, ast.AsyncFor),
            )
            iteration = replace(
                iteration,
                element_result=self._source_element_result(
                    self._node_key(node.target),
                    iterable_result,
                    iteration.element_result,
                    protocol=PythonElementProtocol.ASYNC_ITERATE
                    if isinstance(node, ast.AsyncFor)
                    else PythonElementProtocol.ITERATE,
                ),
            )
            scope.deferred_execution |= iteration.deferred_execution
            iteration_key = self._node_key(node)
            entry = self._apply_effects(entry, iteration.effects)
            prefix = self._normal_flow(
                entry, iterable.effects | iteration.effects
            ).without_normal()
        entry_epoch = self.states.get(entry).taint_epoch

        def advance(header: int) -> tuple[int | None, PythonCompletionFlow[int]]:
            nonlocal retained_boundary
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
                element_result = iteration.element_result
                tainted = self.states.get(body_entry).taint_epoch != entry_epoch
                if tainted or retained_boundary & WRITES_OBJECT_STATE:
                    if iteration.mutable and (
                        tainted or not _owned_iterable_display(node.iter)
                    ):
                        strings = None
                        element_result = expression_result_without_value_facts(
                            element_result
                        )
                    else:
                        element_result = expression_result_without_mutable_contents(
                            element_result
                        )
                body_entry, target_effects = self.assign_target(
                    node.target,
                    element_result.identities,
                    body_entry,
                    scope,
                    static_value=strings,
                    result=element_result,
                )
                header_flow = self._merge_flows(
                    header_flow.without_normal(),
                    self._normal_flow(body_entry, target_effects),
                )
            body = header_flow.sequence(
                lambda normal: self.exec_statements(node.body, normal, scope),
                join_states=self.states.join,
            )
            retained_boundary |= body.effects
            return exhausted, body

        def widen(header: int) -> int:
            return self.states.invalidate_members(
                self._widen_exposed_bindings(header),
                self.policy.callback_member_writes,
            )

        def terminal_release_effects() -> EffectMask:
            assert iteration is not None
            assert isinstance(node, (ast.For, ast.AsyncFor))
            boundary = iteration.effects | retained_boundary
            release_result = _result_after_iteration_boundary(iterable_result, boundary)
            release_may_call = release_result.release_may_call and not (
                isinstance(node.iter, ast.Name)
                and scope.kind != "class"
                and not boundary & _CALLEE_ELISION_FORBIDDEN_EFFECTS
            )
            return _RELEASE_CALLBACK_EFFECTS if release_may_call else NO_EFFECTS

        def finalize(state: int) -> PythonCompletionFlow[int]:
            release_effects = terminal_release_effects()
            self._record_operation_execution(
                scope,
                _operand_dispatch_deferred(
                    release_effects, iterable_result, reaches_contents=True
                ),
            )
            return self._normal_flow(
                self._apply_effects(state, release_effects),
                release_effects,
            )

        loop = PythonCompletionFlow.loop(
            entry,
            advance,
            lambda exhausted: self.exec_statements(node.orelse, exhausted, scope),
            join_states=self.states.join,
            equivalent_states=lambda left, right: (
                self.states.equivalent(left, right)
                and self.states.owner_tokens_equal(left, right)
            ),
            widen_state=widen,
            finalize=finalize if iteration is not None else None,
        )
        if iteration is not None:
            iteration = PythonIterationFact(
                effects=iteration.effects,
                element_strings=iteration.element_strings,
                element_result=iteration.element_result,
                empty=iteration.empty,
                mutable=iteration.mutable,
                release_effects=terminal_release_effects(),
                deferred_execution=iteration.deferred_execution
                | _operand_dispatch_deferred(
                    terminal_release_effects(), iterable_result, reaches_contents=True
                ),
            )
            assert iteration_key is not None
            previous = self.transfer.iterations.get(iteration_key)
            self.transfer.iterations[iteration_key] = (
                iteration if previous is None else previous.merge(iteration)
            )
        return self._merge_flows(
            prefix,
            loop,
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
            exact_file = context.result.kind in {
                "file_text",
                "file_bytes",
            } and not isinstance(node, ast.AsyncWith)
            # Broad file kinds transport the normal ``__enter__`` result, but
            # do not prove wrapped stream/state access callback-free.
            enter_effects = callback_effects
            entered = self._apply_effects(context.state_id, enter_effects)
            prefix = self._merge_flows(
                evaluated.without_normal(),
                self._normal_flow(entered, enter_effects).without_normal(),
            )
            assigned = PythonCompletionFlow(normal=entered)
            if item.optional_vars is not None:
                assigned_state, assigned_effects = self.assign_target(
                    item.optional_vars,
                    int(PythonIdentity.INERT_VALUE) if exact_file else OTHER_IDENTITY,
                    entered,
                    scope,
                    result=context.result if exact_file else UNKNOWN_EXPRESSION_RESULT,
                )
                assigned = self._normal_flow(assigned_state, assigned_effects)
            body = assigned.sequence(
                lambda normal: enter(index + 1, normal), join_states=self.states.join
            )

            def exit_context(incoming: int) -> PythonCompletionFlow[int]:
                exit_effects = callback_effects
                released = self._apply_effects(incoming, exit_effects)
                return self._normal_flow(released, exit_effects)

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
                # The frontend lowers this clear to FRAME_HOME_CLEAR, which
                # cannot raise.
                rebound, release_effects = self._bind_name(
                    exception_name,
                    exact_identity(PythonIdentity.INERT_VALUE),
                    incoming,
                    scope,
                    store_can_fail=False,
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

    def _declare_function(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda
    ) -> PythonSourceCallableDeclaration:
        owner = self._node_key(node)
        known = self.transfer.source_callable_declarations.get(owner)
        if known is not None:
            return known
        assert self._dependency_authority is not None
        yields = self._dependency_authority.summary(node).contains_yield
        if isinstance(node, ast.AsyncFunctionDef):
            kind = FunctionKind.ASYNC_GENERATOR if yields else FunctionKind.ASYNC
        else:
            kind = FunctionKind.GENERATOR if yields else FunctionKind.SYNC
        arguments = node.args
        declaration = PythonSourceCallableDeclaration(
            owner,
            kind,
            tuple(
                self._node_key(arg) for arg in (*arguments.posonlyargs, *arguments.args)
            ),
            self._node_key(arguments.vararg) if arguments.vararg is not None else None,
            tuple(
                (arg.arg, self._node_key(arg))
                for arg in (*arguments.args, *arguments.kwonlyargs)
            ),
            self._node_key(arguments.kwarg) if arguments.kwarg is not None else None,
        )
        self.transfer.source_callable_declarations[owner] = declaration
        return declaration

    def _call_evidence(self, call: ast.Call) -> CallEvidence | None:
        fact = self.transfer.calls.get(self._node_key(call))
        if fact is None:
            return None
        return CallEvidence(
            fact.callee_identities,
            self._node_key(call) in self.transfer.source_calls,
        )

    def _local_literal_actuals(
        self, job: _FunctionJob
    ) -> dict[PythonSourceKey, PythonStringValue]:
        """Join a closed local helper's already observed direct callers."""
        node = job.node
        if (
            job.annotation_scope
            or not isinstance(node, ast.FunctionDef)
            or job.parent_scope.kind != "function"
            or self._opaque_activation_sources is None
            or self.policy.frame_locals_writer is not None
            or node.decorator_list
            or node.type_params
            or node.returns is not None
            or node.args.defaults
            or node.args.kwonlyargs
            or node.args.vararg is not None
            or node.args.kwarg is not None
        ):
            return {}
        parameters = (*node.args.posonlyargs, *node.args.args)
        if not parameters or any(
            parameter.annotation is not None for parameter in parameters
        ):
            return {}
        source = self._node_key(node)
        parent = job.parent_scope.source_node
        if (
            not isinstance(parent, ast.FunctionDef)
            or len(parent.body) < 2
            or parent.body[0] is not node
        ):
            return {}
        # This deliberately small grammar closes caller coverage locally: the
        # frame only creates this helper and invokes it. Any escape, alternate
        # binding, nested caller or unobserved call retains generic activation.
        assert self._dependency_authority is not None
        dependencies = self._dependency_authority.summary(node).body
        if node.name in dependencies.lexical | dependencies.globals:
            return {}
        for statement in parent.body[1:]:
            if not isinstance(statement, ast.Expr) or not isinstance(
                statement.value, ast.Call
            ):
                return {}
            call = statement.value
            if (
                not isinstance(call.func, ast.Name)
                or call.func.id != node.name
                or call.keywords
            ):
                return {}
            if len(call.args) != len(parameters) or any(
                not isinstance(argument, ast.Constant)
                or type(argument.value) is not str
                for argument in call.args
            ):
                return {}
        # Only an admitted caller shape adds a new observed activation premise.
        # Refused shapes leave existing recorder/replay eligibility unchanged.
        if self._activation_group_is_opaque(frozenset({source})):
            return {}
        values: list[set[str]] = [set() for _parameter in parameters]
        for statement in parent.body[1:]:
            assert isinstance(statement, ast.Expr) and isinstance(
                statement.value, ast.Call
            )
            call = statement.value
            key = self._node_key(call)
            fact = self.transfer.calls.get(key)
            actual = self.transfer.source_calls.get(key)
            if (
                fact is None
                or not fact.callee_is(PythonIdentity.USER_FUNCTION)
                or actual is None
                or actual.callees != {source}
                or len(actual.arguments) != len(parameters)
                or len(actual.argument_strings) != len(parameters)
                or any(
                    name != index
                    for index, (name, _origins, _exposed) in enumerate(actual.arguments)
                )
                or any(value is None for value in actual.argument_strings)
            ):
                return {}
            for joined, value in zip(values, actual.argument_strings, strict=True):
                assert value is not None
                joined.add(value)
        return {
            self._node_key(parameter): PythonStringAlternatives(frozenset(joined))
            for parameter, joined in zip(parameters, values, strict=True)
        }

    def _analyze_function_job(self, job: _FunctionJob, outer_state: int) -> None:
        node = job.node
        arguments = node.args
        parameters = function_parameter_names(arguments)
        parameter_nodes = function_parameter_nodes(arguments)
        source = self._node_key(node)
        if not job.annotation_scope:
            self.transfer.executed_function_nodes[source] = node
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
            activation_namespace_stable=False,
        )
        scope.annotation_evaluator = job.annotation_scope
        state_id = self.states.invalidate_members(
            outer_state, self.policy.callback_member_writes
        )
        # Free cells are activation inputs, not source-owned locals. A later
        # nonlocal write can still establish fresh exact facts until a callback.
        updates = self._activation_input_updates(state_id, job.lexical_base_bindings)
        parameter_defaults = dict(job.parameter_defaults)
        local_actuals = self._local_literal_actuals(job)
        # No Python can observe a partly initialized frame. Publish its
        # captures, parameter bindings and empty local slots in one state.
        locals_at_entry: dict[int, _BindingUpdate] = {
            slot: (slot, UNBOUND_IDENTITY, None, UNKNOWN_EXPRESSION_RESULT, 0)
            for slot in scope.slots.values()
        }
        for parameter, parameter_node in zip(parameters, parameter_nodes, strict=True):
            slot = scope.slots.get(parameter)
            if slot is not None:
                default_result = parameter_defaults.get(
                    parameter, UNKNOWN_EXPRESSION_RESULT
                )
                identities = OTHER_IDENTITY | default_result.identities
                parameter_source = self._node_key(parameter_node)
                argument_value = (
                    None
                    if job.annotation_scope
                    else self._activation_argument_values.get(
                        parameter_source, local_actuals.get(parameter_source)
                    )
                )
                if argument_value is not None:
                    default_result = (
                        join_static_expression_results(
                            tuple(
                                StaticExpressionResult.scalar(value)
                                for value in sorted(argument_value.values)
                            )
                        )
                        if isinstance(argument_value, PythonStringAlternatives)
                        else StaticExpressionResult.scalar(argument_value)
                    )
                    identities = int(PythonIdentity.INERT_VALUE)
                locals_at_entry[slot] = (
                    slot,
                    identities,
                    PythonParameterRef(parameter)
                    if argument_value is None
                    else argument_value,
                    replace(
                        expression_result_without_value_facts(default_result)
                        if argument_value is None
                        else default_result,
                        source_origins=default_result.source_origins
                        | {parameter_source},
                    ),
                    0,
                )
        updates.extend(locals_at_entry.values())
        state_id = self.states.set_bindings(state_id, updates)
        observed: list[int] = [state_id]
        self._observed_stack.append(_ObservedStateFrame(observed))
        previous_history = self._active_lexical_history
        self._active_lexical_history = observed
        try:
            flow = self.exec_statements(body, state_id, scope)
            if job.annotation_scope:
                # Annotation evaluators execute through their lexical scope.
                # The synthetic lambda does not own a source callable result.
                return
            previous = self.transfer.source_function_results.get(source)
            previous_closed = previous is None or previous.returns_calls_only
            previous = previous or PythonFunctionResult(
                source,
                frozenset(),
                frozenset(),
                frozenset(),
                frozenset(),
            )
            self.transfer.source_function_results[source] = previous._replace(
                returned=previous.returned | scope.returned_source_origins,
                returned_exposed=previous.returned_exposed
                | scope.returned_exposed_source_origins,
                yielded=previous.yielded | scope.yielded_source_origins,
                yielded_exposed=previous.yielded_exposed
                | scope.yielded_exposed_source_origins,
                returns_calls_only=(
                    flow.normal is None
                    and flow.returned is not None
                    and scope.returns_calls_only
                    and previous_closed
                ),
            )
        finally:
            self._active_lexical_history = previous_history
            self._observed_stack.pop()

    def _seal_deferred_execution(
        self,
    ) -> tuple[
        tuple[PythonExpressionFact, ...],
        tuple[PythonStatementFact, ...],
        tuple[tuple[PythonSourceKey, PythonIterationFact], ...],
        bool,
    ]:
        # Two finite summaries per existing lexical scope: direct execution,
        # and an escaped value that may expose descendants/returned callables.
        # Build reverse edges once; each effect bit traverses each edge once.
        owners = {}
        for (key, _parent, _kind), scope in self._source_scopes.items():
            owners.setdefault(key, []).append(scope.scope_id)
        effects = [
            value
            for scope in self.scopes
            for value in (
                scope.module_metadata_effects,
                scope.module_metadata_effects | scope.escaped_metadata_effects,
            )
        ]
        # Result origins are source values rather than lexical scopes. Keep
        # their conservative effect projection in this same effect worklist;
        # changing value identity must not turn every known call into an opaque
        # metadata mutation. The escaped callee/receiver summary includes all
        # source-owned values it can return or expose.
        result_sources = {
            source: call.callees for source, call in self.transfer.source_calls.items()
        }
        result_sources.update(
            (source, element.origins)
            for source, element in self.transfer.source_elements.items()
        )
        result_sources.update(self.transfer.source_completions)
        result_sources.update(
            (source, frozenset().union(*selections.values()))
            for source, selections in self.transfer.source_attributes.items()
        )
        result_indices: dict[PythonSourceKey, int] = {}
        for source in result_sources:
            result_indices[source] = len(effects)
            effects.extend((NO_EFFECTS, NO_EFFECTS))
        dependents: list[set[int]] = [set() for _ in effects]

        def indices(ref: DeferredExecution) -> tuple[int, ...]:
            escaped = int(
                ref.phase
                in {
                    "escape",
                    "release",
                    "protocol",
                    "truth",
                    "descriptor",
                    "mutate",
                    "method",
                    "iterate",
                    "async_iterate",
                    "get_iter",
                    "get_async_iter",
                    "next",
                    "async_next",
                }
            )
            if ref.source in result_indices:
                return (result_indices[ref.source] + escaped,)
            return tuple(2 * owner + escaped for owner in owners.get(ref.source, ()))

        for source, origins in result_sources.items():
            target = result_indices[source]
            dependents[target].add(target + 1)
            for origin in origins:
                sources = indices(DeferredExecution(origin, "escape"))
                if not sources:
                    effects[target] |= WRITES_MODULE_METADATA
                for dependency in sources:
                    dependents[dependency].add(target)

        for scope in self.scopes:
            direct = 2 * scope.scope_id
            escaped = direct + 1
            dependents[direct].add(escaped)
            if scope.parent is not None:
                dependents[escaped].add(2 * scope.parent.scope_id + 1)
            for ref in scope.escaped_deferred:
                targets = indices(DeferredExecution(ref.source, "escape"))
                if not targets:
                    effects[escaped] |= WRITES_MODULE_METADATA
                for target in targets:
                    dependents[target].add(escaped)
            for ref in scope.deferred_execution:
                targets = indices(ref)
                if not targets:
                    effects[direct] |= WRITES_MODULE_METADATA
                for target in targets:
                    dependents[target].add(direct)
        pending = deque(index for index, effect in enumerate(effects) if effect)
        queued = set(pending)
        while pending:
            origin = pending.popleft()
            queued.discard(origin)
            for target in dependents[origin]:
                updated = effects[target] | effects[origin]
                if updated != effects[target]:
                    effects[target] = updated
                    if target not in queued:
                        queued.add(target)
                        pending.append(target)
        module_import_flow_required = self._module_import_flow_required

        def seal[
            Fact: PythonExpressionFact | PythonIterationFact | PythonStatementFact
        ](fact: Fact) -> Fact:
            nonlocal module_import_flow_required
            resolved = fact.module_metadata_effects
            for ref in fact.deferred_execution:
                targets = indices(ref)
                if not targets:
                    resolved |= WRITES_MODULE_METADATA
                for target in targets:
                    resolved |= effects[target]
            module_import_flow_required |= bool(resolved)
            if resolved != fact.module_metadata_effects:
                return replace(fact, module_metadata_effects=resolved)
            return fact

        iterations = {key: seal(fact) for key, fact in self.transfer.iterations.items()}

        def project_statement(
            fact: PythonStatementFact,
        ) -> PythonStatementFact:
            if fact.iteration is not None:
                iteration = iterations.get(fact.node, fact.iteration)
                if iteration is not fact.iteration:
                    fact = replace(fact, iteration=iteration)
            return seal(fact)

        # Transfer facts and scope effects remain available for another
        # projection after a deferred input changes.
        expressions = tuple(
            sorted(
                (seal(fact) for fact in self._project_builtin_guard_bits()),
                key=lambda fact: fact.node,
            )
        )
        statements = tuple(
            sorted(
                map(project_statement, self.transfer.statements.values()),
                key=lambda fact: fact.node,
            )
        )
        return (
            expressions,
            statements,
            tuple(sorted(iterations.items())),
            module_import_flow_required,
        )

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
        summary.refresh(self.states)
        maybe, definitely, taint_epoch, namespace_unknown = summary.properties(
            summary_start
        )
        state_id = self.states.overlay_summary_properties(
            state_id,
            maybe_invalidated_members=maybe,
            definitely_invalidated_members=definitely,
            taint_epoch=taint_epoch,
            namespace_values_unknown=namespace_unknown,
        )
        updates: list[
            tuple[int, IdentityMask, PythonStaticValue, StaticExpressionResult, int]
        ] = []
        for slot, base_binding in base_bindings:
            future_binding, future_value = summary.project(
                self.states, summary_start, slot
            )
            future_result = join_static_expression_results(
                (
                    expression_result_without_value_facts(
                        self.states.result(state_id, slot)
                    ),
                    future_value,
                )
            )
            updates.append(
                (
                    slot,
                    base_binding | future_binding,
                    None,
                    future_result,
                    0,
                )
            )
        return self.states.set_bindings(state_id, updates)

    def _project_builtin_guard_bits(self) -> Iterator[PythonExpressionFact]:
        """Attach a guard bit to each unproved builtin read the module never binds.

        A function's read of `len` is a live lookup. When the closed writer
        premise proves the read selects the builtin, it needs no guard: no code
        in the graph can rebind it. Otherwise, when no module statement,
        `global` declaration or literal reflective write binds `len`, the
        compiler may specialize the read behind a runtime binding guard; the
        guard, not this projection, proves the binding at run time. A name the
        module binds would fail its guard on every call, so it gets none.
        """
        assert self.module_scope is not None
        for key, fact in self.transfer.expressions.items():
            name = self.transfer._builtin_guard_reads.get(key)
            if (
                name is None
                or self._module_slot(name) is not None
                or fact.exact_builtin_name() == name
            ):
                yield fact
            else:
                yield replace(fact, builtin_guard_bit=BUILTIN_BINDING_GUARD_BITS[name])

    def analyze(
        self,
        tree: ast.Module,
        *,
        ast_digest_admission: _PythonAstDigestAdmission | None = None,
    ) -> _BindingAnalysis:
        revisions = _BINDING_REVISIONS.get()
        if (
            revisions is not None
            and self._opaque_activation_sources is not None
            and self.policy.analyze_deferred_bodies
        ):
            return revisions.analyze(
                self, tree, ast_digest_admission=ast_digest_admission
            )
        self._analyze_module(tree, ast_digest_admission=ast_digest_admission)
        return self._finish_analysis(
            0 if self.policy.analyze_deferred_bodies else len(self.function_jobs)
        )

    def _analyze_module(
        self,
        tree: ast.Module,
        *,
        ast_digest_admission: _PythonAstDigestAdmission | None,
    ) -> None:
        lexical = (
            PythonModuleLexicalPlan.build(tree, target_python=self.policy.target_python)
            if ast_digest_admission is None
            else ast_digest_admission.lexical_plan(
                tree, target_python=self.policy.target_python
            )
        )
        self._ast_admission = ast_digest_admission
        if ast_digest_admission is not None:
            self._call_argument_plan = ast_digest_admission.call_argument_plan
        self._future_annotations = lexical.authority.future_annotations
        self._dependency_authority = lexical.authority
        self._star_import_origins = lexical.star_import_origins
        module = self._new_scope(
            parent=None,
            source_node=tree,
            kind="module",
            name="<module>",
            declarations=lexical.module_declarations,
        )
        self.module_scope = module
        self._module_source = self._node_key(tree)
        self._module_tree = tree
        self._module_read_slots_prepared = False
        self.module_slots = set(module.slots.values())
        self.module_slot_mask = sum(1 << slot for slot in self.module_slots)
        declared_slots = len(self.slot_names)
        self._configure_callback_namespace(tree, ast_digest_admission)
        if (
            self._foreign_universe is not None
            and self._callback_module_names is not None
        ):
            self._foreign_reads = (
                set(MODULE_IMPORT_METADATA_NAMES)
                if self._initializer_premise_reads is None
                else set()
            )
            self._foreign_slot_bounds = (declared_slots, len(self.slot_names))
        if self._star_import_origins:
            # Star imports can shadow otherwise undeclared globals, including
            # builtin fallbacks. Give actual global reads storage so the
            # explicit import write reaches them even with a bounded callback
            # domain. The lexical authority excludes nested local reads.
            self._ensure_module_read_slots()
        self.states.set_taint_domain(self._callback_taint_domain())
        entry_state = self.states.invalidate_members(
            0, self.policy.entry_invalid_members
        )
        self._module_history = [entry_state]
        self._execute_initializer(entry_state)

    def _execute_initializer(
        self, entry: int, continuation: _InitializerContinuation | None = None
    ) -> None:
        assert self.module_scope is not None
        module = self.module_scope
        if continuation is None:
            observation = _ObservedStateFrame([entry])
        else:
            source = continuation.observation
            history = (
                source.states.copy()
                if continuation.history_job is None
                else self.function_jobs[continuation.history_job].lexical_history
            )
            assert history is not None
            observation = replace(
                source, states=history, folded_ids=source.folded_ids.copy()
            )
        self._observed_stack.append(observation)
        self._active_lexical_history = observation.states
        try:
            module_flow = self.exec_statements(
                self._module_tree.body,
                entry,
                module,
                continuation=continuation,
                capture=self._capture_initializer,
            )
            if module_flow.normal is not None:
                namespace = self.transfer.source_namespaces.setdefault(
                    self._module_source, {}
                )
                if (
                    self._foreign_reads is not None
                    and self.states.get(module_flow.normal).taint_epoch
                ):
                    self._foreign_tainted_at_exit = True
                for name, slot in module.slots.items():
                    if (
                        self.states.binding(module_flow.normal, slot)
                        == UNBOUND_IDENTITY
                    ):
                        continue
                    origins = self.states.result(
                        module_flow.normal, slot
                    ).exposed_source_origins
                    if origins:
                        namespace[name] = namespace.get(name, frozenset()) | origins
                    if name == "__getattr__":
                        self._module_getattr_may_be_bound = True

        finally:
            self._observed_stack.pop()
            self._active_lexical_history = None
        if (
            self._initializer_premise_reads is not None
            and self._foreign_reads is not None
        ):
            self._initializer_premise_reads.names = frozenset(self._foreign_reads)
            self._foreign_reads.update(MODULE_IMPORT_METADATA_NAMES)
            self._foreign_reads.update(self._initializer_premise_reads.deferred_names)

    def _run_deferred_job(
        self, job: _FunctionJob, recorder: _DeferredJobRecorder | None
    ) -> None:
        # Deferred bodies can run after any observed module state. Parent
        # function state supplies closure cells and the module summary widens
        # the globals used in this activation.
        module_states = job.module_states
        if module_states is None:
            assert job.module_history_start is not None
            module_states = self._module_history
            module_state_start = min(
                job.module_history_start, len(self._module_history)
            )
        else:
            # Only (job_outer,) and unions of such nonempty tuples are minted.
            assert module_states, "deferred module-state authority is nonempty"
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
            if recorder is None:
                self._analyze_function_job(job, job_outer)
            else:
                recorder.run(self, job, job_outer)
        finally:
            self._active_module_states = previous_module_states

    def _finish_analysis(
        self,
        cursor: int,
        *,
        capture: Callable[[_Analyzer, int], None] | None = None,
        recorder: _DeferredJobRecorder | None = None,
    ) -> _BindingAnalysis:
        while cursor < len(self.function_jobs):
            # Exponential spacing bounds checkpoints in each deferred suffix.
            # The retention policy admits early boundaries on small sources.
            if capture is not None and cursor >= 2 and not cursor & (cursor - 1):
                capture(self, cursor)
            self._run_deferred_job(self.function_jobs[cursor], recorder)
            cursor += 1
        return self._project_analysis()

    def _project_analysis(self) -> _BindingAnalysis:
        lexical = self._dependency_authority
        assert lexical is not None
        expressions, statements, iterations, flow_required = (
            self._seal_deferred_execution()
        )
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
                scope.activation_namespace_stable,
            )
            for scope in self.scopes
        )
        # The invariant-state fast path is valid only when completed binding
        # effects cannot change import metadata. A release, descriptor store,
        # unpack, deletion, or named-expression assignment can invoke Python
        # even when its target is not spelled __package__/__spec__/__name__.
        # Do not let an unrelated deferred return decide whether these facts
        # are projected: every target family records its effects here.
        import_relevant_assignment_effects = any(
            effects & (NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS | WRITES_MODULE_METADATA)
            for effects in self.transfer.assignment_effects.values()
        )
        function_results = sorted(self.transfer.source_function_results.items())
        facts = PythonBindingFacts.create(
            source_digest=self.source_digest,
            class_builder_reads=self.transfer.class_builder_reads,
            decoration_annotation_heads=self.transfer.decoration_annotation_heads,
            callback_metadata_writes=(
                MODULE_IMPORT_METADATA_NAMES
                if self._callback_module_names is None
                else self._callback_module_names & MODULE_IMPORT_METADATA_NAMES
            ),
            source_namespaces=tuple(
                (
                    owner,
                    tuple(sorted(namespace.items())),
                    self.transfer.source_namespace_exposure.get(owner, frozenset()),
                )
                for owner, namespace in sorted(self.transfer.source_namespaces.items())
            ),
            module_getattr_may_be_bound=self._module_getattr_may_be_bound,
            source_attributes=tuple(
                (owner, name, origins)
                for owner, selections in sorted(self.transfer.source_attributes.items())
                for name, origins in sorted(selections.items())
            ),
            source_bases=tuple(sorted(self.transfer.source_bases.items())),
            source_elements=tuple(
                value for _owner, value in sorted(self.transfer.source_elements.items())
            ),
            source_calls=tuple(
                value for _, value in sorted(self.transfer.source_calls.items())
            ),
            source_native_initializers=tuple(
                value
                for _, value in sorted(self.transfer.source_native_initializers.items())
            ),
            source_classes=tuple(
                value for _, value in sorted(self.transfer.source_classes.items())
            ),
            source_callables=tuple(sorted(self.transfer.source_callables.items())),
            source_callable_declarations=tuple(
                value
                for _, value in sorted(
                    self.transfer.source_callable_declarations.items()
                )
            ),
            source_function_results=tuple(value for _, value in function_results),
            source_activation_partitions=tuple(
                PythonActivationPartition(
                    owner, lexical.parameter_plan(node).resolve(self._call_evidence)
                )
                for owner, _ in function_results
                if (node := self.transfer.executed_function_nodes.get(owner))
                is not None
            ),
            source_completions=tuple(sorted(self.transfer.source_completions.items())),
            target_python=self.policy.target_python,
            target_sys_platform=self.policy.target_sys_platform,
            expressions=expressions,
            statements=statements,
            iterations=iterations,
            calls=tuple(
                sorted(self.transfer.calls.values(), key=lambda fact: fact.node)
            ),
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
                join_node_contributor_inputs=self.states.join_node_contributor_inputs,
                join_node_contributor_outputs=self.states.join_node_contributor_outputs,
                join_node_duplicate_contributors=(
                    self.states.join_node_duplicate_contributors
                ),
                join_shared_subtrees_skipped=(self.states.join_shared_subtrees_skipped),
                join_chunk_merges=self.states.join_chunk_merges,
                join_parent_inputs=self.states.join_parent_inputs,
                join_max_parents=self.states.join_max_parents,
                join_two_way_slots=self.states.join_two_way_slots,
                join_wide_slots=self.states.join_wide_slots,
                join_wide_alternatives=self.states.join_wide_alternatives,
                join_payload_identity_skips=self.states.join_payload_identity_skips,
                join_payload_absence_skips=self.states.join_payload_absence_skips,
                join_payload_algebra_calls=self.states.join_payload_algebra_calls,
                join_payload_absorptions=self.states.join_payload_absorptions,
                join_custody_identity_skips=self.states.join_custody_identity_skips,
                observation_fold_calls=self.states.observation_fold_calls,
                observation_fold_new_parents=self.states.observation_fold_new_parents,
                observation_fold_rebuilds=self.states.observation_fold_rebuilds,
                structural_diff_cache_entries=0,
                structural_diff_node_visits=(self.states.structural_diff_node_visits),
                structural_diff_shared_subtrees_skipped=(
                    self.states.structural_diff_shared_skips
                ),
            ),
            slot_names=tuple(self.slot_names),
        )
        declaration_end, callback_end = self._foreign_slot_bounds
        if self._appended_foreign_slots:
            appended = frozenset(self._appended_foreign_slots)
            slots = facts.slot_names
            facts = _project_binding_slots(
                facts,
                (
                    *slots[:declaration_end],
                    *sorted(
                        (
                            *slots[declaration_end:callback_end],
                            *(slots[slot] for slot in self._appended_foreign_slots),
                        )
                    ),
                    *(
                        slots[slot]
                        for slot in range(callback_end, len(slots))
                        if slot not in appended
                    ),
                ),
            )
            callback_end += len(appended)
        return _BindingAnalysis(
            facts,
            # Source demand proofs may require a projection even when strict
            # metadata is invariant. Preserve canonical no-effect assignments;
            # dropping them would turn harmless stores into unknown callbacks.
            MappingProxyType(dict(self.transfer.assignment_effects)),
            flow_required or import_relevant_assignment_effects,
            (
                None
                if self._opaque_activation_sources is None
                else _OpaqueActivationReads(
                    tuple(
                        group
                        for group, answer in self._opaque_activation_reads.items()
                        if answer
                    ),
                    frozenset(
                        source
                        for group, answer in self._opaque_activation_reads.items()
                        if not answer
                        for source in group
                    ),
                )
            ),
            (
                None
                if self._foreign_reads is None
                else _ForeignNamespaceReads(
                    frozenset(self._foreign_reads),
                    declaration_end,
                    callback_end,
                    self._foreign_tainted_at_exit,
                    self._has_empty_module_expiry,
                )
            ),
        )

    def _fork_at_deferred_boundary(
        self, *, empty_transfer: bool = False, initializer_prefix: bool = False
    ) -> _Analyzer:
        """Fork a drained prefix without sharing mutable scope or job owners."""
        assert self.module_scope is not None and self._dependency_authority is not None
        assert not self._binding_read_scopes and self._active_module_states is None
        if initializer_prefix:
            assert len(self._observed_stack) == 1
            assert self._active_lexical_history is self._observed_stack[0].states
        else:
            assert not self._observed_stack and self._active_lexical_history is None
        result = _copy_analysis_containers(self)
        result._observed_stack = []
        result._active_lexical_history = None
        result._initializer_premise_reads = None
        result._capture_initializer = None
        # Flow indexes are private. Lexical queries depend only on the admitted
        # syntax and target, so every suffix uses the same source authority.
        result.states = _copy_analysis_containers(self.states)
        if empty_transfer:
            result.transfer = _BindingTransferFacts()
        else:
            result.transfer = _copy_analysis_containers(self.transfer)
            result.transfer.source_namespaces = {
                owner: names.copy()
                for owner, names in self.transfer.source_namespaces.items()
            }
        result._history_summaries = {}

        scopes: list[_Scope] = []
        for scope in self.scopes:
            parent = scopes[scope.parent.scope_id] if scope.parent is not None else None
            scopes.append(replace(scope, parent=parent, slots=scope.slots.copy()))
        result.scopes = scopes
        result.module_scope = scopes[self.module_scope.scope_id]
        result._source_scopes = {
            key: scopes[scope.scope_id] for key, scope in self._source_scopes.items()
        }
        # Several jobs can read the same ancestor's observed states. Fork
        # each list once and retain those aliases in the new worklist.
        histories = {id(self._module_history): result._module_history}

        def fork_job(job: _FunctionJob) -> _FunctionJob:
            history = job.lexical_history
            if history is not None:
                owned = histories.get(id(history))
                if owned is None:
                    owned = histories[id(history)] = history.copy()
                history = owned
            return replace(
                job,
                parent_scope=scopes[job.parent_scope.scope_id],
                lexical_history=history,
            )

        jobs = {id(job): fork_job(job) for job in self.function_jobs}
        result.function_jobs = [jobs[id(job)] for job in self.function_jobs]
        result._queued_function_jobs = {
            key: jobs[id(job)] for key, job in self._queued_function_jobs.items()
        }
        return result


def _copy_analysis_containers[T](owner: T) -> T:
    """Copy the flat, analysis-owned containers of a binding work owner."""
    result = copy(owner)
    for name, value in vars(owner).items():
        if isinstance(value, (dict, list, set)):
            setattr(result, name, value.copy())
    return result


def _changed_activation_parameters(
    previous: Mapping[PythonSourceKey, PythonStringValue],
    current: Mapping[PythonSourceKey, PythonStringValue],
) -> set[PythonSourceKey]:
    # A missing fact changes the owning job just as a new literal does.
    return {
        key
        for key in previous.keys() | current.keys()
        if previous.get(key) != current.get(key)
    }


@dataclass(frozen=True, slots=True)
class _RecordedBindingTransfer:
    analyzer: _Analyzer
    state_count: int
    telemetry: PythonBindingTelemetry
    initializer_reads: Mapping[frozenset[PythonSourceKey], bool]
    initializer_foreign_names: frozenset[str]
    initializer_empty_expiry: bool
    jobs: tuple[_DeferredJobReads, ...]

    def revise(
        self, policy: PythonBindingFlowPolicy, *, suffix_jobs: int | None
    ) -> tuple[_BindingAnalysis, _RecordedBindingTransfer] | None:
        activation_sources = policy.opaque_activation_sources
        if (
            activation_sources is None
            or replace(
                self.analyzer.policy,
                opaque_activation_sources=activation_sources,
                activation_argument_values=policy.activation_argument_values,
            )
            != policy
        ):
            return None
        active = frozenset(activation_sources)
        arguments = dict(policy.activation_argument_values)
        changed_arguments = _changed_activation_parameters(
            self.analyzer._activation_argument_values, arguments
        )

        def differs(group: frozenset[PythonSourceKey]) -> bool:
            return self.analyzer._opaque_activation_reads[group] != (
                not group.isdisjoint(active)
            )

        if any(differs(group) for group in self.initializer_reads):
            return None
        changed = [
            index
            for index, record in enumerate(self.jobs)
            if any(differs(group) for group in record.activation_groups)
            or (
                changed_arguments
                and any(
                    self.analyzer._node_key(parameter) in changed_arguments
                    for parameter in function_parameter_nodes(
                        self.analyzer.function_jobs[index].node.args
                    )
                )
            )
        ]
        if (
            not changed
            or (suffix_jobs is not None and len(changed) >= suffix_jobs)
            or any(self.jobs[index].state is None for index in changed)
        ):
            return None
        work = self.analyzer._fork_at_deferred_boundary()
        work.policy = policy
        work._opaque_activation_sources = active
        work._activation_argument_values = arguments
        work._opaque_activation_reads = {
            group: not group.isdisjoint(active)
            for group in work._opaque_activation_reads
        }
        jobs = list(self.jobs)
        try:
            for index in changed:
                jobs[index] = self._refresh_job(work, index)
        except _BindingReplayRefused:
            return None
        # Rebuild the module-wide witnesses from the actual initializer and
        # job reads. Reversing a revision can remove a former dependency.
        work._opaque_activation_reads = dict(self.initializer_reads)
        for job in jobs:
            for group in job.activation_groups:
                work._opaque_activation_reads[group] = not group.isdisjoint(active)
        if work._foreign_reads is not None:
            work._foreign_reads = set(self.initializer_foreign_names)
            for job in jobs:
                work._foreign_reads.update(job.foreign_names)
        work._has_empty_module_expiry = self.initializer_empty_expiry or any(
            job.empty_module_expiry for job in jobs
        )
        # Projection reads module slots too. Cached lookups from the previous
        # projection cannot supply those reads to this generation's witness.
        work._scope_slot_cache.clear()
        result = work._project_analysis()
        # A refreshed result shares the unaffected transfer's state statistics.
        result = replace(
            result,
            facts=replace(
                result.facts,
                state_count=self.state_count,
                telemetry=self.telemetry,
            ),
        )

        return result, replace(self, analyzer=work, jobs=tuple(jobs))

    def _refresh_job(self, work: _Analyzer, index: int) -> _DeferredJobReads:
        record = self.jobs[index]
        state = record.state
        assert state is not None
        later = self.jobs[index + 1 :]
        if any(
            scope_id < record.scope_end
            for other in later
            for scope_id in other.earlier_scope_writes
        ):
            raise _BindingReplayRefused
        for later_job in work.function_jobs[index + 1 :]:
            parent: _Scope | None = later_job.parent_scope
            while parent is not None:
                if state.scope_count <= parent.scope_id < record.scope_end:
                    raise _BindingReplayRefused
                parent = parent.parent

        leaf = work._fork_at_deferred_boundary(empty_transfer=True)
        job = leaf.function_jobs[index]
        leaf.scopes = leaf.scopes[: state.scope_count]
        leaf._source_scopes = {
            key: scope
            for key, scope in leaf._source_scopes.items()
            if scope.scope_id < state.scope_count
        }
        leaf.slot_names = leaf.slot_names[: state.slot_count]
        leaf.slot_owner_scope_ids = leaf.slot_owner_scope_ids[: state.slot_count]
        leaf._class_cell_slots = {
            owner: slot
            for owner, slot in leaf._class_cell_slots.items()
            if slot < state.slot_count
        }
        leaf._class_namespace_slots = {
            owner: slot
            for owner, slot in leaf._class_namespace_slots.items()
            if slot < state.slot_count
        }
        leaf._class_cell_slot_mask &= (1 << state.slot_count) - 1
        leaf.callback_slot_mask = state.callback_slots
        leaf.callback_module_slot_mask = state.callback_module_slots
        leaf._callback_module_names = state.callback_module_names
        leaf._namespace_observation_epoch = state.observation_epoch
        leaf._next_owner_token = state.next_owner_token
        leaf._module_import_flow_required = state.import_flow_required
        leaf._module_read_slots_prepared = state.read_slots_prepared
        leaf._super_attribute_receivers.difference_update(map(id, ast.walk(job.node)))
        leaf._scope_slot_cache.clear()
        leaf._opaque_lexical_activations.clear()
        leaf._replaying_deferred = True
        earlier_scope_writes: set[int] = set()
        leaf._deferred_scope_writes = (state.scope_count, earlier_scope_writes)

        work_tables = work.transfer.tables()
        watched = leaf.transfer.watch_tables(
            tuple(zip(work_tables, record.facts, strict=True))
        )
        for table in watched:
            table.observing = True
        leaf.states, entry = state.entry.open()
        leaf._active_module_states = (entry,)
        groups: dict[frozenset[PythonSourceKey], None] = {}
        leaf._recorded_activation_groups = groups
        if leaf._foreign_reads is not None:
            leaf._foreign_reads = set()
        leaf._has_empty_module_expiry = False
        try:
            leaf._analyze_function_job(job, entry)
        finally:
            leaf._active_module_states = None
            leaf._deferred_scope_writes = None
            leaf._recorded_activation_groups = None
            accesses = tuple(table.access for table in watched)
            for table in watched:
                table.observing = False
                table.access = None

        if (
            len(leaf.scopes) != record.scope_end
            or leaf.slot_names != work.slot_names[: record.slot_end]
            or leaf.slot_owner_scope_ids != work.slot_owner_scope_ids[: record.slot_end]
            or leaf._next_owner_token != record.next_owner_token
            or leaf.callback_module_slot_mask != state.callback_module_slots
            or leaf._callback_module_names != state.callback_module_names
            or leaf.callback_slot_mask != record.callback_slots
            or leaf.states._callback_deletion_mask != record.callback_deletions
            or leaf._module_import_flow_required != record.import_flow_required
            or leaf._module_read_slots_prepared != record.read_slots_prepared
            or earlier_scope_writes
        ):
            raise _BindingReplayRefused
        refreshed_scopes = leaf.scopes[state.scope_count :]
        for scope in refreshed_scopes:
            original = work.scopes[scope.scope_id]
            if (
                scope.source != original.source
                or scope.kind != original.kind
                or scope.slots != original.slots
                or scope.parent is None
                or original.parent is None
                or scope.parent.scope_id != original.parent.scope_id
            ):
                raise _BindingReplayRefused

        for table_index, (table, old, new) in enumerate(
            zip(work_tables, record.facts, watched, strict=True)
        ):
            keys = new.keys() if old is None else old.writes | new.keys()
            changed = {
                key
                for key in keys
                if (key in table) != (key in new) or table.get(key) != new.get(key)
            }
            if any(
                access is not None
                and (
                    access.enumerates
                    or not changed.isdisjoint(access.reads)
                    or not changed.isdisjoint(access.writes)
                )
                for other in later
                if changed
                for access in (other.facts[table_index],)
            ):
                raise _BindingReplayRefused
            for key in changed:
                if key in new:
                    table[key] = new[key]
                else:
                    del table[key]

        for scope in refreshed_scopes:
            assert scope.parent is not None
            scope.parent = work.scopes[scope.parent.scope_id]
            work.scopes[scope.scope_id] = scope
        for key, prior_scope in work._source_scopes.items():
            if state.scope_count <= prior_scope.scope_id < record.scope_end:
                work._source_scopes[key] = work.scopes[prior_scope.scope_id]
        work._module_import_flow_required |= leaf._module_import_flow_required
        return replace(
            record,
            activation_groups=tuple(groups),
            foreign_names=frozenset(leaf._foreign_reads or ()),
            empty_module_expiry=leaf._has_empty_module_expiry,
            facts=accesses,
        )


@dataclass(frozen=True, slots=True)
class _RebasableInitializer:
    prefix: _Analyzer
    reads: _InitializerPremiseReads
    size: int
    continuation: _InitializerContinuation | None = None

    @classmethod
    def capture(
        cls,
        owner: _Analyzer,
        reads: _InitializerPremiseReads,
        completion: PythonCompletionFlow[int] | None = None,
    ) -> _RebasableInitializer | None:
        if owner._foreign_reads is None and completion is not None:
            return None
        size = _binding_checkpoint_size(owner) + len(reads.metadata)
        if size > 8192:
            return None
        prefix = owner._fork_at_deferred_boundary(
            initializer_prefix=completion is not None
        )
        if completion is None:
            return cls(prefix, reads, size)
        assert owner._foreign_reads is not None
        current = owner._observed_stack[0]
        history_job = next(
            (
                index
                for index, job in enumerate(owner.function_jobs)
                if job.lexical_history is current.states
            ),
            None,
        )
        reads = replace(
            reads,
            names=frozenset(owner._foreign_reads),
            metadata=reads.metadata.copy(),
            deferred_names=reads.deferred_names.copy(),
        )
        return cls(
            prefix,
            reads,
            size,
            _InitializerContinuation(
                completion,
                replace(
                    current,
                    states=current.states.copy(),
                    folded_ids=current.folded_ids.copy(),
                ),
                history_job,
            ),
        )

    def revise(self, requested: _Analyzer) -> _Analyzer | None:
        work = _rebase_binding_namespace(self.prefix, requested, self.reads)
        if work is None:
            return None
        if self.continuation is not None:
            entry = self.continuation.completion.normal
            assert entry is not None
            work._initializer_premise_reads = replace(
                self.reads,
                metadata=self.reads.metadata.copy(),
                deferred_names=self.reads.deferred_names.copy(),
            )
            try:
                work._execute_initializer(entry, self.continuation)
            finally:
                work._initializer_premise_reads = None
        return work


def _binding_activation_reads_agree(
    prefix: _Analyzer, active: frozenset[PythonSourceKey]
) -> bool:
    return all(
        answer != owners.isdisjoint(active)
        for owners, answer in prefix._opaque_activation_reads.items()
    )


def _rebase_binding_namespace(
    prefix: _Analyzer, requested: _Analyzer, reads: _InitializerPremiseReads
) -> _Analyzer | None:
    """Keep valid physical coordinates when a source gains foreign writers."""
    before, policy = prefix.policy, requested.policy
    active = requested._opaque_activation_sources
    former, current = (
        before.foreign_namespace_writes,
        policy.foreign_namespace_writes,
    )
    if active is None or former is None or current is None:
        return None
    if not _binding_activation_reads_agree(prefix, active):
        return None
    exposed = frame_locals_exposed(policy.target_python, policy.frame_locals_writer)
    formerly_exposed = frame_locals_exposed(
        before.target_python, before.frame_locals_writer
    )
    if exposed != formerly_exposed and (
        reads.frame_borrows
        or any(
            scope.kind not in ("module", "class") and scope.slots
            for scope in prefix.scopes
        )
    ):
        return None
    additions = frozenset(current).difference(former)
    if prefix._callback_module_names is None:
        # Both premises supply concrete foreign writes, so source-owned
        # reflection kept the callback domain open. The explicit
        # __builtins__ writer still participates in pristine lookup.
        if ("__builtins__" in former) != ("__builtins__" in current):
            return None
    else:
        if not frozenset(former).issubset(current):
            return None
        if not additions.isdisjoint(reads.names) or (
            additions and prefix._has_empty_module_expiry
        ):
            return None
        assert prefix.module_scope is not None
        existing = additions.intersection(prefix.module_scope.slots)
        if existing and any(state.taint_epoch for state in prefix.states._states):
            return None
    work = prefix._fork_at_deferred_boundary()
    work.policy = policy
    work._opaque_activation_sources = active
    work._activation_argument_values = requested._activation_argument_values.copy()
    if work._callback_module_names is None:
        return work
    assert work._foreign_reads is not None
    work._callback_module_names |= additions
    appended_start = len(work.slot_names)
    for name in sorted(additions):
        slot = work._ensure_module_slot(name)
        work.callback_module_slot_mask |= 1 << slot
    work.states.set_taint_domain(work._callback_taint_domain())
    for (state, scope), expected in reads.metadata.items():
        if (
            work._module_metadata_borrowing_snapshot(state, work.scopes[scope])
            != expected
        ):
            return None
    appended_end = len(work.slot_names)
    if not additions.isdisjoint(reads.deferred_names):
        if prefix._deferred_queue_generation != len(prefix.function_jobs):
            return None
        for job in work.function_jobs:
            module, lexical = work._activation_slot_bindings(
                job.node,
                job.parent_scope,
                job.outer_state_id,
                annotation_scope=job.annotation_scope,
            )
            if lexical != job.lexical_base_bindings:
                return None
            job.module_base_bindings = module
        if (
            len(work.slot_names) != appended_end
            or work.callback_slot_mask != prefix.callback_slot_mask
            or work.states._callback_deletion_mask
            != prefix.states._callback_deletion_mask
        ):
            return None
    if appended_end != appended_start:
        # Body histories keep their physical slot coordinates. The final
        # immutable layout puts newly named slots back in callback order.
        work._appended_foreign_slots += tuple(range(appended_start, appended_end))
        if work._foreign_tainted_at_exit and "__getattr__" in additions:
            work._module_getattr_may_be_bound = True
    return work


@dataclass(frozen=True, slots=True)
class _BindingCheckpoint:
    completed_jobs: int
    prefix: _Analyzer

    def agrees_with_activations(self, requested: _Analyzer) -> bool:
        prefix = self.prefix
        active = requested._opaque_activation_sources
        assert active is not None
        if not _binding_activation_reads_agree(prefix, active):
            return False
        previous = prefix._activation_argument_values
        current = requested._activation_argument_values
        if previous == current:
            return True
        changed = {
            parameter
            for parameter in previous.keys() | current.keys()
            if previous.get(parameter) != current.get(parameter)
        }
        return not any(
            prefix._node_key(parameter) in changed
            for owner in prefix.transfer.executed_function_nodes.values()
            for parameter in function_parameter_nodes(owner.args)
        )

    def rebase_namespace(self, requested: _Analyzer) -> _Analyzer | None:
        names = self.prefix._foreign_reads
        if names is None or not self.agrees_with_activations(requested):
            return None
        # A drained body prefix owns both its executed reads and the entry
        # bindings it assembled for scheduled bodies. Unlike an initializer
        # continuation, those observations must all survive unchanged.
        return _rebase_binding_namespace(
            self.prefix, requested, _InitializerPremiseReads(names=frozenset(names))
        )


def _binding_checkpoint_size(analyzer: _Analyzer) -> int:
    return (
        len(analyzer.states)
        + len(analyzer.transfer.expressions)
        + len(analyzer.transfer.statements)
    )


class _BindingRevisions:
    """Operation-local transfers for adjacent policies of the same source."""

    def __init__(self) -> None:
        self._lock = RLock()
        self._recent: OrderedDict[
            tuple[str, PythonBindingFlowPolicy], tuple[_BindingCheckpoint, ...]
        ] = OrderedDict()
        self._recorded: dict[
            tuple[str, PythonBindingFlowPolicy], _RecordedBindingTransfer
        ] = {}
        self._observed_sources: set[str] = set()
        self._initializers: OrderedDict[
            tuple[str, PythonBindingFlowPolicy], tuple[_RebasableInitializer, ...]
        ] = OrderedDict()
        self._initializer_extent = 0

    def analyze(
        self,
        analyzer: _Analyzer,
        tree: ast.Module,
        *,
        ast_digest_admission: _PythonAstDigestAdmission | None,
    ) -> _BindingAnalysis:
        policy = analyzer.policy
        active = analyzer._opaque_activation_sources
        assert active is not None
        arguments = analyzer._activation_argument_values
        key = (
            analyzer.source_digest,
            replace(
                policy, opaque_activation_sources=(), activation_argument_values=()
            ),
        )
        initializer_key = (
            key[0],
            replace(key[1], foreign_namespace_writes=(), frame_locals_writer=None),
        )
        with self._lock:
            prior = self._recent.get(key, ())
            recorded = self._recorded.get(key)
            initializers = self._initializers.get(initializer_key, ())
        accepted = next(
            (
                item
                for item in reversed(prior)
                if item.agrees_with_activations(analyzer)
            ),
            None,
        )
        if recorded is not None:
            # Compare the recorded leaves with the newest proven prefix before
            # either strategy forks an analyzer. Ties keep the contiguous suffix.
            suffix_jobs = (
                len(recorded.jobs) - accepted.completed_jobs
                if accepted is not None
                else None
            )
            revised = recorded.revise(policy, suffix_jobs=suffix_jobs)
            if revised is not None:
                result, refreshed = revised
                with self._lock:
                    if self._recorded.get(key) is recorded:
                        self._recorded[key] = refreshed
                return result
        checkpoints: list[_BindingCheckpoint] = []
        cursor = 0
        if accepted is not None:
            analyzer = accepted.prefix._fork_at_deferred_boundary()
            analyzer.policy = policy
            analyzer._opaque_activation_sources = active
            analyzer._activation_argument_values = arguments.copy()
            cursor = accepted.completed_jobs
            checkpoints.extend(item for item in prior if item.completed_jobs <= cursor)
        else:
            namespace_key = replace(key[1], foreign_namespace_writes=())
            with self._lock:
                namespace_candidates = [
                    item
                    for (digest, former), retained in self._recent.items()
                    if digest == key[0]
                    and former != key[1]
                    and replace(former, foreign_namespace_writes=()) == namespace_key
                    for item in retained
                    if item.completed_jobs > 0
                ]
            for checkpoint in sorted(
                namespace_candidates, key=lambda item: -item.completed_jobs
            ):
                rebased = checkpoint.rebase_namespace(analyzer)
                if rebased is not None:
                    analyzer = rebased
                    cursor = checkpoint.completed_jobs
                    checkpoints.append(
                        _BindingCheckpoint(
                            cursor, analyzer._fork_at_deferred_boundary()
                        )
                    )
                    break
        if cursor == 0 and accepted is None:
            for initializer in reversed(initializers):
                rebased = initializer.revise(analyzer)
                if rebased is not None:
                    analyzer = rebased
                    break
            else:
                reads = (
                    None
                    if analyzer._foreign_universe is None
                    else _InitializerPremiseReads()
                )
                first = next(
                    (item for item in initializers if item.continuation is not None),
                    None,
                )

                def capture_initializer(
                    completion: PythonCompletionFlow[int],
                ) -> None:
                    nonlocal first
                    assert reads is not None
                    first = _RebasableInitializer.capture(analyzer, reads, completion)

                analyzer._initializer_premise_reads = reads
                if reads is not None and first is None:
                    analyzer._capture_initializer = capture_initializer
                try:
                    analyzer._analyze_module(
                        tree, ast_digest_admission=ast_digest_admission
                    )
                finally:
                    analyzer._initializer_premise_reads = None
                    analyzer._capture_initializer = None
                if reads is not None:
                    completed = _RebasableInitializer.capture(analyzer, reads)
                    if completed is not None:
                        self._retain_initializers(
                            initializer_key,
                            (completed,) if first is None else (first, completed),
                        )

        def capture(owner: _Analyzer, completed_jobs: int) -> None:
            if checkpoints and checkpoints[-1].completed_jobs == completed_jobs:
                return
            # Bound retention on large sources as well as on many small
            # sources. Exact analysis still runs after a refused checkpoint.
            size = _binding_checkpoint_size(owner)
            if size > 32768:
                return
            if 0 < completed_jobs < 64 and (not checkpoints or size > 1200):
                return
            checkpoints.append(
                _BindingCheckpoint(completed_jobs, owner._fork_at_deferred_boundary())
            )
            if len(checkpoints) > 8:
                del checkpoints[0]

        if (
            cursor == 0
            and not checkpoints
            and analyzer.function_jobs
            and _binding_checkpoint_size(analyzer) <= 512
        ):
            capture(analyzer, 0)
        recorder = None
        if (
            cursor == 0
            and recorded is None
            and analyzer._foreign_universe is not None
            and 1 < len(analyzer.function_jobs) <= 256
            and _binding_checkpoint_size(analyzer) <= 8192
        ):
            # Recording is a one-time investment for each source. A later
            # policy shape can reuse its exact prefix and need not rebuild a
            # dependency log that can only serve that shape.
            with self._lock:
                if analyzer.source_digest not in self._observed_sources:
                    self._observed_sources.add(analyzer.source_digest)
                    recorder = _DeferredJobRecorder(analyzer)
        result = analyzer._finish_analysis(cursor, capture=capture, recorder=recorder)
        with self._lock:
            self._recent.pop(key, None)
            if (
                checkpoints
                and recorder is not None
                and len(recorder.jobs) <= 256
                and any(job.state is not None for job in recorder.jobs)
            ):
                # A leaf retains its persistent entry environment. The state
                # arena belongs to the completed transfer and need not stay live.
                analyzer.states = _StatePool()
                self._recorded[key] = _RecordedBindingTransfer(
                    analyzer,
                    result.facts.state_count,
                    result.facts.telemetry,
                    recorder.initializer_reads,
                    recorder.initializer_foreign_names,
                    recorder.initializer_empty_expiry,
                    tuple(recorder.jobs),
                )
            if checkpoints:
                self._recent[key] = tuple(checkpoints)
                large: list[tuple[str, PythonBindingFlowPolicy]] = []
                small_closed: list[tuple[str, PythonBindingFlowPolicy]] = []
                small_escaped: list[tuple[str, PythonBindingFlowPolicy]] = []
                for retained_key, retained in self._recent.items():
                    if retained[-1].completed_jobs >= 64:
                        large.append(retained_key)
                    elif retained[0].prefix._opaque_activation_sources:
                        small_escaped.append(retained_key)
                    else:
                        small_closed.append(retained_key)
                # Newly exposed modules and modules with existing escapes have
                # their own quota. Short captures cannot evict a large prefix.
                for evicted in (*large[:-4], *small_closed[:-8], *small_escaped[:-8]):
                    del self._recent[evicted]
                    self._recorded.pop(evicted, None)
            else:
                self._recorded.pop(key, None)
        return result

    def _retain_initializers(
        self,
        key: tuple[str, PythonBindingFlowPolicy],
        snapshots: tuple[_RebasableInitializer, ...],
    ) -> None:
        with self._lock:
            replaced = self._initializers.pop(key, ())
            self._initializer_extent -= sum(item.size for item in replaced)
            self._initializers[key] = snapshots
            self._initializer_extent += sum(item.size for item in snapshots)
            while self._initializer_extent > 65536 or len(self._initializers) > 128:
                _, evicted = self._initializers.popitem(last=False)
                self._initializer_extent -= sum(item.size for item in evicted)


_BINDING_REVISIONS: ContextVar[_BindingRevisions | None] = ContextVar(
    "_BINDING_REVISIONS", default=None
)


@contextmanager
def python_binding_revision_scope() -> Generator[None]:
    """Keep partial transfers only while discovery settles its input graph."""
    if _BINDING_REVISIONS.get() is not None:
        yield
        return
    token = _BINDING_REVISIONS.set(_BindingRevisions())
    try:
        yield
    finally:
        _BINDING_REVISIONS.reset(token)


def _project_binding_index(
    analysis: _BindingAnalysis,
    context: ModuleImportContext,
    tree: Callable[[], ast.Module],
    *,
    include_import_discovery: bool,
    ast_digest_admission: _PythonAstDigestAdmission | None,
) -> PythonBindingIndex:
    from molt.compiler_analysis.python_imports import (
        ModuleImportFlow,
        _analyze_module_import_flow_uncached,
        context_import_state,
    )

    facts = analysis.facts
    argument_plan = (
        call_argument_plan
        if ast_digest_admission is None
        else ast_digest_admission.call_argument_plan
    )
    source_key = (
        python_node_source_key
        if ast_digest_admission is None
        else ast_digest_admission.source_key
    )
    if include_import_discovery:
        # This dependency is acyclic: the strict projection never requests a
        # discovery projection. Both keys share the core's bounded single flight.
        strict = analysis.projections.get_or_compute(
            (context, False),
            lambda: _project_binding_index(
                analysis,
                context,
                tree,
                include_import_discovery=False,
                ast_digest_admission=ast_digest_admission,
            ),
        )
        flow = strict.module_import_flow
    elif analysis.module_import_flow_required:
        flow = _analyze_module_import_flow_uncached(
            tree(),
            context,
            statement_facts=facts._statement_lookup,
            iteration_facts=facts._iteration_lookup,
            expression_facts=facts._expression_lookup,
            assignment_effects=analysis.assignment_effects,
            call_facts=facts._call_lookup,
            argument_plan=argument_plan,
            source_key=source_key,
            ast_digest_admission=ast_digest_admission,
            callback_metadata_writes=facts.callback_metadata_writes,
        )
    else:
        state = context_import_state(context)
        flow = ModuleImportFlow({}, (state,), (state,))
    if include_import_discovery:
        # Reuse the cached strict projection and immutable fixpoint facts. Only
        # the source transfer is additional, and it is cached with the core.
        # Even an invariant lexical state can lose storage custody at a getter,
        # import, or deferred activation. The strict fast path needs no state
        # transfer; source completeness still needs the sparse demand proofs.
        source_flow = _analyze_module_import_flow_uncached(
            tree(),
            context,
            statement_facts=facts._statement_lookup,
            iteration_facts=facts._iteration_lookup,
            expression_facts=facts._expression_lookup,
            assignment_effects=analysis.assignment_effects,
            call_facts=facts._call_lookup,
            argument_plan=argument_plan,
            source_key=source_key,
            ast_digest_admission=ast_digest_admission,
            source_discovery=True,
            callback_metadata_writes=facts.callback_metadata_writes,
        )
        flow = replace(
            flow,
            source_states_by_node=source_flow.states_by_node,
            source_all_states=source_flow.all_states,
        )
    return PythonBindingIndex.from_facts(
        facts,
        module_name=context.module_name,
        module_spec_name=context.spec_name,
        module_is_package=context.is_package,
        module_execution_kind=context.execution_kind,
        module_import_flow=flow,
    )


@dataclass(slots=True)
class _PendingAnalysis[T]:
    ready: Event = field(default_factory=Event)
    result: T | None = None
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


class _BindingCache[T]:
    """Free-thread-safe single-flight cache with bounded FIFO completion eviction."""

    def __init__(self, max_entries: int = 128) -> None:
        self._max_entries = max_entries
        self._lock = RLock()
        self._ready: OrderedDict[tuple[object, ...], T] = OrderedDict()
        self._pending: dict[tuple[object, ...], _PendingAnalysis[T]] = {}
        self._computations = 0

    def computation_count(self) -> int:
        """Actual owner computations, including failed attempts, not fact reuse."""
        with self._lock:
            return self._computations

    def _remember_ready(self, key: tuple[object, ...], result: T) -> None:
        self._ready[key] = result
        while len(self._ready) > self._max_entries:
            self._ready.popitem(last=False)

    def get_or_compute(
        self,
        key: tuple[object, ...],
        compute: Callable[[], T],
        *,
        reuse: Callable[[tuple[object, ...], T], T | None] | None = None,
    ) -> T:
        owner = False
        with self._lock:
            cached = self._ready.get(key)
            if cached is not None:
                return cached
            pending = self._pending.get(key)
            if pending is None:
                if reuse is not None:
                    for prior_key, result in reversed(self._ready.items()):
                        if (revised := reuse(prior_key, result)) is not None:
                            self._remember_ready(key, revised)
                            return revised
                pending = _PendingAnalysis()
                self._pending[key] = pending
                self._computations += 1
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
            self._remember_ready(key, result)
            pending.result = result
            self._pending.pop(key, None)
            pending.ready.set()
        return result


_CORE_CACHE = _BindingCache[_BindingAnalysis]()


def python_binding_core_computations() -> int:
    """Process-lifetime count of fixpoints started, including failed attempts."""
    return _CORE_CACHE.computation_count()


def analyze_python_bindings(
    tree: ast.Module,
    *,
    source_digest: str,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
    policy: PythonBindingPolicy = PythonBindingPolicy(),
) -> PythonBindingIndex:
    """Analyze an AST through the canonical content-addressed index cache."""

    if ast_digest_admission is None:
        canonical_python_module(tree)
    else:
        if ast_digest_admission.digest != source_digest:
            raise ValueError("binding source digest does not match AST admission")
        ast_digest_admission.canonical_tree(tree)
    return _cached_binding_index(
        lambda: tree, source_digest, policy, ast_digest_admission=ast_digest_admission
    )


def _cached_binding_index(
    load_tree: Callable[[], ast.Module],
    source_digest: str,
    policy: PythonBindingPolicy,
    *,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> PythonBindingIndex:
    parsed: ast.Module | None = None

    def tree() -> ast.Module:
        nonlocal parsed
        if parsed is None:
            parsed = load_tree()
        return parsed

    analysis = _binding_analysis(
        tree,
        source_digest,
        policy.flow_policy(),
        ast_digest_admission=ast_digest_admission,
    )
    # Projections are owned by their core, so eviction cannot leave a second
    # global cache retaining orphaned facts. Each source retains at most eight
    # module contexts; computing either level uses the same single-flight law.
    # The core may already serve another flow policy with identical facts.
    # Projection reads only those facts, the context and its discovery mode.
    context = ModuleImportContext(
        module_name=policy.module_name,
        is_package=policy.module_is_package,
        spec_name=policy.module_spec_name,
        target_python=policy.target_python,
        execution_kind=policy.module_execution_kind,
    )
    include_import_discovery = policy.include_import_discovery
    return analysis.projections.get_or_compute(
        (context, include_import_discovery),
        lambda: _project_binding_index(
            analysis,
            context,
            tree,
            include_import_discovery=include_import_discovery,
            ast_digest_admission=ast_digest_admission,
        ),
    )


def _binding_analysis(
    tree: Callable[[], ast.Module],
    source_digest: str,
    policy: PythonBindingFlowPolicy,
    *,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
) -> _BindingAnalysis:
    if type(policy) is not PythonBindingFlowPolicy:
        raise TypeError("binding fixpoint requires an exact flow-only policy")
    key = (_ANALYSIS_SCHEMA, source_digest, policy)

    def reuse(
        prior_key: tuple[object, ...], result: _BindingAnalysis
    ) -> _BindingAnalysis | None:
        if prior_key[:2] != key[:2] or result.opaque_activation_reads is None:
            return None
        prior = cast(PythonBindingFlowPolicy, prior_key[2])
        opaque = policy.opaque_activation_sources
        assert opaque is not None
        # A deterministic transfer observes the same branches while every
        # consumed activation query agrees. Named additions may alter unused
        # namespace slots. All other input families keep their exact identity.
        prior = replace(prior, opaque_activation_sources=opaque)
        if not result.opaque_activation_reads.unchanged_under(frozenset(opaque)):
            return None
        if prior == policy:
            return result
        former, current = (
            prior.foreign_namespace_writes,
            policy.foreign_namespace_writes,
        )
        if (
            policy.foreign_namespace_universe is None
            or former is None
            or current is None
            or not frozenset(former).issubset(current)
            or replace(prior, foreign_namespace_writes=current) != policy
        ):
            return None
        return result.with_added_foreign_slots(frozenset(current).difference(former))

    return _CORE_CACHE.get_or_compute(
        key,
        lambda: _Analyzer(policy, source_digest).analyze(
            tree(), ast_digest_admission=ast_digest_admission
        ),
        reuse=reuse if policy.opaque_activation_sources is not None else None,
    )


def analyze_python_binding_facts(
    tree: ast.Module,
    *,
    source_digest: str,
    ast_digest_admission: _PythonAstDigestAdmission | None = None,
    policy: PythonBindingFlowPolicy = PythonBindingFlowPolicy(),
) -> PythonBindingFacts:
    """Query shared lexical facts without computing an unused import context."""
    if ast_digest_admission is None:
        canonical_python_module(tree)
    else:
        if ast_digest_admission.digest != source_digest:
            raise ValueError("binding source digest does not match AST admission")
        ast_digest_admission.canonical_tree(tree)
    return _binding_analysis(
        lambda: tree, source_digest, policy, ast_digest_admission=ast_digest_admission
    ).facts


def analyze_python_source_bindings(
    source: str,
    *,
    filename: str = "<unknown>",
    policy: PythonBindingPolicy = PythonBindingPolicy(),
) -> PythonBindingIndex:
    """Parse and analyze source through the deterministic single-flight cache."""

    digest = python_source_digest(source)
    return _cached_binding_index(
        lambda: canonical_python_module(
            ast.parse(source, filename=filename, feature_version=policy.target_python)
        ),
        digest,
        policy,
    )


__all__ = [
    "PythonBindingFlowPolicy",
    "PythonBindingPolicy",
    "analyze_python_binding_facts",
    "analyze_python_bindings",
    "analyze_python_source_bindings",
    "python_binding_core_computations",
    "python_canonical_member_writes",
    "python_dynamic_import_facts_required",
]
