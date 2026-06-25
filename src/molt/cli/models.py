from __future__ import annotations

import ast
import hashlib
import json
import os
import subprocess
from concurrent.futures import Future
from dataclasses import dataclass, field
from pathlib import Path
from typing import (
    Any,
    Callable,
    Collection,
    Literal,
    Mapping,
    MutableMapping,
    NamedTuple,
    Sequence,
    TYPE_CHECKING,
)

from molt.cli.output import CliFailure as _CliFailure
from molt.cli.target_python import TargetPythonVersion
from molt.type_facts import TypeFacts

if TYPE_CHECKING:
    from molt.cli.module_graph import (
        ModuleSyntaxErrorInfo,
        _ModuleResolutionCache,
    )
    from molt.cli.module_source import _ModuleSourceCatalog

ParseCodec = Literal["msgpack", "cbor", "json"]
TypeHintPolicy = Literal["ignore", "trust", "check"]
FallbackPolicy = Literal["error", "bridge"]
BuildProfile = Literal["dev", "release"]
EmitMode = Literal["bin", "obj", "wasm"]
Target = str
ImportScanMode = Literal["full", "module_init"]


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


class _TimedResult(NamedTuple):
    returncode: int
    stdout: str
    stderr: str
    duration_s: float


class _BackendDaemonCompileResult(NamedTuple):
    ok: bool
    error: str | None
    health: dict[str, Any] | None
    cached: bool | None
    cache_tier: str | None
    output_written: bool
    output_exists: bool
    full_request_sent: bool = False


class _PersistedModuleGraphState(NamedTuple):
    graph: dict[str, Path]
    explicit_imports: set[str]
    dirty_modules: set[str]


class _MaintenanceStep(NamedTuple):
    name: str
    cmd: list[str]
    cwd: Path
    category: Literal["toolchain", "lock", "manifest"]


class _ValidationStep(NamedTuple):
    name: str
    cmd: list[str]
    cwd: Path
    category: Literal["command", "correctness", "conformance", "benchmark"]
    backends: tuple[str, ...]
    profiles: tuple[str, ...]
    suite: Literal["full", "smoke"]


@dataclass(frozen=True)
class _ToolchainReport:
    checks: list[dict[str, Any]]
    warnings: list[str]
    errors: list[str]
    environment: dict[str, str]
    actions: list[dict[str, str]]
    backends: dict[str, bool]
    profiles: dict[str, bool]


@dataclass(frozen=True)
class _ScopedLoweringInputs:
    known_modules_by_module: dict[str, tuple[str, ...]]
    known_func_defaults_by_module: dict[str, dict[str, dict[str, Any]]]
    known_func_kinds_by_module: dict[str, dict[str, dict[str, str]]]
    pgo_hot_function_names_by_module: dict[str, tuple[str, ...]]
    type_facts_by_module: dict[str, TypeFacts | None]


@dataclass(frozen=True)
class _ScopedLoweringInputView:
    known_modules: tuple[str, ...]
    known_func_defaults: dict[str, dict[str, Any]]
    known_func_kinds: dict[str, dict[str, str]]
    pgo_hot_function_names: tuple[str, ...]
    type_facts: TypeFacts | None
    known_modules_payload: list[str] = field(default_factory=list)
    known_modules_set: frozenset[str] = field(default_factory=frozenset)
    pgo_hot_function_names_payload: list[str] = field(default_factory=list)
    pgo_hot_function_names_set: frozenset[str] = field(default_factory=frozenset)

    def __post_init__(self) -> None:
        if not self.known_modules_payload and self.known_modules:
            object.__setattr__(self, "known_modules_payload", list(self.known_modules))
        if not self.known_modules_set and self.known_modules:
            object.__setattr__(self, "known_modules_set", frozenset(self.known_modules))
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
    logical_source_path_by_module: Mapping[str, str]
    entry_override_by_module: Mapping[str, str | None]
    module_is_namespace_by_module: Mapping[str, bool]
    module_is_package_by_module: Mapping[str, bool]
    frontend_module_costs: Mapping[str, float] | None
    stdlib_like_by_module: Mapping[str, bool] | None


@dataclass(frozen=True)
class _ModuleLoweringMetadataView:
    logical_source_path: str
    entry_override: str | None
    module_is_namespace: bool
    is_package: bool
    path_stat: os.stat_result | None


@dataclass(frozen=True)
class _ModuleLoweringExecutionView:
    metadata: _ModuleLoweringMetadataView
    scoped_inputs: _ScopedLoweringInputView
    scoped_known_classes: dict[str, Any]

@dataclass(frozen=True)
class _ParallelWorkerSubmission:
    module_name: str
    submitted_ns: int
    future: Any


@dataclass(frozen=True)
class _WorkerTimingSummary:
    count: int
    queue_ms_total: float
    queue_ms_max: float
    wait_ms_total: float
    wait_ms_max: float
    exec_ms_total: float
    exec_ms_max: float
    roundtrip_ms_total: float


@dataclass(frozen=True)
class _FrontendLayerStaticMetrics:
    predicted_cost_total: float
    stdlib_candidates: int


@dataclass(frozen=True)
class _FrontendModuleResultTimings:
    visit_s: float
    lower_s: float
    total_s: float


@dataclass(frozen=True)
class _FrontendLayerPolicySummary:
    enabled: bool
    workers: int
    reason: str
    predicted_cost_total: float
    effective_min_predicted_cost: float
    stdlib_candidates: int


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
class _FrontendLayerRunResult:
    layer_state: _FrontendParallelLayerState
    layer_plan: _FrontendLayerPlan
    parallel_pool_usable: bool


@dataclass(frozen=True)
class _FrontendLayerExecutionContext:
    syntax_error_modules: Mapping[str, Any]
    module_graph: Mapping[str, Path]
    module_source_catalog: _ModuleSourceCatalog
    project_root: Path | None
    module_resolution_cache: "_ModuleResolutionCache"
    parse_codec: "ParseCodec"
    type_hint_policy: "TypeHintPolicy"
    fallback_policy: "FallbackPolicy"
    type_facts: TypeFacts | None
    enable_phi: bool
    known_modules: Collection[str]
    stdlib_allowlist: Collection[str]
    known_func_defaults: dict[str, dict[str, dict[str, Any]]]
    known_func_kinds: dict[str, dict[str, str]]
    module_deps: dict[str, set[str]]
    module_chunk_max_ops: int
    optimization_profile: str
    pgo_hot_function_names: Collection[str]
    known_modules_sorted: tuple[str, ...]
    stdlib_allowlist_sorted: tuple[str, ...]
    pgo_hot_function_names_sorted: tuple[str, ...]
    module_dep_closures: dict[str, frozenset[str]]
    module_graph_metadata: "_ModuleGraphMetadata"
    path_stat_by_module: Mapping[str, os.stat_result | None] | None
    module_chunking: bool
    scoped_lowering_inputs: "_ScopedLoweringInputs | None"
    dirty_lowering_modules: Collection[str]
    frontend_module_costs: Mapping[str, float]
    stdlib_like_by_module: Mapping[str, bool]
    known_classes: Mapping[str, Any]
    target_python: TargetPythonVersion


@dataclass(frozen=True)
class _FrontendLayerRuntimeHooks:
    warnings: list[str]
    frontend_parallel_details: MutableMapping[str, Any]
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]]
    record_frontend_timing: Callable[..., None]
    integrate_module_frontend_result: Callable[..., str | None]
    accumulate_midend_diagnostics: Callable[..., None]
    fail: Callable[..., _CliFailure]
    json_output: bool
    run_serial_frontend_lower: Callable[
        [str, Path],
        tuple[
            dict[str, Any] | None,
            "_FrontendModuleResultTimings | None",
            _CliFailure | None,
        ],
    ]


@dataclass(frozen=True)
class _SerialFrontendLoweringContext:
    syntax_error_modules: Mapping[str, Any]
    module_trees: Mapping[str, ast.AST]
    module_source_catalog: _ModuleSourceCatalog
    generated_module_source_paths: Mapping[str, str]
    module_resolution_cache: "_ModuleResolutionCache"
    project_root: Path | None
    dirty_lowering_modules: Collection[str]
    parse_codec: "ParseCodec"
    type_hint_policy: "TypeHintPolicy"
    fallback_policy: "FallbackPolicy"
    type_facts: TypeFacts | None
    enable_phi: bool
    known_modules: Collection[str]
    stdlib_allowlist: Collection[str]
    known_func_defaults: dict[str, dict[str, dict[str, Any]]]
    known_func_kinds: dict[str, dict[str, str]]
    module_deps: dict[str, set[str]]
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
    module_path_stats: Mapping[str, os.stat_result | None] | None
    known_classes: Mapping[str, Any]
    frontend_phase_timeout: float | None
    target_python: TargetPythonVersion


@dataclass(frozen=True)
class _SerialFrontendLoweringHooks:
    record_frontend_timing: Callable[..., None]
    fail: Callable[..., _CliFailure]
    json_output: bool


@dataclass
class _FrontendIntegrationState:
    functions: list[dict[str, Any]]
    known_classes: dict[str, Any]
    global_code_ids: dict[str, int] = field(default_factory=dict)
    global_code_id_counter: int = 0


@dataclass
class _MidendDiagnosticsState:
    policy_outcomes_by_function: dict[str, dict[str, Any]]
    pass_stats_by_function: dict[str, dict[str, dict[str, Any]]]


@dataclass(frozen=True)
class _EntryFrontendLoweringContext:
    entry_module: str
    entry_path: Path
    parse_codec: "ParseCodec"
    type_hint_policy: "TypeHintPolicy"
    fallback_policy: "FallbackPolicy"
    type_facts: TypeFacts | None
    enable_phi: bool
    known_modules: Collection[str]
    known_classes: Mapping[str, Any]
    stdlib_allowlist: Collection[str]
    known_func_defaults: dict[str, dict[str, dict[str, Any]]]
    known_func_kinds: dict[str, dict[str, str]]
    module_chunking: bool
    module_chunk_max_ops: int
    optimization_profile: str
    pgo_hot_function_names: Collection[str]
    frontend_phase_timeout: float | None
    target_python: TargetPythonVersion


@dataclass
class _RuntimeArtifactState:
    runtime_lib: Path | None = None
    runtime_wasm: Path | None = None
    runtime_reloc_wasm: Path | None = None
    runtime_wasm_ready: bool = False
    runtime_reloc_wasm_ready: bool = False
    runtime_lib_ready_future: Future[bool] | None = None


@dataclass(frozen=True)
class _BackendCacheSetup:
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


@dataclass(frozen=True)
class _BuildDiagnosticsContext:
    diagnostics_enabled: bool
    diagnostics_start: float
    phase_starts: Mapping[str, float]
    module_graph: Mapping[str, Path]
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


@dataclass(frozen=True)
class _FrontendTimingRecorderConfig:
    enabled: bool
    raw: bool
    threshold: float
    json_output: bool


@dataclass(frozen=True)
class _BuildOutputLayout:
    is_wasm: bool
    is_wasm_freestanding: bool
    is_rust_transpile: bool
    is_luau_transpile: bool
    is_mlir_emit: bool
    split_runtime: bool
    linked: bool
    target_triple: str | None
    emit_mode: str
    output_artifact: Path
    output_binary: Path | None
    linked_output_path: Path | None
    emit_ir_path: Path | None


@dataclass(frozen=True)
class _SupportModuleAugmentation:
    namespace_module_names: frozenset[str]
    generated_module_source_paths: dict[str, str]


@dataclass(frozen=True)
class _ModuleGraphAugmentation:
    spawn_enabled: bool
    explicit_imports: set[str]
    stub_parents: set[str]


@dataclass(frozen=True)
class _RuntimeImportSupportPolicy:
    needs_generated_importer: bool
    needs_runtime_import_support: bool


@dataclass(frozen=True)
class _ModuleRootResolution:
    roots: tuple[Path, ...]
    external_roots: tuple[Path, ...]


@dataclass(frozen=True)
class _ExternalPackageNativeArtifact:
    package: str
    module: str
    package_dir: Path
    path: Path
    manifest_path: Path
    extension_sha256: str
    manifest_sha256: str
    capabilities: tuple[str, ...]
    abi_tag: str
    target_triple: str
    platform_tag: str

    def digest_payload(self) -> dict[str, Any]:
        return {
            "package": self.package,
            "module": self.module,
            "package_dir": str(self.package_dir),
            "path": str(self.path),
            "manifest_path": str(self.manifest_path),
            "extension_sha256": self.extension_sha256,
            "manifest_sha256": self.manifest_sha256,
            "capabilities": list(self.capabilities),
            "abi_tag": self.abi_tag,
            "target_triple": self.target_triple,
            "platform_tag": self.platform_tag,
        }


@dataclass(frozen=True)
class _ExternalPackageNativeArtifactPlan:
    artifacts: tuple[_ExternalPackageNativeArtifact, ...] = ()

    def digest_payload(self) -> dict[str, Any]:
        return {"artifacts": [artifact.digest_payload() for artifact in self.artifacts]}

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
        }


@dataclass(frozen=True)
class _ImportAdmissionPolicy:
    external_roots: tuple[Path, ...] = ()
    admitted_external_packages: frozenset[str] = frozenset()
    native_artifact_plan: _ExternalPackageNativeArtifactPlan = field(
        default_factory=_ExternalPackageNativeArtifactPlan
    )

    def __post_init__(self) -> None:
        external_roots = tuple(
            dict.fromkeys(root.resolve() for root in self.external_roots)
        )
        admitted = frozenset(
            name.strip()
            for name in self.admitted_external_packages
            if name and name.strip()
        )
        object.__setattr__(self, "external_roots", external_roots)
        object.__setattr__(self, "admitted_external_packages", admitted)

    def _external_root_for_path(self, path: Path) -> Path | None:
        resolved_path = path.resolve()
        for root in self.external_roots:
            if resolved_path == root or resolved_path.is_relative_to(root):
                return root
        return None

    def _package_admitted(self, module_name: str) -> bool:
        for package in self.admitted_external_packages:
            if module_name == package or module_name.startswith(package + "."):
                return True
        return False

    def admits_import(
        self,
        module_name: str,
        path: Path,
        *,
        from_entry_path: bool,
    ) -> bool:
        if self._external_root_for_path(path) is None:
            return True
        return from_entry_path or self._package_admitted(module_name)

    def admits_package_parent(
        self,
        module_name: str,
        path: Path,
        *,
        existing_modules: Collection[str],
    ) -> bool:
        if self._external_root_for_path(path) is None:
            return True
        if self._package_admitted(module_name):
            return True
        prefix = module_name + "."
        return any(name.startswith(prefix) for name in existing_modules)

    def digest_payload(self) -> dict[str, Any]:
        return {
            "external_roots": [str(root) for root in self.external_roots],
            "admitted_external_packages": sorted(self.admitted_external_packages),
            "native_artifact_plan": self.native_artifact_plan.digest_payload(),
        }


@dataclass(frozen=True)
class _PreparedEntryModuleGraph:
    stdlib_allowlist: set[str]
    roots: list[Path]
    module_resolution_cache: "_ModuleResolutionCache"
    module_graph: dict[str, Path]
    explicit_imports: set[str]
    runtime_import_dispatch_roots: frozenset[str]
    stub_parents: set[str]
    spawn_enabled: bool
    runtime_import_support_policy: _RuntimeImportSupportPolicy
    native_artifact_plan: _ExternalPackageNativeArtifactPlan


@dataclass(frozen=True)
class _ResolvedBuildEntry:
    source_path: Path
    entry_module: str
    module_roots: list[Path]
    entry_source: str
    entry_tree: ast.AST
    target_python: TargetPythonVersion
    external_module_roots: tuple[Path, ...] = ()


@dataclass(frozen=True)
class _PreparedBuildModuleOutputs:
    import_plan: "_ImportPlan"
    output_layout: _BuildOutputLayout


@dataclass(frozen=True)
class _ImportPlan:
    stdlib_allowlist: frozenset[str]
    roots: tuple[Path, ...]
    stdlib_root: Path
    module_resolution_cache: "_ModuleResolutionCache"
    module_graph: Mapping[str, Path]
    explicit_imports: frozenset[str]
    runtime_import_dispatch_roots: frozenset[str]
    stub_parents: frozenset[str]
    spawn_enabled: bool
    runtime_import_support_policy: _RuntimeImportSupportPolicy
    namespace_module_names: frozenset[str]
    generated_module_source_paths: Mapping[str, str]
    known_modules: frozenset[str]
    known_modules_sorted: tuple[str, ...]
    stdlib_allowlist_sorted: tuple[str, ...]
    module_graph_metadata: _ModuleGraphMetadata
    native_artifact_plan: _ExternalPackageNativeArtifactPlan


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
    capabilities_list: list[str] | None
    capability_profiles: list[str]
    capabilities_source: str | None
    manifest_env_vars: dict[str, str]
    capability_config_cache_digest: str
    target_python: TargetPythonVersion


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
    native_arch_perf_enabled: bool


@dataclass(frozen=True)
class _PreparedBuildRoots:
    cwd_root: Path
    project_root: Path
    molt_root: Path
    sysroot_path: Path | None


@dataclass(frozen=True)
class _PreparedFrontendAnalysis:
    module_graph_metadata: _ModuleGraphMetadata
    module_deps: dict[str, set[str]]
    module_sources: dict[str, str]
    module_source_catalog: _ModuleSourceCatalog
    known_func_defaults: dict[str, dict[str, dict[str, Any]]]
    known_func_kinds: dict[str, dict[str, str]]
    module_trees: dict[str, ast.AST]
    module_path_stats: dict[str, os.stat_result | None]
    syntax_error_modules: dict[str, "ModuleSyntaxErrorInfo"]
    module_order: list[str]
    reverse_module_deps: dict[str, set[str]]
    has_back_edges: bool
    module_layers: list[list[str]]
    module_dep_closures: dict[str, frozenset[str]]
    dirty_lowering_modules: set[str]


@dataclass(frozen=True)
class _PreparedFrontendLoweringConfig:
    type_facts: TypeFacts | None
    known_classes: dict[str, Any]
    scoped_lowering_inputs: _ScopedLoweringInputs
    module_graph_metadata: _ModuleGraphMetadata
    frontend_module_costs: Mapping[str, float]
    stdlib_like_by_module: Mapping[str, bool]
    enable_phi: bool
    module_chunk_max_ops: int
    module_chunking: bool
    frontend_parallel_config: _FrontendParallelConfig
    frontend_parallel_layers: list[dict[str, Any]]
    frontend_parallel_worker_timings: list[dict[str, Any]]


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
    runtime_state: _RuntimeArtifactState
    cache_setup: _BackendCacheSetup
    cache_hit: bool
    cache_hit_tier: str | None
    cache_key: str | None
    function_cache_key: str | None
    cache_path: Path | None
    function_cache_path: Path | None
    stdlib_object_path: Path | None
    cache_candidates: list[tuple[str, Path]]


@dataclass(frozen=True)
class _PreparedBackendDispatch:
    backend_env: dict[str, str] | None
    reloc_requested: bool
    backend_bin: Path
    daemon_socket: Path | None
    daemon_ready: bool
    backend_daemon_config_digest: str | None


@dataclass(frozen=True)
class _BackendExecutionResult:
    backend_daemon_cached: bool | None
    backend_daemon_cache_tier: str | None
    backend_daemon_health: dict[str, Any] | None


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


@dataclass(frozen=True)
class _PreparedBackendRuntimeContext:
    runtime_state: _RuntimeArtifactState
    runtime_lib: Path | None
    runtime_wasm: Path | None
    runtime_reloc_wasm: Path | None
    ensure_runtime_wasm_shared: Callable[[set[str] | frozenset[str] | None], bool]
    ensure_runtime_wasm_reloc: Callable[[set[str] | frozenset[str] | None], bool]
    cache_setup: _BackendCacheSetup
    cache_hit: bool
    cache_hit_tier: str | None
    cache_key: str | None
    function_cache_key: str | None
    cache_path: Path | None
    function_cache_path: Path | None
    stdlib_object_path: Path | None


@dataclass(frozen=True)
class _PreparedBackendIR:
    ir: dict[str, Any]


@dataclass(frozen=True)
class _PreparedNonNativeResult:
    primary_output: Path
    consumer_output: Path
    bundle_root: Path | None
    linked_output_path: Path | None
    success_messages: list[str]
    extra_fields: dict[str, Any]
    artifacts: dict[str, str] | None


@dataclass(frozen=True)
class _WrapperBuildContract:
    output: Path
    consumer_output: Path
    bundle_root: Path | None
    artifacts: dict[str, Path]


@dataclass(frozen=True)
class _PreparedNativeLink:
    output_obj: Path
    stub_path: Path
    runtime_lib: Path
    output_binary: Path
    external_native_artifacts: tuple[_StagedExternalPackageNativeArtifact, ...]
    link_cmd: list[str]
    linker_hint: str | None
    normalized_target: str | None
    link_fingerprint_path: Path
    link_fingerprint: dict[str, str | None] | None
    link_skipped: bool
    link_process: subprocess.CompletedProcess[str]


@dataclass(frozen=True)
class _PreparedBuildCallbacks:
    record_frontend_timing: Callable[..., None]
    build_diagnostics_payload: Callable[[], tuple[dict[str, Any] | None, Path | None]]


class _ModuleLowerError(RuntimeError):
    def __init__(self, message: str, *, timed_out: bool = False) -> None:
        super().__init__(message)
        self.timed_out = timed_out
