from __future__ import annotations

from molt.compilation_target import Target
from molt.compiler_analysis.native_support_slice import ModuleBodySelection
from molt.compiler_analysis.python_imports import (
    SourceImportSite,
    ModuleSourceRole,
    SourceAllExports,
    _ImportScanRequests,
    _SourceImportBinding,
    _SourceValueDemand,
    _SourceValueSummary,
    expand_runtime_catalog,
)

import ast
import hashlib
import json
import os
from concurrent.futures import Future
from dataclasses import asdict, dataclass, field, replace
from pathlib import Path
from types import MappingProxyType
from typing import (
    Any,
    Callable,
    Collection,
    Literal,
    Mapping,
    NamedTuple,
    Sequence,
    TYPE_CHECKING,
)

from molt.compiler_analysis.python_source_keys import PythonSourceKey
from molt.backend_environment import CodegenSelection
from molt.cli.output import CliFailure as _CliFailure
from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.cli.cache_fingerprints import _operation_resolved
from molt.cli.python_import_topology import PythonImportTopology
from molt.native_callable_exports import (
    NativeCallableExport as _ExternalNativeCallableExport,
    NativeCallableExportError,
    normalize_native_callable_export,
)
from molt.target_python import TargetPythonVersion
from molt.type_facts import TypeFacts
from molt.toolchain_identity import StableRegularFileIdentity

if TYPE_CHECKING:
    from molt.tool_releases import ToolDiscovery
    from molt.backend_protocol import BackendEmissionOrigin
    from molt.cli.build_request import BuildProfile, TypeHintPolicy
    from molt.cli.link_input_observation import CompilationInputRecord
    from molt.cli.external_native import (
        _ExternalNativeAbiSymbol,
        _ExternalNativeCapiSymbol,
        _ExternalPackageNativeArtifact,
    )
    from molt.cli.frontend_execution import (
        _FrontendLayerExecutionContext,
        _FrontendLayerRuntimeHooks,
    )
    from molt.cli.frontend_parallel import _FrontendWorkerResourceDecision
    from molt.cli.runtime_native_build import _NativeRuntimeBuildFailure
    from molt.cli.runtime_wasm_failure import _RuntimeWasmBuildFailure
    from molt.cli.function_records import FunctionDigestSeed
    from molt.frontend.module_publication import FunctionAssemblyMetadata
    from molt.cli.wasm_codegen_layout import WasmCodegenLayout
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
    from molt.frontend.lowering.exception_lowering import (
        SyntaxErrorAttribute,
        SyntaxErrorName,
    )
    from molt.frontend import ClassId, ClassInfo
    from molt.cli.backend_artifact_contract import BackendArtifactContract
    from molt.capability_manifest import ResolvedRuntimePolicy
    from molt.cli.runtime_identity_schema import RuntimeBuildIdentity
    from molt.cli.runtime_wasm_generation import RuntimeWasmCodegenBinding
    from molt.cli.runtime_native_codegen import NativeRuntimeCodegenBinding
    from molt.cli.runtime_native_generation import NativeRuntimeGeneration
    from molt.cli.installed_runtime_contract import InstalledNativeAdmission
    from molt.cli.module_resolution import _ModuleResolutionCache
    from molt.cli.module_graph_discovery import _ModuleGraphDiscovery
    from molt.cli.module_source import _ModuleSourceCatalog, PythonSourceSnapshot
    from molt.cli.source_extension_link_requirements import (
        SourceExtensionLinkRequirements,
    )

ImportScanMode = Literal["full", "module_init"]
BinaryImageKind = Literal[
    "entry_script",
    "entry_module",
    "entry_package",
    "project_entry_script",
    "project_entry_module",
    "project_entry_package",
]
BinaryImageClosureMode = Literal["reachable_only"]
ModuleExecutionKind = Literal["imported", "module", "script"]
BuildEntrySelectorOrigin = Literal["cli", "config", "legacy"]
BuildEntrySelectorTarget = Literal["file", "module"]


@dataclass(frozen=True)
class ModuleSyntaxErrorInfo:
    """Parser constructor arguments plus observable state changed afterward.

    Parser args contain only immutable literals and an optional location
    details tuple. Sparse overrides preserve attributes not represented by args.
    """

    exception_name: SyntaxErrorName
    args: tuple[object, ...]
    attribute_overrides: tuple[tuple[SyntaxErrorAttribute, object], ...]


class _CanonicalPackagePaths(tuple[Path, ...]):
    """Resolve search roots once; immutable authority updates reuse the value."""

    __slots__ = ()

    def __new__(cls, paths: tuple[Path, ...]) -> _CanonicalPackagePaths:
        if isinstance(paths, cls):
            return paths
        return super().__new__(
            cls, dict.fromkeys(_operation_resolved(path) for path in paths)
        )


@dataclass(frozen=True)
class _ModuleSourceScanAuthority:
    """A scan's source path, captured bytes and depth, never source/AST storage.

    An absent SHA makes no byte-generation claim. A captured SHA alone does
    not establish a complete namespace-writer or target-execution proof.
    """

    module_name: str
    source_path: Path
    mode: ImportScanMode
    body_selection: ModuleBodySelection = field(kw_only=True)
    is_package: bool | None = None
    requires_runtime_package_anchor: bool = False
    # Some scan of this source reached an import or an import-capable call.
    uses_runtime_import_protocol: bool = False
    deferred_owners: frozenset[tuple[PythonSourceKey, tuple[str, ...]]] = frozenset()
    namespace_premise: PythonNamespacePremise | None = None
    source_sha256: str | None = None
    package_paths: tuple[Path, ...] = ()

    def __post_init__(self) -> None:
        if self.mode not in {"module_init", "full"}:
            raise ValueError(f"invalid module scan mode: {self.mode!r}")
        if self.source_sha256 is not None and (
            not isinstance(self.source_sha256, str)
            or len(self.source_sha256) != 64
            or any(c not in "0123456789abcdef" for c in self.source_sha256)
        ):
            raise ValueError("invalid source scan byte identity")
        object.__setattr__(self, "source_path", _operation_resolved(self.source_path))
        if self.is_package is None:
            object.__setattr__(
                self, "is_package", self.source_path.name == "__init__.py"
            )
        package_paths = (
            _CanonicalPackagePaths(self.package_paths) if self.package_paths else ()
        )
        if self.is_package:
            if not package_paths:
                if self.source_path.name != "__init__.py":
                    raise ValueError("generated package lacks original search paths")
                package_paths = _CanonicalPackagePaths((self.source_path.parent,))
        elif package_paths:
            raise ValueError("non-package source cannot own package search paths")
        object.__setattr__(self, "package_paths", package_paths)
        if not isinstance(self.body_selection, ModuleBodySelection):
            raise ValueError("source scan requires a module body selection")

    def validate(self, module_name: str, source_path: Path) -> None:
        if self.module_name != module_name or self.source_path != _operation_resolved(
            source_path
        ):
            raise ValueError(f"module scan lost source authority: {module_name!r}")

    def validate_generation(self, source_sha256: str | None) -> None:
        if self.source_sha256 is not None and source_sha256 != self.source_sha256:
            from molt.cli.module_source import PythonSourceChangedError

            raise PythonSourceChangedError(
                f"source scan generation changed: {self.module_name!r}; "
                f"source changed or became unavailable: {self.source_path}"
            )


@dataclass(frozen=True)
class _ModuleGraphScanAuthority:
    sources: tuple[_ModuleSourceScanAuthority, ...] = ()
    by_module: Mapping[str, _ModuleSourceScanAuthority] = field(
        init=False, compare=False, repr=False
    )

    def __post_init__(self) -> None:
        if len({source.module_name for source in self.sources}) != len(self.sources):
            raise ValueError("module scan authority requires unique module identities")
        object.__setattr__(
            self,
            "by_module",
            MappingProxyType({source.module_name: source for source in self.sources}),
        )

    def mode_for(self, module_name: str, source_path: Path) -> ImportScanMode:
        source = self.by_module.get(module_name)
        if source is None:
            raise ValueError(f"module lacks source scan authority: {module_name!r}")
        source.validate(module_name, source_path)
        return source.mode

    def merged(self, other: "_ModuleGraphScanAuthority") -> "_ModuleGraphScanAuthority":
        from molt.compiler_analysis.python_namespace_graph import (
            join_python_namespace_premises,
        )

        sources = {source.module_name: source for source in self.sources}
        ranks = {"module_init": 0, "full": 1}
        for source in other.sources:
            prior = sources.get(source.module_name)
            if prior is not None:
                prior.validate(source.module_name, source.source_path)
                if prior.is_package != source.is_package:
                    raise ValueError(
                        f"module package scan authority changed: {source.module_name!r}"
                    )
                if prior.package_paths != source.package_paths:
                    raise ValueError(
                        f"module package search paths changed: {source.module_name!r}"
                    )
                if prior.source_sha256 is not None and source.source_sha256 is not None:
                    prior.validate_generation(source.source_sha256)
                source_sha256 = prior.source_sha256 or source.source_sha256
                body_selection = (
                    ModuleBodySelection("complete_source")
                    if (
                        prior.body_selection.kind == "complete_source"
                        and prior.source_sha256 is not None
                        or source.body_selection.kind == "complete_source"
                        and source.source_sha256 is not None
                    )
                    else ModuleBodySelection.from_requested_roots(
                        set(prior.body_selection.roots)
                        | set(source.body_selection.roots)
                    )
                )
                requires_runtime_package_anchor = (
                    prior.requires_runtime_package_anchor
                    or source.requires_runtime_package_anchor
                )
                uses_runtime_import_protocol = (
                    prior.uses_runtime_import_protocol
                    or source.uses_runtime_import_protocol
                )
                deferred_owners = prior.deferred_owners | source.deferred_owners
                namespace_premise = join_python_namespace_premises(
                    prior.namespace_premise, source.namespace_premise
                )
                if ranks[prior.mode] >= ranks[source.mode]:
                    source = prior
                if (
                    deferred_owners != source.deferred_owners
                    or requires_runtime_package_anchor
                    != source.requires_runtime_package_anchor
                    or uses_runtime_import_protocol
                    != source.uses_runtime_import_protocol
                    or namespace_premise != source.namespace_premise
                    or source_sha256 != source.source_sha256
                    or body_selection != source.body_selection
                ):
                    source = replace(
                        source,
                        requires_runtime_package_anchor=requires_runtime_package_anchor,
                        uses_runtime_import_protocol=uses_runtime_import_protocol,
                        deferred_owners=deferred_owners,
                        namespace_premise=namespace_premise,
                        source_sha256=source_sha256,
                        body_selection=body_selection,
                    )
            sources[source.module_name] = source
        return _ModuleGraphScanAuthority(
            tuple(sources[name] for name in sorted(sources))
        )

    def restricted(self, graph: Mapping[str, Path]) -> "_ModuleGraphScanAuthority":
        sources = tuple(
            source for source in self.sources if source.module_name in graph
        )
        for source in sources:
            source.validate(source.module_name, graph[source.module_name])
        return _ModuleGraphScanAuthority(sources)

    def validate_graph(self, graph: Mapping[str, Path]) -> None:
        self.restricted(graph)
        if {source.module_name for source in self.sources} != set(graph):
            raise ValueError("source graph and scan authority differ")

    def validate_source_generations(self) -> None:
        """Fence all admitted bytes, including foreign namespace writers."""
        from molt.cli.module_source import _source_content_sha256

        for source in self.sources:
            if source.source_sha256 is not None:
                source.validate_generation(_source_content_sha256(source.source_path))

    def payload(
        self, logical_paths: Mapping[str, str] | None = None
    ) -> list[dict[str, Any]]:
        return [
            {
                "module": source.module_name,
                "path": (logical_paths or {}).get(
                    source.module_name, os.fspath(source.source_path)
                ),
                "mode": source.mode,
                "source_sha256": source.source_sha256,
                "deferred_bodies": [
                    {"source": list(owner), "arguments": list(arguments)}
                    for owner, arguments in sorted(source.deferred_owners)
                ],
                "is_package": source.is_package,
                "package_paths": [os.fspath(path) for path in source.package_paths],
                "body_selection": source.body_selection.payload(),
                "requires_runtime_package_anchor": (
                    source.requires_runtime_package_anchor
                ),
                "uses_runtime_import_protocol": source.uses_runtime_import_protocol,
                "namespace_premise": (
                    None
                    if source.namespace_premise is None
                    else asdict(source.namespace_premise)
                ),
            }
            for source in sorted(self.sources, key=lambda source: source.module_name)
        ]


class _CompleteImportScan(NamedTuple):
    source_requests: _ImportScanRequests
    imports: tuple[str, ...]
    source_executions: tuple[tuple[str | None, Path], ...]
    dynamic_relative_import_candidates: tuple[str, ...] = ()
    requires_runtime_package_anchor: bool = False
    source_demands: tuple[_SourceValueDemand, ...] = ()
    source_bindings: tuple[_SourceImportBinding, ...] = ()
    source_values: _SourceValueSummary = _SourceValueSummary()
    source_sites: tuple[SourceImportSite, ...] = ()
    star_exports: tuple[tuple[str, SourceAllExports], ...] = ()


@dataclass(frozen=True)
class _PrecomputedModuleImportScan:
    """Both projections from one identified source and one scan mode."""

    authority: _ModuleSourceScanAuthority
    target_python_tag: str
    target_sys_platform: str | None
    scan: _CompleteImportScan

    def __post_init__(self) -> None:
        if self.authority.source_sha256 is None:
            raise ValueError("precomputed source scan requires captured byte identity")


@dataclass(frozen=True)
class PgoProfileSummary:
    version: str
    hash: str
    hot_functions: list[str]
    branch_counts: dict[str, dict[str, int]] | None = None
    call_counts: dict[str, int] | None = None
    loop_counts: dict[str, dict[str, float | int]] | None = None


@dataclass(frozen=True)
class RuntimeFeedbackSummary:
    schema_version: int
    hash: str
    hot_functions: list[str]


@dataclass(frozen=True)
class _ScopedLoweringInputs:
    known_modules_by_module: dict[str, tuple[str, ...]]
    direct_call_modules_by_module: dict[str, tuple[str, ...]]
    known_func_defaults_by_module: dict[str, dict[str, dict[str, Any]]]
    known_func_kinds_by_module: dict[str, dict[str, dict[str, str]]]
    native_callable_exports_by_module: dict[str, dict[str, dict[str, Any]]]
    native_python_exports_by_module: dict[str, tuple[str, ...]]
    native_support_function_roots_by_module: dict[str, tuple[str, ...]]
    pgo_hot_function_names_by_module: dict[str, tuple[str, ...]]
    type_facts_by_module: dict[str, TypeFacts | None]


@dataclass(frozen=True)
class _ScopedLoweringInputView:
    known_modules: tuple[str, ...]
    known_func_defaults: dict[str, dict[str, Any]]
    known_func_kinds: dict[str, dict[str, str]]
    pgo_hot_function_names: tuple[str, ...]
    type_facts: TypeFacts | None
    direct_call_modules: tuple[str, ...] = ()
    native_callable_exports: dict[str, dict[str, Any]] = field(default_factory=dict)
    native_python_exports: tuple[str, ...] = ()
    body_selection: ModuleBodySelection = field(kw_only=True)
    known_modules_payload: list[str] = field(default_factory=list)
    known_modules_set: frozenset[str] = field(default_factory=frozenset)
    direct_call_modules_payload: list[str] = field(default_factory=list)
    direct_call_modules_set: frozenset[str] = field(default_factory=frozenset)
    native_callable_exports_payload: dict[str, dict[str, Any]] = field(
        default_factory=dict
    )
    native_python_exports_payload: list[str] = field(default_factory=list)
    native_python_exports_set: frozenset[str] = field(default_factory=frozenset)
    pgo_hot_function_names_payload: list[str] = field(default_factory=list)
    pgo_hot_function_names_set: frozenset[str] = field(default_factory=frozenset)

    def __post_init__(self) -> None:
        if not self.known_modules_payload and self.known_modules:
            object.__setattr__(self, "known_modules_payload", list(self.known_modules))
        if not self.known_modules_set and self.known_modules:
            object.__setattr__(self, "known_modules_set", frozenset(self.known_modules))
        if not self.direct_call_modules_payload and self.direct_call_modules:
            object.__setattr__(
                self,
                "direct_call_modules_payload",
                list(self.direct_call_modules),
            )
        if not self.direct_call_modules_set and self.direct_call_modules:
            object.__setattr__(
                self,
                "direct_call_modules_set",
                frozenset(self.direct_call_modules),
            )
        if not self.native_callable_exports_payload and self.native_callable_exports:
            object.__setattr__(
                self,
                "native_callable_exports_payload",
                {
                    name: dict(self.native_callable_exports[name])
                    for name in sorted(self.native_callable_exports)
                },
            )
        if not self.native_python_exports_payload and self.native_python_exports:
            object.__setattr__(
                self,
                "native_python_exports_payload",
                list(self.native_python_exports),
            )
        if not self.native_python_exports_set and self.native_python_exports:
            object.__setattr__(
                self,
                "native_python_exports_set",
                frozenset(self.native_python_exports),
            )
        if not isinstance(self.body_selection, ModuleBodySelection):
            raise ValueError("lowering requires a module body selection")
        if not self.pgo_hot_function_names_payload and self.pgo_hot_function_names:
            object.__setattr__(
                self,
                "pgo_hot_function_names_payload",
                list(self.pgo_hot_function_names),
            )
        if not self.pgo_hot_function_names_set and self.pgo_hot_function_names:
            object.__setattr__(
                self,
                "pgo_hot_function_names_set",
                frozenset(self.pgo_hot_function_names),
            )


@dataclass(frozen=True)
class _ModuleGraphMetadata:
    scan_authority: _ModuleGraphScanAuthority | None
    logical_source_path_by_module: Mapping[str, str]
    namespace_portions_by_module: Mapping[str, tuple[str, ...]]
    module_is_package_by_module: Mapping[str, bool]
    module_execution_kind_by_module: Mapping[str, ModuleExecutionKind]
    module_source_role_by_module: Mapping[str, ModuleSourceRole]
    frontend_module_costs: Mapping[str, float] | None
    stdlib_like_by_module: Mapping[str, bool] | None


@dataclass(frozen=True)
class _BinaryImageScope:
    kind: BinaryImageKind
    selector_source: str
    entry_module: str
    source_path: Path
    project_root: Path
    module_roots: tuple[Path, ...]
    root_modules: tuple[str, ...] = ()
    closure_mode: BinaryImageClosureMode = "reachable_only"

    def __post_init__(self) -> None:
        object.__setattr__(self, "source_path", self.source_path.resolve())
        object.__setattr__(self, "project_root", self.project_root.resolve())
        object.__setattr__(
            self,
            "module_roots",
            tuple(dict.fromkeys(root.resolve() for root in self.module_roots)),
        )
        if not self.root_modules:
            object.__setattr__(self, "root_modules", (self.entry_module,))
        else:
            object.__setattr__(
                self,
                "root_modules",
                tuple(dict.fromkeys(self.root_modules)),
            )

    @classmethod
    def from_entry(
        cls,
        *,
        kind: BinaryImageKind,
        selector_source: str,
        entry_module: str,
        source_path: Path,
        project_root: Path,
        module_roots: Sequence[Path],
    ) -> "_BinaryImageScope":
        return cls(
            kind=kind,
            selector_source=selector_source,
            entry_module=entry_module,
            source_path=source_path,
            project_root=project_root,
            module_roots=tuple(module_roots),
            root_modules=(entry_module,),
        )

    def diagnostic_payload(self) -> dict[str, Any]:
        return {
            "kind": self.kind,
            "selector_source": self.selector_source,
            "entry_module": self.entry_module,
            "source_path": os.fspath(self.source_path),
            "project_root": os.fspath(self.project_root),
            "module_roots": [os.fspath(root) for root in self.module_roots],
            "root_modules": list(self.root_modules),
            "closure_mode": self.closure_mode,
        }

    @property
    def entry_execution_kind(self) -> ModuleExecutionKind:
        return "script" if self.kind.endswith("entry_script") else "module"

    def with_root_modules(
        self,
        root_modules: Collection[str],
    ) -> "_BinaryImageScope":
        roots = tuple(dict.fromkeys(root_modules))
        if not roots:
            roots = (self.entry_module,)
        if roots == self.root_modules:
            return self
        return _BinaryImageScope(
            kind=self.kind,
            selector_source=self.selector_source,
            entry_module=self.entry_module,
            source_path=self.source_path,
            project_root=self.project_root,
            module_roots=self.module_roots,
            root_modules=roots,
            closure_mode=self.closure_mode,
        )


@dataclass(frozen=True)
class _FrontendModuleResultTimings:
    visit_s: float
    lower_s: float
    total_s: float
    cache_hit: bool = False
    reused_s: float = 0.0


@dataclass
class _FrontendParallelLayerState:
    results: dict[str, dict[str, Any]] = field(default_factory=dict)
    context_digests: dict[str, str] = field(default_factory=dict)
    worker_timings_by_module: dict[str, dict[str, Any]] = field(default_factory=dict)
    recorded_worker_timings: list[dict[str, Any]] = field(default_factory=list)
    fallback_reason: str | None = None


@dataclass(frozen=True)
class _FrontendParallelConfig:
    workers: int
    min_modules: int
    min_predicted_cost: float
    target_cost_per_worker: float
    stdlib_min_cost_scale: float
    enabled: bool
    reason: str
    worker_resources: _FrontendWorkerResourceDecision


@dataclass(frozen=True)
class _FrontendLayerPlan:
    candidates: tuple[str, ...]
    predicted_cost_total: float
    effective_min_predicted_cost: float
    stdlib_candidates: int
    workers: int
    policy_reason: str
    mode: str


@dataclass(frozen=True)
class _SerialFrontendLoweringContext:
    syntax_error_modules: Mapping[str, "ModuleSyntaxErrorInfo"]
    module_source_catalog: _ModuleSourceCatalog
    project_root: Path | None
    type_hint_policy: "TypeHintPolicy"
    type_facts: TypeFacts | None
    enable_phi: bool
    known_modules: Collection[str]
    direct_call_modules: Collection[str]
    stdlib_allowlist: Collection[str]
    known_func_defaults: dict[str, dict[str, dict[str, Any]]]
    known_func_kinds: dict[str, dict[str, str]]
    native_callable_exports: Mapping[str, Mapping[str, Any]]
    native_python_exports: Collection[str]
    module_deps: dict[str, set[str]]
    source_modules: Collection[str]
    module_chunking: bool
    module_chunk_max_ops: int
    optimization_profile: str
    pgo_hot_function_names: Collection[str]
    known_modules_sorted: tuple[str, ...]
    stdlib_allowlist_sorted: tuple[str, ...]
    pgo_hot_function_names_sorted: tuple[str, ...]
    module_dep_closures: dict[str, frozenset[str]]
    scoped_lowering_inputs: "_ScopedLoweringInputs | None"
    module_graph_metadata: "_ModuleGraphMetadata"
    known_classes: Mapping[ClassId, ClassInfo]
    frontend_phase_timeout: float | None
    target_python: TargetPythonVersion
    target_sys_platform: str | None


@dataclass(frozen=True)
class _SerialFrontendLoweringHooks:
    record_frontend_timing: Callable[..., None]
    fail: Callable[..., _CliFailure]
    json_output: bool


@dataclass
class _FrontendIntegrationState:
    functions: list[dict[str, Any]]
    known_classes: dict[ClassId, ClassInfo]
    function_digest_seeds: list[FunctionDigestSeed] = field(default_factory=list)
    global_code_ids: dict[str, int] = field(default_factory=dict)
    function_metadata: dict[str, FunctionAssemblyMetadata] = field(default_factory=dict)
    # HF-220: the module that owns each class statement symbol, and the first
    # module whose stamp or layout guard names each symbol.
    class_definition_owners: dict[str, str] = field(default_factory=dict)
    class_definition_references: dict[str, str] = field(default_factory=dict)


@dataclass
class _MidendDiagnosticsState:
    policy_outcomes_by_function: dict[str, dict[str, Any]]
    pass_stats_by_function: dict[str, dict[str, dict[str, Any]]]


@dataclass
class _RuntimeArtifactState:
    runtime_lib: Path | None = None
    # Fixed names are private build coordinates. Internal consumers use the
    # immutable members selected by runtime_wasm_generation.
    runtime_wasm: Path | None = None
    runtime_reloc_wasm: Path | None = None
    runtime_wasm_generation: Path | None = None
    runtime_wasm_selected: Path | None = None
    runtime_reloc_wasm_selected: Path | None = None
    runtime_wasm_expected_identity: Path | None = None
    runtime_wasm_codegen_binding: RuntimeWasmCodegenBinding | None = None
    # One build operation's prerequisite discovery. Structural validation still
    # reattests the executable at each actual execution boundary.
    runtime_wasm_validator: ToolDiscovery | None = None
    extra_runtime_features: tuple[str, ...] = ()
    # The build's Rust CPU policy for the native runtime Cargo plan. WASM
    # runtimes always compile for their fixed baseline.
    rust_cpu: RustCpuPolicy = RustCpuPolicy.BASELINE
    native_runtime_build_identity: RuntimeBuildIdentity | None = None
    native_runtime_generation: NativeRuntimeGeneration | None = None
    native_runtime_codegen_binding: NativeRuntimeCodegenBinding | None = None
    # One build operation's installed-cell admission; codegen and final link
    # reuse it through its fences. None for source-checkout runtimes.
    installed_native_admission: InstalledNativeAdmission | None = None
    native_runtime_build_failure: _NativeRuntimeBuildFailure | None = None
    runtime_wasm_build_failure: _RuntimeWasmBuildFailure | None = None
    runtime_lib_ready_future: Future[bool] | None = None

    def revoke_native_runtime_admission(self) -> None:
        """Revoke all authority to use this operation's native generation."""
        self.native_runtime_codegen_binding = None
        self.native_runtime_build_identity = None
        self.native_runtime_generation = None
        self.installed_native_admission = None


_SharedStdlibCacheValidationToken = tuple[str, tuple[StableRegularFileIdentity, ...]]


@dataclass(frozen=True)
class _BackendCacheSetup:
    backend_environment: Mapping[str, str]
    artifact_contract: BackendArtifactContract
    cache_enabled: bool
    cache_key: str | None
    function_cache_key: str | None
    cache_path: Path | None
    function_cache_path: Path | None
    stdlib_object_path: Path | None
    stdlib_object_cache_key: str | None
    cache_candidates: tuple[tuple[str, Path], ...]
    cache_hit: bool
    cache_hit_tier: str | None
    stdlib_object_manifest: str | None = None
    stdlib_module_symbols_json: str | None = None
    stdlib_module_symbols: frozenset[str] = field(default_factory=frozenset)
    stdlib_contract_validation_token: _SharedStdlibCacheValidationToken | None = None
    stdlib_cache_entry_path: Path | None = None
    wasm_layout: WasmCodegenLayout | None = None
    backend_emission_origin: BackendEmissionOrigin | None = None


@dataclass(frozen=True)
class _BuildDiagnosticsContext:
    diagnostics_enabled: bool
    diagnostics_start: float
    phase_starts: Mapping[str, float]
    image_scope: _BinaryImageScope | None
    binary_image_closure: Mapping[str, Any] | None
    binary_image_analysis: Mapping[str, Any] | None
    module_graph: Mapping[str, Path]
    module_graph_operation_counts: Mapping[str, int]
    module_reasons: Mapping[str, set[str]]
    frontend_module_timings: Sequence[dict[str, Any]]
    allocation_diagnostics_enabled: bool
    frontend_parallel_details: Mapping[str, Any]
    profile: str
    midend_policy_outcomes_by_function: Mapping[str, dict[str, Any]]
    midend_pass_stats_by_function: Mapping[str, dict[str, dict[str, Any]]]
    backend_daemon_health: Mapping[str, Any] | None
    backend_daemon_cached: bool | None
    backend_daemon_cache_tier: str | None
    backend_daemon_config_digest: str | None
    diagnostics_path_spec: str | None
    artifacts_root: Path
    entry_module: str
    entry_path: Path


@dataclass(frozen=True)
class _FrontendTimingRecorderConfig:
    enabled: bool
    raw: bool
    threshold: float
    json_output: bool


@dataclass(frozen=True)
class _RuntimeImportSupportPolicy:
    needs_runtime_import_support: bool


@dataclass(frozen=True)
class _RuntimeImportScanCustody:
    """Source-backed finite catalog for runtime protocol implementation scans.

    Owners may defer dynamic package selection to the finite compiled catalog.
    Every row must be retained as a dispatch root by the graph producer, whether
    emitted as registry data or source dispatch. This is neither permission to
    discover arbitrary modules nor proof of a backend's import capabilities.
    """

    owners: tuple[tuple[str, Path], ...]
    catalog: tuple[tuple[str, Path], ...]
    owner_ast_digests: tuple[tuple[str, str], ...]
    catalog_by_module: Mapping[str, Path] = field(init=False, compare=False, repr=False)
    owners_by_module: Mapping[str, Path] = field(init=False, compare=False, repr=False)
    owner_digests_by_module: Mapping[str, str] = field(
        init=False, compare=False, repr=False
    )
    modules: tuple[str, ...] = field(init=False, compare=False)

    def __post_init__(self) -> None:
        catalog = dict(self.catalog)
        if not catalog or len(catalog) != len(self.catalog):
            raise ValueError(
                "runtime import custody requires a nonempty unique catalog"
            )
        self._validate_sources()
        if not self.owners or len(dict(self.owners)) != len(self.owners):
            raise ValueError("runtime import custody requires unique source owners")
        for name, path in self.owners:
            if catalog.get(name) != path:
                raise ValueError(f"runtime import owner is outside catalog: {name!r}")
        digests = dict(self.owner_ast_digests)
        if set(digests) != set(dict(self.owners)) or any(
            not digest for digest in digests.values()
        ):
            raise ValueError(
                "runtime import custody requires each owner's source AST digest"
            )
        object.__setattr__(self, "catalog_by_module", MappingProxyType(catalog))
        object.__setattr__(
            self, "owners_by_module", MappingProxyType(dict(self.owners))
        )
        object.__setattr__(self, "owner_digests_by_module", MappingProxyType(digests))
        object.__setattr__(self, "modules", tuple(catalog))

    def owns(self, module_name: str | None, source_path: Path | None) -> bool:
        return (
            source_path is not None
            and self.owners_by_module.get(module_name) == source_path
        )

    def validate_scan_mode(
        self,
        module_name: str | None,
        source_path: Path | None,
        mode: ImportScanMode,
    ) -> None:
        if self.owns(module_name, source_path) and mode != "full":
            raise ValueError("runtime import custody requires full-depth owner scans")

    def admits_scan(
        self, module_name: str | None, source_path: Path | None, ast_digest: str | None
    ) -> bool:
        if not self.owns(module_name, source_path):
            return False
        if ast_digest != self.owner_digests_by_module.get(module_name):
            raise ValueError(
                f"runtime import custody source AST changed: {module_name!r}"
            )
        return True

    def complete_owner_scan(
        self, module_name: str | None, source_path: Path, scan: _ImportScanRequests
    ) -> _ImportScanRequests:
        """Admit an owner's source-pure scan, then append this live catalog.

        The scan names the AST it was made from; only this custody's own owner
        source is admitted, whether the scan is fresh or persisted.
        """
        deferral = scan.runtime_catalog
        if deferral is None:
            raise ValueError(
                f"runtime import owner scan has no catalog deferral: {module_name!r}"
            )
        if not self.admits_scan(module_name, source_path, deferral.source_ast_digest):
            raise ValueError(f"runtime import custody does not own {module_name!r}")
        return expand_runtime_catalog(scan, self.modules)

    def validate_graph(self, graph: Mapping[str, Path]) -> None:
        self._validate_sources()
        for name, path in self.catalog:
            actual = graph.get(name)
            if actual is None or _operation_resolved(actual) != path:
                raise ValueError(
                    f"runtime import catalog lost source authority: {name!r}"
                )

    def _validate_sources(self) -> None:
        for name, path in self.catalog:
            if not name or not path.is_absolute() or not path.is_file():
                raise ValueError(f"runtime import custody requires a source: {name!r}")


@dataclass(frozen=True)
class _ExternalNativeModuleAttrPublishSpec:
    provider_module: str
    attr: str


@dataclass(frozen=True)
class _ExternalNativeModuleInitSpec:
    module: str
    init_symbol: str = ""
    module_attr_exports: tuple[_ExternalNativeModuleAttrPublishSpec, ...] = ()
    direct_symbol_exports: tuple[_ExternalNativeCallableExport, ...] = ()
    alias_of: str = ""

    @property
    def is_extension(self) -> bool:
        return bool(self.init_symbol)

    @property
    def is_alias(self) -> bool:
        return bool(self.alias_of)


@dataclass(frozen=True)
class _ExternalPackageNativeArtifactPlan:
    artifacts: tuple[_ExternalPackageNativeArtifact, ...] = ()

    def digest_payload(self) -> dict[str, Any]:
        return {"artifacts": [artifact.digest_payload() for artifact in self.artifacts]}

    def runtime_export_symbols(self) -> frozenset[str]:
        """Exact runtime symbols required by the plan's native object closure.

        This is a property of the admitted package plan, not of its later
        staging location.  Keeping it here lets runtime build identity close
        before backend code generation needs the executable runtime layout and
        prevents the staged-artifact projection from becoming a second symbol
        authority.
        """
        symbols: set[str] = set()
        for artifact in self.artifacts:
            symbols.update(
                symbol.symbol
                for symbol in artifact.c_api_symbols
                if symbol.status == "cpython_abi_link"
            )
            symbols.update(
                symbol.symbol
                for symbol in artifact.abi_symbols
                if (symbol.status, symbol.primitive_class)
                in {
                    ("external_link", "molt_cpython_abi_link_import"),
                    ("runtime_backed", "wasm_runtime_import"),
                }
            )
        return frozenset(symbols)

    @staticmethod
    def _module_prefixes(name: str) -> set[str]:
        parts = name.split(".")
        return {".".join(parts[:idx]) for idx in range(1, len(parts) + 1)}

    @staticmethod
    def _capsule_module_name(capsule: str) -> str | None:
        parts = tuple(part for part in capsule.split(".") if part)
        if len(parts) < 2:
            return None
        return ".".join(parts[:-1])

    @classmethod
    def _artifact_capsule_alias_module_names(
        cls,
        artifact: _ExternalPackageNativeArtifact,
    ) -> tuple[str, ...]:
        alias_modules = {
            module_name
            for capsule in artifact.provided_capsules
            for module_name in (cls._capsule_module_name(capsule),)
            if module_name is not None and module_name != artifact.module
        }
        return tuple(sorted(alias_modules))

    @staticmethod
    def _support_init_module_names(
        artifact: _ExternalPackageNativeArtifact,
    ) -> set[str]:
        names: set[str] = set()
        for rel_path, _digest in artifact.support_file_sha256:
            parts = rel_path.replace("\\", "/").split("/")
            if not parts or parts[-1] != "__init__.py":
                continue
            module_parts = parts[:-1]
            if module_parts:
                names.add(".".join(module_parts))
        return names

    @staticmethod
    def _support_python_module_names(
        artifact: _ExternalPackageNativeArtifact,
    ) -> set[str]:
        names: set[str] = set()
        for rel_path, _digest in artifact.support_file_sha256:
            module_name = (
                _ExternalPackageNativeArtifactPlan._support_python_module_name(rel_path)
            )
            if module_name is not None:
                names.add(module_name)
        return names

    @staticmethod
    def _support_python_module_name(
        rel_path: str,
        *,
        include_init: bool = False,
    ) -> str | None:
        normalized = rel_path.replace("\\", "/")
        if not normalized.endswith(".py"):
            return None
        parts = normalized.split("/")
        if not parts:
            return None
        if parts[-1] == "__init__.py":
            if not include_init:
                return None
            module_parts = parts[:-1]
        else:
            module_parts = [*parts[:-1], parts[-1][:-3]]
        if not module_parts:
            return None
        return ".".join(module_parts)

    @classmethod
    def _artifact_base_module_names(
        cls,
        artifact: _ExternalPackageNativeArtifact,
    ) -> tuple[set[str], set[str]]:
        support_init_modules = cls._support_init_module_names(artifact)
        names = set(cls._module_prefixes(artifact.package))
        names.update(cls._module_prefixes(artifact.module))
        names.update(support_init_modules)
        for module_name in cls._artifact_capsule_alias_module_names(artifact):
            names.update(cls._module_prefixes(module_name))
        for module_name in cls._support_python_module_names(artifact):
            names.update(cls._module_prefixes(module_name))
        return names, support_init_modules

    @staticmethod
    def _package_source_root(
        package_dir: Path,
        package: str,
    ) -> Path:
        package_source_root = package_dir.resolve()
        package_parts = tuple(part for part in package.split(".") if part)
        if (
            package_parts
            and len(package_source_root.parts) >= len(package_parts)
            and tuple(package_source_root.parts[-len(package_parts) :]) == package_parts
        ):
            package_source_root = package_source_root.parents[len(package_parts) - 1]
        return package_source_root

    def package_source_roots(self) -> tuple[Path, ...]:
        roots = {
            self._package_source_root(artifact.package_dir, artifact.package)
            for artifact in self.artifacts
        }
        return tuple(sorted(roots))

    def support_source_paths_by_module(self) -> dict[str, Path]:
        sources: dict[str, Path] = {}
        runtime_import_modules = self.runtime_python_import_module_names()
        for artifact in self.artifacts:
            package_source_root = self._package_source_root(
                artifact.package_dir,
                artifact.package,
            )
            for rel_path, _digest in artifact.support_file_sha256:
                module_name = self._support_python_module_name(rel_path)
                if module_name is None:
                    init_module_name = self._support_python_module_name(
                        rel_path,
                        include_init=True,
                    )
                    module_name = (
                        init_module_name
                        if init_module_name in runtime_import_modules
                        else None
                    )
                if module_name is not None:
                    sources[module_name] = (
                        package_source_root / Path(rel_path.replace("\\", "/"))
                    ).resolve()
        return {module: sources[module] for module in sorted(sources)}

    def support_source_module_names(self) -> frozenset[str]:
        return frozenset(self.support_source_paths_by_module())

    def runtime_python_imports(self) -> frozenset[str]:
        return frozenset(
            name
            for artifact in self.artifacts
            for name in artifact.runtime_python_imports
        )

    def runtime_python_import_module_names(self) -> frozenset[str]:
        return frozenset(
            name
            for artifact in self.artifacts
            for name in artifact.runtime_python_import_modules
        )

    def native_module_names(self) -> frozenset[str]:
        names: set[str] = set()
        for artifact in self.artifacts:
            base_names, support_init_modules = self._artifact_base_module_names(
                artifact
            )
            names.update(base_names)
            for exported_name in artifact.python_exports:
                parts = exported_name.split(".")
                names.update(".".join(parts[:idx]) for idx in range(1, len(parts)))
                if (
                    exported_name == artifact.package
                    or exported_name == artifact.module
                    or exported_name in support_init_modules
                ):
                    names.add(exported_name)
            exported_names = (
                export.qualified_name for export in artifact.callable_exports
            )
            for exported_name in exported_names:
                parts = exported_name.split(".")
                names.update(".".join(parts[:idx]) for idx in range(1, len(parts)))
            for export in artifact.callable_exports:
                if export.provider_module is not None:
                    names.update(self._module_prefixes(export.provider_module))
        return frozenset(names)

    def native_callable_export_names(self) -> frozenset[str]:
        return frozenset(
            export.qualified_name
            for artifact in self.artifacts
            for export in artifact.callable_exports
        )

    def native_python_export_names(self) -> frozenset[str]:
        return frozenset(
            export_name
            for artifact in self.artifacts
            for export_name in artifact.python_exports
        )

    def native_callable_exports_by_qualified_name(self) -> dict[str, dict[str, Any]]:
        exports, _conflicts = self._merged_native_callable_exports()
        return {
            export.qualified_name: export.digest_payload()
            for _origin_module, export in exports
        }

    @staticmethod
    def _native_callable_contract_display(
        export: _ExternalNativeCallableExport,
        *,
        effective_provider: str | None,
    ) -> str:
        return (
            f"binding={export.binding}, abi={export.abi}, "
            f"symbol={export.symbol!r}, provider={effective_provider!r}, "
            f"arity={export.arity!r}, effects={export.effects!r}, "
            f"deterministic={export.deterministic!r}"
        )

    def _merged_native_callable_exports(
        self,
    ) -> tuple[
        tuple[tuple[str, _ExternalNativeCallableExport], ...],
        tuple[str, ...],
    ]:
        qualified_contracts: dict[
            str,
            tuple[
                tuple[object, ...],
                str,
                str,
                _ExternalNativeCallableExport,
            ],
        ] = {}
        conflicts: list[str] = []
        for artifact in self.artifacts:
            for export in artifact.callable_exports:
                try:
                    normalized = normalize_native_callable_export(
                        export.digest_payload(),
                        qualified_name=export.qualified_name,
                    )
                except NativeCallableExportError as exc:
                    conflicts.append(
                        f"native callable export {export.qualified_name!r} is invalid: {exc}"
                    )
                    continue
                effective_provider = (
                    normalized.provider_module or artifact.module
                    if normalized.binding == "module_attr"
                    else None
                )
                contract = (
                    normalized.binding,
                    normalized.abi,
                    normalized.symbol,
                    effective_provider,
                    normalized.arity,
                    normalized.effects,
                    normalized.deterministic,
                )
                origin = f"{artifact.module} ({artifact.manifest_path})"
                prior = qualified_contracts.get(normalized.qualified_name)
                if prior is None:
                    qualified_contracts[normalized.qualified_name] = (
                        contract,
                        origin,
                        artifact.module,
                        normalized,
                    )
                elif prior[0] != contract:
                    prior_export = prior[3]
                    prior_effective_provider = (
                        prior_export.provider_module or prior[2]
                        if prior_export.binding == "module_attr"
                        else None
                    )
                    prior_display = self._native_callable_contract_display(
                        prior_export,
                        effective_provider=prior_effective_provider,
                    )
                    current_display = self._native_callable_contract_display(
                        normalized,
                        effective_provider=effective_provider,
                    )
                    conflicts.append(
                        f"native callable export {normalized.qualified_name!r} has "
                        f"conflicting declarations: {prior[1]} "
                        f"({prior_display}) and {origin} ({current_display})"
                    )

        symbol_contracts: dict[str, tuple[str, int, str]] = {}
        for qualified_name in sorted(qualified_contracts):
            export = qualified_contracts[qualified_name][3]
            if export.binding != "direct_symbol" or export.symbol is None:
                continue
            contract = (
                export.abi,
                export.wrapper_payload_arity,
                export.qualified_name,
            )
            prior = symbol_contracts.get(export.symbol)
            if prior is None:
                symbol_contracts[export.symbol] = contract
                continue
            if prior[:2] != contract[:2]:
                conflicts.append(
                    f"direct symbol {export.symbol!r} has conflicting callable "
                    f"contracts: {prior[2]} ({prior[0]}, arity={prior[1]}) and "
                    f"{contract[2]} ({contract[0]}, arity={contract[1]})"
                )
        merged = tuple(
            (
                qualified_contracts[name][2],
                qualified_contracts[name][3],
            )
            for name in sorted(qualified_contracts)
        )
        return merged, tuple(sorted(set(conflicts)))

    def native_callable_contract_conflicts(self) -> tuple[str, ...]:
        _exports, conflicts = self._merged_native_callable_exports()
        return conflicts

    def native_module_init_specs(self) -> tuple[_ExternalNativeModuleInitSpec, ...]:
        specs: dict[str, _ExternalNativeModuleInitSpec] = {}
        module_attr_exports: dict[str, set[_ExternalNativeModuleAttrPublishSpec]] = {}
        direct_symbol_exports: dict[str, set[_ExternalNativeCallableExport]] = {}
        merged_callable_exports, _conflicts = self._merged_native_callable_exports()
        for artifact in self.artifacts:
            names, _support_init_modules = self._artifact_base_module_names(artifact)
            capsule_alias_modules = self._artifact_capsule_alias_module_names(artifact)
            for exported_name in artifact.python_exports:
                parts = exported_name.split(".")
                names.update(".".join(parts[:idx]) for idx in range(1, len(parts)))
                if exported_name in {artifact.package, artifact.module}:
                    names.add(exported_name)
            for origin_module, export in merged_callable_exports:
                if origin_module != artifact.module:
                    continue
                parts = export.qualified_name.split(".")
                names.update(".".join(parts[:idx]) for idx in range(1, len(parts)))
                provider_module = export.provider_module or origin_module
                if export.binding == "module_attr":
                    names.update(self._module_prefixes(provider_module))
                if export.binding == "module_attr" and export.module != provider_module:
                    names.add(export.module)
                    module_attr_exports.setdefault(export.module, set()).add(
                        _ExternalNativeModuleAttrPublishSpec(
                            provider_module=provider_module,
                            attr=export.name,
                        )
                    )
                if export.binding == "direct_symbol":
                    names.add(export.module)
                    direct_symbol_exports.setdefault(export.module, set()).add(export)
            for name in names:
                if name == artifact.module and artifact.init_symbol:
                    specs[name] = _ExternalNativeModuleInitSpec(
                        module=name,
                        init_symbol=artifact.init_symbol,
                    )
                elif name not in specs:
                    specs[name] = _ExternalNativeModuleInitSpec(module=name)
            for alias_module in capsule_alias_modules:
                existing = specs.get(alias_module)
                if existing is not None and existing.is_extension:
                    continue
                specs[alias_module] = _ExternalNativeModuleInitSpec(
                    module=alias_module,
                    alias_of=artifact.module,
                )
            for runtime_module in artifact.runtime_python_import_modules:
                # Modules the extension imports dynamically at runtime must
                # be dispatchable by name through isolate import; the plain
                # spec supplies the dispatcher entry while the compiled
                # Python init (which always exists for these support-custody
                # modules) wins at function emission. Parent packages are
                # already covered by the artifact base-name expansion.
                if runtime_module not in specs:
                    specs[runtime_module] = _ExternalNativeModuleInitSpec(
                        module=runtime_module
                    )
        for module, exports in module_attr_exports.items():
            existing = specs.get(module, _ExternalNativeModuleInitSpec(module=module))
            specs[module] = _ExternalNativeModuleInitSpec(
                module=existing.module,
                init_symbol=existing.init_symbol,
                module_attr_exports=tuple(
                    sorted(exports, key=lambda item: (item.provider_module, item.attr))
                ),
                direct_symbol_exports=existing.direct_symbol_exports,
                alias_of=existing.alias_of,
            )
        for module, exports in direct_symbol_exports.items():
            existing = specs.get(module, _ExternalNativeModuleInitSpec(module=module))
            specs[module] = _ExternalNativeModuleInitSpec(
                module=existing.module,
                init_symbol=existing.init_symbol,
                module_attr_exports=existing.module_attr_exports,
                direct_symbol_exports=tuple(
                    sorted(exports, key=lambda item: item.qualified_name)
                ),
                alias_of=existing.alias_of,
            )
        return tuple(
            specs[name]
            for name in sorted(specs, key=lambda value: (value.count("."), value))
        )

    @staticmethod
    def _requested_import_reaches_module(
        requested_name: str,
        module_name: str,
    ) -> bool:
        return requested_name == module_name or requested_name.startswith(
            module_name + "."
        )

    @classmethod
    def _requested_import_reaches_callable_export(
        cls,
        requested_name: str,
        export: _ExternalNativeCallableExport,
    ) -> bool:
        return (
            cls._requested_import_reaches_module(
                requested_name,
                export.qualified_name,
            )
            or requested_name == export.module
        )

    def with_reachable_imports(
        self,
        explicit_imports: Collection[str],
    ) -> "_ExternalPackageNativeArtifactPlan":
        if not self.artifacts:
            return self
        requested = frozenset(
            name.strip()
            for name in explicit_imports
            if isinstance(name, str) and name.strip()
        )
        if not requested:
            return _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN

        reachable: list[_ExternalPackageNativeArtifact] = []
        reachable_keys: set[tuple[str, Path]] = set()
        for artifact in self.artifacts:
            if any(
                self._requested_import_reaches_module(requested_name, provider_name)
                for requested_name in requested
                for provider_name in (artifact.module, *artifact.python_exports)
            ) or any(
                self._requested_import_reaches_callable_export(requested_name, export)
                for requested_name in requested
                for export in artifact.callable_exports
            ):
                reachable.append(artifact)
                reachable_keys.add((artifact.module, artifact.path))
        while True:
            required_capsules = frozenset(
                capsule
                for artifact in reachable
                for capsule in artifact.required_capsules
            )
            if not required_capsules:
                break
            added = False
            for artifact in self.artifacts:
                key = (artifact.module, artifact.path)
                if key in reachable_keys:
                    continue
                if required_capsules.intersection(artifact.provided_capsules):
                    reachable.append(artifact)
                    reachable_keys.add(key)
                    added = True
            if not added:
                break
        if len(reachable) == len(self.artifacts):
            return self
        return _ExternalPackageNativeArtifactPlan(artifacts=tuple(reachable))

    def digest(self) -> str:
        payload = json.dumps(
            self.digest_payload(),
            sort_keys=True,
            separators=(",", ":"),
        )
        return hashlib.sha256(payload.encode("utf-8")).hexdigest()


_EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN = _ExternalPackageNativeArtifactPlan()


@dataclass(frozen=True)
class _StagedExternalPackageNativeArtifact:
    package: str
    module: str
    runtime_root: Path
    source_path: Path
    source_manifest_path: Path
    staged_path: Path
    staged_manifest_path: Path
    staged_support_paths: tuple[Path, ...]
    extension_sha256: str
    manifest_sha256: str
    capabilities: tuple[str, ...]
    abi_tag: str
    target_triple: str
    platform_tag: str
    runtime_linkage: str
    artifact_kind: str
    link_requirements: SourceExtensionLinkRequirements
    staged_link_input_paths: tuple[Path, ...] = ()
    init_symbol: str = ""
    support_file_sha256: tuple[tuple[str, str], ...] = ()
    provided_capsules: tuple[str, ...] = ()
    required_capsules: tuple[str, ...] = ()
    python_exports: tuple[str, ...] = ()
    callable_exports: tuple[_ExternalNativeCallableExport, ...] = ()
    abi_symbols: tuple[_ExternalNativeAbiSymbol, ...] = ()
    c_api_symbols: tuple[_ExternalNativeCapiSymbol, ...] = ()

    def json_payload(self) -> dict[str, Any]:
        return {
            "package": self.package,
            "module": self.module,
            "runtime_root": str(self.runtime_root),
            "source_path": str(self.source_path),
            "source_manifest_path": str(self.source_manifest_path),
            "staged_path": str(self.staged_path),
            "staged_manifest_path": str(self.staged_manifest_path),
            "staged_support_paths": [str(path) for path in self.staged_support_paths],
            "extension_sha256": self.extension_sha256,
            "manifest_sha256": self.manifest_sha256,
            "capabilities": list(self.capabilities),
            "abi_tag": self.abi_tag,
            "target_triple": self.target_triple,
            "platform_tag": self.platform_tag,
            "init_symbol": self.init_symbol,
            "runtime_linkage": self.runtime_linkage,
            "artifact_kind": self.artifact_kind,
            "link_requirements": self.link_requirements.manifest_payload(),
            "staged_link_input_paths": [
                str(path) for path in self.staged_link_input_paths
            ],
            "support_file_sha256": [
                {"path": rel_path, "sha256": digest}
                for rel_path, digest in self.support_file_sha256
            ],
            "provided_capsules": list(self.provided_capsules),
            "required_capsules": list(self.required_capsules),
            "python_exports": list(self.python_exports),
            "callable_exports": [
                export.digest_payload() for export in self.callable_exports
            ],
            "abi_symbols": [symbol.digest_payload() for symbol in self.abi_symbols],
            "c_api_symbols": [symbol.digest_payload() for symbol in self.c_api_symbols],
        }


@dataclass(frozen=True)
class _ImportAdmissionPolicy:
    """Which discovered sources the build compiles.

    A source belongs to the deepest root that contains it. It is external when
    that root is an external root (MOLT_MODULE_ROOTS, --lib-path, PYTHONPATH,
    site-packages), and then only direct entry imports and admitted packages
    are compiled. Program roots hold the program's own source: the project,
    cwd, src and vendor roots, the root a script entry adds and the entry's
    top-level package (``_resolve_build_entry``). A program root wins over an
    external root that equals or contains it, so the entry's own package is
    never third-party code, wherever its root came from.
    """

    external_roots: tuple[Path, ...] = ()
    program_roots: tuple[Path, ...] = ()
    admitted_external_packages: frozenset[str] = frozenset()
    native_artifact_source_packages: frozenset[str] = frozenset()
    native_artifact_plan: _ExternalPackageNativeArtifactPlan = field(
        default_factory=_ExternalPackageNativeArtifactPlan
    )
    # One discovery asks about the same few hundred paths about 21,000 times;
    # each answer costs a realpath, so the policy remembers it.
    _roots_by_path: dict[Path, Path | None] = field(
        default_factory=dict, init=False, compare=False, hash=False, repr=False
    )

    def __post_init__(self) -> None:
        program_roots = tuple(
            dict.fromkeys(root.resolve() for root in self.program_roots)
        )
        external_roots = tuple(
            root
            for root in dict.fromkeys(root.resolve() for root in self.external_roots)
            if root not in program_roots
        )
        admitted = frozenset(
            name.strip()
            for name in self.admitted_external_packages
            if name and name.strip()
        )
        native_artifact_source_packages = frozenset(
            name.strip()
            for name in self.native_artifact_source_packages
            if name and name.strip()
        )
        object.__setattr__(self, "external_roots", external_roots)
        object.__setattr__(self, "program_roots", program_roots)
        object.__setattr__(self, "admitted_external_packages", admitted)
        object.__setattr__(
            self,
            "native_artifact_source_packages",
            native_artifact_source_packages,
        )

    def _external_root_for_path(self, path: Path) -> Path | None:
        if not self.external_roots:
            return None
        try:
            return self._roots_by_path[path]
        except KeyError:
            pass
        resolved_path = path.resolve()

        def deepest(roots: tuple[Path, ...]) -> Path | None:
            containing = [root for root in roots if resolved_path.is_relative_to(root)]
            return (
                max(containing, key=lambda root: len(root.parts))
                if containing
                else None
            )

        external = deepest(self.external_roots)
        program = deepest(self.program_roots)
        found = (
            external
            if external is not None
            and (program is None or len(external.parts) > len(program.parts))
            else None
        )
        self._roots_by_path[path] = found
        return found

    def _package_admitted(self, module_name: str) -> bool:
        for package in self.admitted_external_packages:
            if module_name == package or module_name.startswith(package + "."):
                return True
        return False

    def _native_artifact_source_package(self, module_name: str) -> str | None:
        for package in self.native_artifact_source_packages:
            if module_name == package or module_name.startswith(package + "."):
                return package
        return None

    def _native_artifact_support_source_module(self, module_name: str) -> bool:
        return module_name in self.native_artifact_plan.support_source_module_names()

    def owns_source_closure_with_native_artifact_plan(
        self,
        module_name: str,
        path: Path,
    ) -> bool:
        if self._native_artifact_support_source_module(module_name):
            return False
        if self._native_artifact_source_package(module_name) is not None:
            return True
        return False

    def admits_import(
        self,
        module_name: str,
        path: Path,
        *,
        from_entry_path: bool,
    ) -> bool:
        if self._native_artifact_support_source_module(module_name):
            return True
        if self._native_artifact_source_package(module_name) is not None:
            return False
        if self._external_root_for_path(path) is None:
            return True
        if from_entry_path:
            return True
        return self._package_admitted(module_name)

    def admits_package_parent(
        self,
        module_name: str,
        path: Path,
        *,
        existing_modules: Collection[str],
    ) -> bool:
        if self._native_artifact_source_package(module_name) is not None:
            return False
        if self._external_root_for_path(path) is None:
            return True
        if self._package_admitted(module_name):
            return True
        prefix = module_name + "."
        return any(name.startswith(prefix) for name in existing_modules)

    def digest_payload(self) -> dict[str, Any]:
        return {
            "external_roots": [str(root) for root in self.external_roots],
            "program_roots": [str(root) for root in self.program_roots],
            "admitted_external_packages": sorted(self.admitted_external_packages),
            "native_artifact_source_packages": sorted(
                self.native_artifact_source_packages
            ),
            "native_artifact_plan": self.native_artifact_plan.digest_payload(),
        }


@dataclass(frozen=True)
class _PreparedEntryModuleGraph:
    image_scope: _BinaryImageScope
    declared_root_modules: frozenset[str]
    stdlib_allowlist: set[str]
    roots: list[Path]
    module_resolution_cache: "_ModuleResolutionCache"
    discovery: "_ModuleGraphDiscovery"
    module_graph: dict[str, Path]
    scan_authority: _ModuleGraphScanAuthority
    target: Target
    explicit_imports: set[str]
    runtime_import_dispatch_roots: frozenset[str]
    stub_parents: set[str]
    spawn_enabled: bool
    runtime_import_support_policy: _RuntimeImportSupportPolicy
    native_artifact_plan: _ExternalPackageNativeArtifactPlan
    target_python: TargetPythonVersion
    runtime_import_scan_custody: _RuntimeImportScanCustody | None = None
    project_root: Path | None = None


@dataclass(frozen=True)
class _ResolvedBuildEntry:
    source_path: Path
    entry_module: str
    module_roots: list[Path]
    entry_source: str
    entry_tree: ast.Module
    target_python: TargetPythonVersion
    entry_snapshot: PythonSourceSnapshot | None = None
    import_topology: PythonImportTopology = field(default_factory=PythonImportTopology)
    module_root_aliases: tuple[tuple[str, Path], ...] = ()
    external_module_roots: tuple[Path, ...] = ()
    program_module_roots: tuple[Path, ...] = ()
    image_scope: _BinaryImageScope | None = None

    def __post_init__(self) -> None:
        if self.image_scope is None:
            object.__setattr__(
                self,
                "image_scope",
                _BinaryImageScope.from_entry(
                    kind="entry_script",
                    selector_source="legacy:entry",
                    entry_module=self.entry_module,
                    source_path=self.source_path,
                    project_root=self.source_path.parent,
                    module_roots=self.module_roots,
                ),
            )


@dataclass(frozen=True)
class _ImportPlan:
    image_scope: _BinaryImageScope
    stdlib_allowlist: frozenset[str]
    roots: tuple[Path, ...]
    stdlib_root: Path
    module_resolution_cache: "_ModuleResolutionCache"
    module_graph: Mapping[str, Path]
    scan_authority: _ModuleGraphScanAuthority
    module_graph_operation_counts: Mapping[str, int]
    explicit_imports: frozenset[str]
    runtime_import_dispatch_roots: frozenset[str]
    stub_parents: frozenset[str]
    spawn_enabled: bool
    runtime_import_support_policy: _RuntimeImportSupportPolicy
    namespace_portions_by_module: Mapping[str, tuple[str, ...]]
    known_modules: frozenset[str]
    direct_call_modules: frozenset[str]
    declared_root_modules: frozenset[str]
    entry_reachable_modules: frozenset[str]
    runtime_support_modules: frozenset[str]
    stdlib_support_modules: frozenset[str]
    package_parent_modules: frozenset[str]
    compile_modules: frozenset[str]
    known_modules_sorted: tuple[str, ...]
    stdlib_allowlist_sorted: tuple[str, ...]
    module_graph_metadata: _ModuleGraphMetadata
    native_artifact_plan: _ExternalPackageNativeArtifactPlan
    native_support_function_roots_by_module: Mapping[str, tuple[str, ...]] = field(
        default_factory=dict
    )
    runtime_import_scan_custody: _RuntimeImportScanCustody | None = None

    @property
    def namespace_module_names(self) -> frozenset[str]:
        return frozenset(self.namespace_portions_by_module)

    def with_compile_modules(self, compile_modules: Collection[str]) -> "_ImportPlan":
        compile_set = frozenset(compile_modules)
        unknown = sorted(compile_set - frozenset(self.module_graph))
        if unknown:
            raise ValueError(
                "compile module set contains modules outside the closure plan "
                "or without source graph entries: " + ", ".join(unknown)
            )
        return _ImportPlan(
            image_scope=self.image_scope,
            stdlib_allowlist=self.stdlib_allowlist,
            roots=self.roots,
            stdlib_root=self.stdlib_root,
            module_resolution_cache=self.module_resolution_cache,
            module_graph=self.module_graph,
            scan_authority=self.scan_authority,
            module_graph_operation_counts=self.module_graph_operation_counts,
            explicit_imports=self.explicit_imports,
            runtime_import_dispatch_roots=self.runtime_import_dispatch_roots,
            stub_parents=self.stub_parents,
            spawn_enabled=self.spawn_enabled,
            runtime_import_support_policy=self.runtime_import_support_policy,
            runtime_import_scan_custody=self.runtime_import_scan_custody,
            namespace_portions_by_module=self.namespace_portions_by_module,
            known_modules=self.known_modules,
            direct_call_modules=frozenset(
                name for name in compile_set if name in self.direct_call_modules
            ),
            declared_root_modules=self.declared_root_modules,
            entry_reachable_modules=self.entry_reachable_modules,
            runtime_support_modules=self.runtime_support_modules,
            stdlib_support_modules=self.stdlib_support_modules,
            package_parent_modules=self.package_parent_modules,
            compile_modules=compile_set,
            known_modules_sorted=tuple(sorted(self.known_modules)),
            stdlib_allowlist_sorted=self.stdlib_allowlist_sorted,
            module_graph_metadata=self.module_graph_metadata,
            native_artifact_plan=self.native_artifact_plan,
            native_support_function_roots_by_module=dict(
                self.native_support_function_roots_by_module
            ),
        )

    def closure_payload(self) -> dict[str, Any]:
        return {
            "image": self.image_scope.diagnostic_payload(),
            "known_modules": sorted(self.known_modules),
            "direct_call_modules": sorted(self.direct_call_modules),
            "compile_modules": sorted(self.compile_modules),
            "declared_root_modules": sorted(self.declared_root_modules),
            "entry_reachable_modules": sorted(self.entry_reachable_modules),
            "runtime_support_modules": sorted(self.runtime_support_modules),
            "stdlib_support_modules": sorted(self.stdlib_support_modules),
            "package_parent_modules": sorted(self.package_parent_modules),
            "runtime_import_dispatch_roots": sorted(self.runtime_import_dispatch_roots),
            "source_scan_authority": self.scan_authority.payload(
                self.module_graph_metadata.logical_source_path_by_module
            ),
            "stub_parents": sorted(self.stub_parents),
            "namespace_portions_by_module": {
                name: list(self.namespace_portions_by_module[name])
                for name in sorted(self.namespace_portions_by_module)
            },
            "spawn_enabled": self.spawn_enabled,
            "runtime_import_support": {
                "scan_owner_ast_digests": (
                    dict(self.runtime_import_scan_custody.owner_ast_digests)
                    if self.runtime_import_scan_custody is not None
                    else {}
                ),
                "needs_runtime_import_support": (
                    self.runtime_import_support_policy.needs_runtime_import_support
                ),
            },
            "external_native_artifacts": self.native_artifact_plan.digest_payload(),
            "native_support_function_roots_by_module": {
                module: list(roots)
                for module, roots in sorted(
                    self.native_support_function_roots_by_module.items()
                )
            },
        }


@dataclass(frozen=True)
class _PreparedBuildConfig:
    pgo_profile_summary: PgoProfileSummary | None
    pgo_profile_path: Path | None
    runtime_feedback_summary: RuntimeFeedbackSummary | None
    runtime_feedback_path: Path | None
    pgo_hot_function_names: set[str]
    pgo_hot_function_names_sorted: tuple[str, ...]
    pgo_profile_payload: dict[str, Any] | None
    runtime_feedback_payload: dict[str, Any] | None
    cargo_timeout: float | None
    backend_timeout: float | None
    link_timeout: float | None
    frontend_phase_timeout: float | None
    backend_profile: BuildProfile
    runtime_cargo_profile: str
    backend_cargo_profile: str
    resolved_capability_policy: ResolvedRuntimePolicy
    capabilities_source: str | None
    target_python: TargetPythonVersion
    target_sys_platform: str | None
    # The backend-process selection for this build; see CodegenSelection.
    codegen: CodegenSelection

    @property
    def capabilities_list(self) -> list[str]:
        return list(self.resolved_capability_policy.grants.capabilities)

    @property
    def capability_profiles(self) -> list[str]:
        return list(self.resolved_capability_policy.grants.profiles)

    @property
    def manifest_env_vars(self) -> dict[str, str]:
        return self.resolved_capability_policy.to_env_vars()


@dataclass(frozen=True)
class _PreparedBuildPreamble:
    diagnostics_path_spec: str
    diagnostics_enabled: bool
    resolved_diagnostics_verbosity: str
    allocation_diagnostics_enabled: bool
    frontend_timing_raw: str
    frontend_timing_enabled: bool
    frontend_timing_threshold: float
    frontend_module_timings: list[dict[str, Any]]
    midend_policy_outcomes_by_function: dict[str, dict[str, Any]]
    midend_pass_stats_by_function: dict[str, dict[str, dict[str, Any]]]
    frontend_parallel_details: dict[str, Any]
    diagnostics_start: float
    phase_starts: dict[str, float]
    backend_daemon_health: dict[str, Any] | None
    backend_daemon_cached: bool | None
    backend_daemon_cache_tier: str | None
    backend_daemon_config_digest: str | None
    module_reasons: dict[str, set[str]]
    stdlib_root: Path
    warnings: list[str]
    rust_cpu: RustCpuPolicy


@dataclass(frozen=True)
class _PreparedBuildRoots:
    cwd_root: Path
    project_root: Path
    molt_root: Path
    sysroot_path: Path | None


@dataclass(frozen=True)
class _PreparedFrontendRunTicket:
    module_order: list[str]
    module_layers: list[list[str]]
    frontend_parallel_config: _FrontendParallelConfig
    frontend_parallel_layers: list[dict[str, Any]]
    frontend_parallel_worker_timings: list[dict[str, Any]]
    frontend_parallel_details: dict[str, Any]
    frontend_layer_execution_context: _FrontendLayerExecutionContext
    frontend_layer_runtime_hooks: _FrontendLayerRuntimeHooks


@dataclass(frozen=True)
class _PreparedBackendSetup:
    backend: str
    runtime_state: _RuntimeArtifactState
    backend_bin: Path
    cache_setup: _BackendCacheSetup
    cache_hit: bool
    cache_hit_tier: str | None
    cache_key: str | None
    function_cache_key: str | None
    cache_path: Path | None
    function_cache_path: Path | None
    stdlib_object_path: Path | None
    cache_candidates: list[tuple[str, Path]]
    runtime_callable_symbols_digest: str = ""
    backend_compiler_fingerprint: str | None = None


@dataclass(frozen=True)
class _PreparedBackendCompile:
    cache_enabled: bool
    cache_hit: bool
    cache_hit_tier: str | None
    wasm_table_base: int | None
    backend_daemon_cached: bool | None
    backend_daemon_cache_tier: str | None
    backend_daemon_health: dict[str, Any] | None
    backend_daemon_config_digest: str | None
    backend_emission_origin: BackendEmissionOrigin | None = None


@dataclass(frozen=True)
class _PreparedBackendRuntimeContext:
    runtime_state: _RuntimeArtifactState
    backend_bin: Path
    runtime_lib: Path | None
    ensure_runtime_wasm_both: Callable[[set[str] | frozenset[str] | None], bool]
    cache_setup: _BackendCacheSetup
    cache_hit: bool
    cache_hit_tier: str | None
    cache_key: str | None
    function_cache_key: str | None
    cache_path: Path | None
    function_cache_path: Path | None
    stdlib_object_path: Path | None
    backend_compiler_fingerprint: str | None = None


@dataclass(frozen=True)
class _PreparedNonNativeResult:
    primary_output: Path
    consumer_output: Path
    bundle_root: Path | None
    linked_output_path: Path | None
    success_messages: list[str]
    extra_fields: dict[str, Any]
    artifacts: dict[str, str] | None
    link_input_observation: CompilationInputRecord | None = None
