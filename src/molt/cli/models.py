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
    from molt.cli.module_graph import ModuleSyntaxErrorInfo
    from molt.cli.module_resolution import _ModuleResolutionCache
    from molt.cli.module_source import _ModuleSourceCatalog

ParseCodec = Literal["msgpack", "cbor", "json"]
TypeHintPolicy = Literal["ignore", "trust", "check"]
FallbackPolicy = Literal["error", "bridge"]
BuildProfile = Literal["dev", "release"]
EmitMode = Literal["bin", "obj", "wasm"]
Target = str
ImportScanMode = Literal["full", "module_init", "module_init_static_helpers"]
BinaryImageKind = Literal[
    "entry_script",
    "entry_module",
    "entry_package",
    "project_entry_script",
    "project_entry_module",
    "project_entry_package",
]
BinaryImageClosureMode = Literal["reachable_only"]
BuildEntrySelectorOrigin = Literal["cli", "config", "legacy"]
BuildEntrySelectorTarget = Literal["file", "module"]


@dataclass(frozen=True)
class _BuildEntrySelector:
    origin: BuildEntrySelectorOrigin
    target: BuildEntrySelectorTarget
    value: str
    source: str
    config_key: str | None = None

    @property
    def file_path(self) -> str | None:
        return self.value if self.target == "file" else None

    @property
    def module(self) -> str | None:
        return self.value if self.target == "module" else None


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
    native_callable_exports: dict[str, dict[str, Any]] = field(
        default_factory=dict
    )
    native_python_exports: tuple[str, ...] = ()
    native_support_function_roots: tuple[str, ...] = ()
    known_modules_payload: list[str] = field(default_factory=list)
    known_modules_set: frozenset[str] = field(default_factory=frozenset)
    direct_call_modules_payload: list[str] = field(default_factory=list)
    direct_call_modules_set: frozenset[str] = field(default_factory=frozenset)
    native_callable_exports_payload: dict[str, dict[str, Any]] = field(
        default_factory=dict
    )
    native_python_exports_payload: list[str] = field(default_factory=list)
    native_python_exports_set: frozenset[str] = field(default_factory=frozenset)
    native_support_function_roots_payload: list[str] = field(default_factory=list)
    native_support_function_roots_set: frozenset[str] = field(
        default_factory=frozenset
    )
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
        if (
            not self.native_callable_exports_payload
            and self.native_callable_exports
        ):
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
        if (
            not self.native_support_function_roots_payload
            and self.native_support_function_roots
        ):
            object.__setattr__(
                self,
                "native_support_function_roots_payload",
                list(self.native_support_function_roots),
            )
        if (
            not self.native_support_function_roots_set
            and self.native_support_function_roots
        ):
            object.__setattr__(
                self,
                "native_support_function_roots_set",
                frozenset(self.native_support_function_roots),
            )
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
    direct_call_modules: Collection[str]
    stdlib_allowlist: Collection[str]
    known_func_defaults: dict[str, dict[str, dict[str, Any]]]
    known_func_kinds: dict[str, dict[str, str]]
    native_callable_exports: Mapping[str, Mapping[str, Any]]
    native_python_exports: Collection[str]
    module_deps: dict[str, set[str]]
    source_modules: Collection[str]
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
    direct_call_modules: Collection[str]
    known_classes: Mapping[str, Any]
    stdlib_allowlist: Collection[str]
    known_func_defaults: dict[str, dict[str, dict[str, Any]]]
    known_func_kinds: dict[str, dict[str, str]]
    native_callable_exports: Mapping[str, Mapping[str, Any]]
    native_python_exports: Collection[str]
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
    extra_runtime_features: tuple[str, ...] = ()
    runtime_wasm_ready: bool = False
    runtime_reloc_wasm_ready: bool = False
    runtime_wasm_ready_export_sets: set[frozenset[str] | None] = field(
        default_factory=set
    )
    runtime_reloc_wasm_ready_export_sets: set[frozenset[str] | None] = field(
        default_factory=set
    )
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
    image_scope: _BinaryImageScope | None
    binary_image_closure: Mapping[str, Any] | None
    binary_image_analysis: Mapping[str, Any] | None
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
    init_symbol: str = ""
    runtime_linkage: str = "host_resolved"
    artifact_kind: str = "shared_library"
    support_file_sha256: tuple[tuple[str, str], ...] = ()
    provided_capsules: tuple[str, ...] = ()
    required_capsules: tuple[str, ...] = ()
    python_exports: tuple[str, ...] = ()
    callable_exports: tuple["_ExternalNativeCallableExport", ...] = ()
    abi_symbols: tuple["_ExternalNativeAbiSymbol", ...] = ()
    c_api_symbols: tuple["_ExternalNativeCapiSymbol", ...] = ()

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
            "init_symbol": self.init_symbol,
            "runtime_linkage": self.runtime_linkage,
            "artifact_kind": self.artifact_kind,
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
            "abi_symbols": [
                symbol.digest_payload() for symbol in self.abi_symbols
            ],
            "c_api_symbols": [
                symbol.digest_payload() for symbol in self.c_api_symbols
            ],
        }


@dataclass(frozen=True)
class _ExternalNativeAbiSymbol:
    symbol: str
    status: str
    primitive_class: str
    source: str

    def digest_payload(self) -> dict[str, Any]:
        return {
            "symbol": self.symbol,
            "status": self.status,
            "primitive_class": self.primitive_class,
            "source": self.source,
        }


@dataclass(frozen=True)
class _ExternalNativeCapiSymbol:
    symbol: str
    status: str
    primitive_class: str
    source: str

    def digest_payload(self) -> dict[str, Any]:
        return {
            "symbol": self.symbol,
            "status": self.status,
            "primitive_class": self.primitive_class,
            "source": self.source,
        }


@dataclass(frozen=True)
class _ExternalNativeCallableExport:
    module: str
    name: str
    binding: str
    abi: str
    symbol: str | None = None
    provider_module: str | None = None
    effects: tuple[str, ...] = ()
    deterministic: bool = False

    @property
    def qualified_name(self) -> str:
        return f"{self.module}.{self.name}"

    def digest_payload(self) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "module": self.module,
            "name": self.name,
            "binding": self.binding,
            "abi": self.abi,
            "effects": list(self.effects),
            "deterministic": self.deterministic,
        }
        if self.symbol is not None:
            payload["symbol"] = self.symbol
        if self.provider_module is not None:
            payload["provider_module"] = self.provider_module
        return payload


@dataclass(frozen=True)
class _ExternalNativeModuleAttrPublishSpec:
    provider_module: str
    attr: str


@dataclass(frozen=True)
class _ExternalNativeModuleInitSpec:
    module: str
    init_symbol: str = ""
    module_attr_exports: tuple[_ExternalNativeModuleAttrPublishSpec, ...] = ()

    @property
    def is_extension(self) -> bool:
        return bool(self.init_symbol)


@dataclass(frozen=True)
class _ExternalPackageNativeArtifactPlan:
    artifacts: tuple[_ExternalPackageNativeArtifact, ...] = ()

    def digest_payload(self) -> dict[str, Any]:
        return {"artifacts": [artifact.digest_payload() for artifact in self.artifacts]}

    @staticmethod
    def _module_prefixes(name: str) -> set[str]:
        parts = name.split(".")
        return {".".join(parts[:idx]) for idx in range(1, len(parts) + 1)}

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
            normalized = rel_path.replace("\\", "/")
            if not normalized.endswith(".py"):
                continue
            parts = normalized.split("/")
            if not parts:
                continue
            if parts[-1] == "__init__.py":
                continue
            module_parts = [*parts[:-1], parts[-1][:-3]]
            if module_parts:
                names.add(".".join(module_parts))
        return names

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
            and tuple(package_source_root.parts[-len(package_parts) :])
            == package_parts
        ):
            package_source_root = package_source_root.parents[
                len(package_parts) - 1
            ]
        return package_source_root

    def package_source_roots(self) -> tuple[Path, ...]:
        roots = {
            self._package_source_root(artifact.package_dir, artifact.package)
            for artifact in self.artifacts
        }
        return tuple(sorted(roots))

    def support_source_paths_by_module(self) -> dict[str, Path]:
        sources: dict[str, Path] = {}
        for artifact in self.artifacts:
            package_source_root = self._package_source_root(
                artifact.package_dir,
                artifact.package,
            )
            for rel_path, _digest in artifact.support_file_sha256:
                normalized = rel_path.replace("\\", "/")
                if not normalized.endswith(".py"):
                    continue
                parts = normalized.split("/")
                if not parts:
                    continue
                if parts[-1] == "__init__.py":
                    continue
                module_parts = [*parts[:-1], parts[-1][:-3]]
                if module_parts:
                    sources[".".join(module_parts)] = (
                        package_source_root / Path(normalized)
                    ).resolve()
        return {module: sources[module] for module in sorted(sources)}

    def support_source_module_names(self) -> frozenset[str]:
        return frozenset(self.support_source_paths_by_module())

    def native_module_names(self) -> frozenset[str]:
        names: set[str] = set()
        for artifact in self.artifacts:
            names.update(self._module_prefixes(artifact.package))
            names.update(self._module_prefixes(artifact.module))
            support_init_modules = self._support_init_module_names(artifact)
            names.update(support_init_modules)
            support_source_modules = self._support_python_module_names(artifact)
            for module_name in support_source_modules:
                names.update(self._module_prefixes(module_name))
            for exported_name in artifact.python_exports:
                parts = exported_name.split(".")
                names.update(".".join(parts[:idx]) for idx in range(1, len(parts)))
                if (
                    exported_name == artifact.package
                    or exported_name == artifact.module
                    or exported_name in support_init_modules
                ):
                    names.add(exported_name)
            exported_names = (export.qualified_name for export in artifact.callable_exports)
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
        exports: dict[str, dict[str, Any]] = {}
        for artifact in self.artifacts:
            for export in artifact.callable_exports:
                exports[export.qualified_name] = export.digest_payload()
        return {name: exports[name] for name in sorted(exports)}

    def native_module_init_specs(self) -> tuple[_ExternalNativeModuleInitSpec, ...]:
        specs: dict[str, _ExternalNativeModuleInitSpec] = {}
        module_attr_exports: dict[
            str, set[_ExternalNativeModuleAttrPublishSpec]
        ] = {}
        for artifact in self.artifacts:
            names = set(self._module_prefixes(artifact.package))
            names.update(self._module_prefixes(artifact.module))
            names.update(self._support_init_module_names(artifact))
            for exported_name in artifact.python_exports:
                parts = exported_name.split(".")
                names.update(".".join(parts[:idx]) for idx in range(1, len(parts)))
                if exported_name in {artifact.package, artifact.module}:
                    names.add(exported_name)
            for export in artifact.callable_exports:
                parts = export.qualified_name.split(".")
                names.update(".".join(parts[:idx]) for idx in range(1, len(parts)))
                provider_module = export.provider_module or artifact.module
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
            for name in names:
                if name == artifact.module and artifact.init_symbol:
                    specs[name] = _ExternalNativeModuleInitSpec(
                        module=name,
                        init_symbol=artifact.init_symbol,
                    )
                elif name not in specs:
                    specs[name] = _ExternalNativeModuleInitSpec(module=name)
        for module, exports in module_attr_exports.items():
            existing = specs.get(module, _ExternalNativeModuleInitSpec(module=module))
            specs[module] = _ExternalNativeModuleInitSpec(
                module=existing.module,
                init_symbol=existing.init_symbol,
                module_attr_exports=tuple(
                    sorted(exports, key=lambda item: (item.provider_module, item.attr))
                ),
            )
        return tuple(
            specs[name]
            for name in sorted(specs, key=lambda value: (value.count("."), value))
        )

    @staticmethod
    def _name_reaches_provider(requested_name: str, provider_name: str) -> bool:
        return requested_name == provider_name or requested_name.startswith(
            provider_name + "."
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
            providers = (
                artifact.module,
                *artifact.python_exports,
                *(export.qualified_name for export in artifact.callable_exports),
            )
            if any(
                self._name_reaches_provider(requested_name, provider_name)
                for requested_name in requested
                for provider_name in providers
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
    init_symbol: str = ""
    runtime_linkage: str = "host_resolved"
    artifact_kind: str = "shared_library"
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
            "abi_symbols": [
                symbol.digest_payload() for symbol in self.abi_symbols
            ],
            "c_api_symbols": [
                symbol.digest_payload() for symbol in self.c_api_symbols
            ],
        }


@dataclass(frozen=True)
class _ImportAdmissionPolicy:
    external_roots: tuple[Path, ...] = ()
    admitted_external_packages: frozenset[str] = frozenset()
    native_artifact_source_packages: frozenset[str] = frozenset()
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
        native_artifact_source_packages = frozenset(
            name.strip()
            for name in self.native_artifact_source_packages
            if name and name.strip()
        )
        object.__setattr__(self, "external_roots", external_roots)
        object.__setattr__(self, "admitted_external_packages", admitted)
        object.__setattr__(
            self,
            "native_artifact_source_packages",
            native_artifact_source_packages,
        )

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
        return (
            self._external_root_for_path(path) is not None
            and self._native_artifact_source_package(module_name) is not None
        )

    def admits_import(
        self,
        module_name: str,
        path: Path,
        *,
        from_entry_path: bool,
    ) -> bool:
        if self._external_root_for_path(path) is None:
            return True
        if self._native_artifact_support_source_module(module_name):
            return True
        if self._native_artifact_source_package(module_name) is not None:
            return False
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
        if self._external_root_for_path(path) is None:
            return True
        if self._native_artifact_source_package(module_name) is not None:
            prefix = module_name + "."
            return any(name.startswith(prefix) for name in existing_modules)
        if self._package_admitted(module_name):
            return True
        prefix = module_name + "."
        return any(name.startswith(prefix) for name in existing_modules)

    def digest_payload(self) -> dict[str, Any]:
        return {
            "external_roots": [str(root) for root in self.external_roots],
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
class _PreparedBuildModuleOutputs:
    import_plan: "_ImportPlan"
    output_layout: _BuildOutputLayout


@dataclass(frozen=True)
class _ImportPlan:
    image_scope: _BinaryImageScope
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

    def with_compile_modules(self, compile_modules: Collection[str]) -> "_ImportPlan":
        compile_set = frozenset(compile_modules)
        unknown = sorted(compile_set - frozenset(self.module_graph))
        if unknown:
            raise ValueError(
                "compile module set contains modules without source graph entries: "
                + ", ".join(unknown)
            )
        return _ImportPlan(
            image_scope=self.image_scope,
            stdlib_allowlist=self.stdlib_allowlist,
            roots=self.roots,
            stdlib_root=self.stdlib_root,
            module_resolution_cache=self.module_resolution_cache,
            module_graph=self.module_graph,
            explicit_imports=self.explicit_imports,
            runtime_import_dispatch_roots=self.runtime_import_dispatch_roots,
            stub_parents=self.stub_parents,
            spawn_enabled=self.spawn_enabled,
            runtime_import_support_policy=self.runtime_import_support_policy,
            namespace_module_names=self.namespace_module_names,
            generated_module_source_paths=self.generated_module_source_paths,
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
            "runtime_import_dispatch_roots": sorted(
                self.runtime_import_dispatch_roots
            ),
            "stub_parents": sorted(self.stub_parents),
            "namespace_module_names": sorted(self.namespace_module_names),
            "spawn_enabled": self.spawn_enabled,
            "runtime_import_support": {
                "needs_generated_importer": (
                    self.runtime_import_support_policy.needs_generated_importer
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
    set_binary_image_closure_payload: Callable[[Mapping[str, Any] | None], None]
    record_binary_image_analysis: Callable[[str, Mapping[str, Any] | None], None]


class _ModuleLowerError(RuntimeError):
    def __init__(self, message: str, *, timed_out: bool = False) -> None:
        super().__init__(message)
        self.timed_out = timed_out
