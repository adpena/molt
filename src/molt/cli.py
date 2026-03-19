import argparse
import ast
import base64
import codecs
from collections import deque
import copy
import contextlib
from concurrent.futures import ProcessPoolExecutor
import errno
import datetime as dt
import functools
import hashlib
import http.client
import io
import tempfile
import json
import os
import platform
import posixpath
import re
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import tomllib
import time
import threading
import tracemalloc
import tokenize
import urllib.parse
import urllib.request
import uuid
import zipfile
from contextlib import contextmanager, nullcontext, redirect_stderr, redirect_stdout
from dataclasses import dataclass, field
from pathlib import Path
from typing import (
    Any,
    Callable,
    Collection,
    ContextManager,
    Iterable,
    Iterator,
    Literal,
    Mapping,
    MutableMapping,
    NamedTuple,
    Sequence,
    cast,
)

from packaging.markers import InvalidMarker, Marker
from packaging.requirements import InvalidRequirement, Requirement
from molt.compat import CompatibilityError
from molt.frontend import MoltValue, SimpleTIRGenerator
from molt.type_facts import (
    TypeFacts,
    collect_type_facts_from_paths,
    load_type_facts,
    write_type_facts,
)

Target = str
ParseCodec = Literal["msgpack", "cbor", "json"]
TypeHintPolicy = Literal["ignore", "trust", "check"]
FallbackPolicy = Literal["error", "bridge"]
BuildProfile = Literal["dev", "release"]
EmitMode = Literal["bin", "obj", "wasm"]
STUB_MODULES = {"molt_buffer", "molt_cbor", "molt_json", "molt_msgpack"}
STUB_PARENT_MODULES = {"molt"}
# Stdlib modules that rely on nested imports for required runtime semantics.
STDLIB_NESTED_IMPORT_SCAN_MODULES = {
    "collections",
    "typing",
    # EmailMessage lazily imports email.policy inside __init__.
    "email.message",
}
ENTRY_OVERRIDE_ENV = "MOLT_ENTRY_MODULE"
ENTRY_OVERRIDE_SPAWN = "multiprocessing.spawn"
IMPORTER_MODULE_NAME = "_molt_importer"
JSON_SCHEMA_VERSION = "1.0"
REMOTE_REGISTRY_SCHEMES = {"http", "https"}
_ARTIFACT_SYNC_STATE_CACHE: dict[Path, tuple[int, int, dict[str, Any] | None]] = {}
_PERSISTED_JSON_OBJECT_CACHE: dict[Path, tuple[int, int, dict[str, Any] | None]] = {}
_LOCK_CHECK_CACHE_VERSION = 1
_HASH_SEED_SENTINEL_ENV = "MOLT_HASH_SEED_APPLIED"
_HASH_SEED_OVERRIDE_ENV = "MOLT_HASH_SEED"
_BACKEND_DAEMON_PROTOCOL_VERSION = 1
_BACKEND_CODEGEN_ENV_DIGEST_SCHEMA_VERSION = 1
_DAEMON_CONFIG_DIGEST_SCHEMA_VERSION = 1
_NATIVE_CODEGEN_ENV_KNOBS = (
    "MOLT_BACKEND_REGALLOC_ALGORITHM",
    "MOLT_BACKEND_MIN_FUNCTION_ALIGNMENT_LOG2",
    "MOLT_BACKEND_LIBCALL_CALL_CONV",
    "MOLT_BACKEND_ENABLE_VERIFIER",
    "MOLT_DISABLE_STRUCT_ELIDE",
    "MOLT_PORTABLE",
)
_WASM_CODEGEN_ENV_KNOBS = (
    "MOLT_WASM_DATA_BASE",
    "MOLT_WASM_MIN_PAGES",
    "MOLT_WASM_LINK",
    "MOLT_WASM_TABLE_BASE",
)
CAPABILITY_PROFILES: dict[str, list[str]] = {
    "core": [],
    "fs": ["fs.read", "fs.write"],
    "env": ["env.read", "env.write"],
    "net": ["net", "websocket.connect", "websocket.listen"],
    "db": ["db.read", "db.write"],
    "time": ["time"],
    "random": ["random"],
}
CAPABILITY_TOKEN_RE = re.compile(r"^[a-z0-9][a-z0-9._-]*$")
_CARGO_PROFILE_NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]*$")
_OUTPUT_BASE_SAFE_RE = re.compile(r"[^A-Za-z0-9._-]+")
_ABI_VERSION_RE = re.compile(r"^(\d+)\.(\d+)(?:\.(\d+))?$")
_MOLT_C_API_VERSION_RE = re.compile(r"^\d+(?:\.\d+){0,2}$")
_WHEEL_TOKEN_RE = re.compile(r"[^A-Za-z0-9_.]+")
_WHEEL_VERSION_RE = re.compile(r"[^A-Za-z0-9._]+")
_PY_IDENTIFIER_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
_PY_C_API_TOKEN_RE = re.compile(r"\bPy[A-Za-z_][A-Za-z0-9_]*\b")
_C_BLOCK_COMMENT_RE = re.compile(r"/\*.*?\*/", flags=re.DOTALL)
_C_LINE_COMMENT_RE = re.compile(r"//.*?$", flags=re.MULTILINE)
_C_STRING_LITERAL_RE = re.compile(r'"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'')
_SUPPORTED_PKG_ABI_MAJOR = 0
_SUPPORTED_PKG_ABI_MINOR = 1
_SUPPORTED_PKG_ABI = f"{_SUPPORTED_PKG_ABI_MAJOR}.{_SUPPORTED_PKG_ABI_MINOR}"
CapabilityInput = str | list[str] | dict[str, Any]


def _dedupe_preserve_order(items: Iterable[str]) -> list[str]:
    seen: set[str] = set()
    deduped: list[str] = []
    for item in items:
        if item in seen:
            continue
        seen.add(item)
        deduped.append(item)
    return deduped


def _split_tokens(value: str) -> list[str]:
    return [token for token in re.split(r"[,\s]+", value) if token]


@dataclass(frozen=True)
class CapabilityGrant:
    allow: list[str] | None
    deny: list[str]
    effects: list[str] | None

    def merged(self, other: "CapabilityGrant") -> "CapabilityGrant":
        allow = _merge_optional_list(self.allow, other.allow)
        deny = _dedupe_preserve_order([*self.deny, *other.deny])
        effects = _merge_optional_list(self.effects, other.effects)
        return CapabilityGrant(allow=allow, deny=deny, effects=effects)


@dataclass(frozen=True)
class CapabilityManifest:
    allow: list[str] | None
    deny: list[str]
    effects: list[str] | None
    packages: dict[str, CapabilityGrant]


@dataclass(frozen=True)
class CapabilitySpec:
    capabilities: list[str] | None
    profiles: list[str]
    source: str | None
    errors: list[str]
    manifest: CapabilityManifest | None


@dataclass(frozen=True)
class PgoProfileSummary:
    version: str
    hash: str
    hot_functions: list[str]


@dataclass(frozen=True)
class RuntimeFeedbackSummary:
    schema_version: int
    hash: str
    hot_functions: list[str]


@dataclass(frozen=True)
class ExtensionManifestValidation:
    errors: list[str]
    warnings: list[str]
    wheel_path: Path | None
    abi_version: str
    abi_tag: str | None
    capabilities: list[str]
    wheel_tags: tuple[str, str, str] | None


def _emit_json(payload: dict[str, Any], json_output: bool) -> None:
    if json_output:
        print(json.dumps(payload))


def _json_payload(
    command: str,
    status: str,
    *,
    data: dict[str, Any] | None = None,
    warnings: list[str] | None = None,
    errors: list[str] | None = None,
) -> dict[str, Any]:
    payload = {
        "schema_version": JSON_SCHEMA_VERSION,
        "command": command,
        "status": status,
        "data": data or {},
        "warnings": warnings or [],
        "errors": errors or [],
    }
    return payload


def _fail(
    message: str,
    json_output: bool,
    code: int = 2,
    command: str = "molt",
) -> int:
    if json_output:
        payload = _json_payload(
            command,
            "error",
            data={"returncode": code},
            errors=[message],
        )
        _emit_json(payload, json_output=True)
    else:
        print(message, file=sys.stderr)
    return code


def _write_importer_module(module_names: list[str], output_dir: Path) -> Path:
    filtered_names = [name for name in module_names if name]
    known_modules = sorted(filtered_names)
    top_level_by_module = {
        name: name.split(".", 1)[0] for name in known_modules if "." in name
    }
    lines = [
        '"""Auto-generated import dispatcher for Molt-compiled modules."""',
        "",
        "from __future__ import annotations",
        "",
    ]
    lines.extend(
        [
            "import sys as _sys",
            "from _intrinsics import require_intrinsic as _require_intrinsic",
            "",
            f"_KNOWN_MODULES = frozenset({known_modules!r})",
            f"_TOP_LEVEL_BY_MODULE = {top_level_by_module!r}",
            "_MODULE_IMPORT = _require_intrinsic('molt_module_import', globals())",
            "",
            "def _resolve_name(name: str, package: str | None, level: int) -> str:",
            "    if level <= 0:",
            "        return name",
            "    if not package:",
            '        raise ImportError("relative import requires package")',
            '    parts = package.split(".")',
            "    if level > len(parts):",
            '        raise ImportError("attempted relative import beyond top-level package")',
            "    cut = len(parts) - level + 1",
            '    base = ".".join(parts[:cut])',
            "    if name:",
            '        return f"{base}.{name}" if base else name',
            "    return base",
            "",
            "def _molt_import(name, globals=None, locals=None, fromlist=(), level=0):",
            "    if not name:",
            '        raise ImportError("Empty module name")',
            "    package = None",
            "    if isinstance(globals, dict):",
            '        package = globals.get("__package__")',
            '        if not package and globals.get("__path__") and globals.get("__name__"):',
            '            package = globals.get("__name__")',
            "    resolved = _resolve_name(name, package, level) if level else name",
            '    modules = getattr(_sys, "modules", {})',
            "    if resolved in modules:",
            "        mod = modules[resolved]",
            "        if mod is None:",
            '            raise ImportError(f"import of {resolved} halted; None in sys.modules")',
            "        if fromlist:",
            "            return mod",
            "        top = _TOP_LEVEL_BY_MODULE.get(resolved, resolved)",
            "        return modules.get(top, mod)",
            "    if resolved not in _KNOWN_MODULES:",
            "        raise ImportError(f\"No module named '{resolved}'\")",
            "    mod = _MODULE_IMPORT(resolved)",
            "    if mod is None:",
            "        raise ImportError(f\"No module named '{resolved}'\")",
            "    if fromlist:",
            "        return mod",
            "    top = _TOP_LEVEL_BY_MODULE.get(resolved, resolved)",
            "    return modules.get(top, mod)",
        ]
    )
    path = output_dir / f"{IMPORTER_MODULE_NAME}.py"
    _write_text_if_changed(path, "\n".join(lines) + "\n")
    return path


def _collect_env_overrides(file_path: str) -> dict[str, str]:
    overrides: dict[str, str] = {}
    try:
        text = Path(file_path).read_text()
    except OSError:
        return overrides
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("# MOLT_ENV:"):
            continue
        payload = stripped[len("# MOLT_ENV:") :].strip()
        for token in payload.split():
            if "=" not in token:
                continue
            key, value = token.split("=", 1)
            overrides[key] = value
    return overrides


def _resolve_python_exe(python_exe: str | None) -> str:
    if not python_exe:
        return sys.executable
    if python_exe[0].isdigit() and os.sep not in python_exe:
        python_exe = f"python{python_exe}"
    if os.sep in python_exe or Path(python_exe).is_absolute():
        candidate = Path(python_exe)
        if candidate.exists():
            return python_exe
        base_exe = getattr(sys, "_base_executable", "")
        if base_exe and Path(base_exe).exists():
            return base_exe
    return python_exe


def _vendor_roots(project_root: Path) -> list[Path]:
    vendor_root = project_root / "vendor"
    roots: list[Path] = []
    for name in ("packages", "local"):
        candidate = vendor_root / name
        if candidate.exists():
            roots.append(candidate)
    return roots


def _base_env(
    root: Path,
    script_path: Path | None = None,
    *,
    molt_root: Path | None = None,
) -> dict[str, str]:
    env = os.environ.copy()
    paths = [env.get("PYTHONPATH", "")]
    if script_path is not None:
        paths.append(str(script_path.parent))
    roots: list[Path] = []
    if molt_root is not None and molt_root != root:
        roots.append(molt_root)
    roots.append(root)
    for base in roots:
        paths.extend([str(base / "src"), str(base)])
        paths.extend(str(path) for path in _vendor_roots(base))
    env["PYTHONPATH"] = os.pathsep.join(p for p in paths if p)
    env.setdefault("PYTHONHASHSEED", "0")
    if molt_root is not None:
        env.setdefault("MOLT_PROJECT_ROOT", str(molt_root))
    return env


def _run_command(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
    json_output: bool = False,
    verbose: bool = False,
    label: str | None = None,
    warnings: list[str] | None = None,
) -> int:
    cmd = [str(part) for part in cmd]
    if verbose and not json_output:
        print(f"Running: {shlex.join(cmd)}")
    capture = json_output
    result = subprocess.run(
        cmd,
        env=env,
        cwd=cwd,
        capture_output=capture,
        text=True,
    )
    if json_output:
        data: dict[str, Any] = {"returncode": result.returncode}
        if result.stdout:
            data["stdout"] = result.stdout
        if result.stderr:
            data["stderr"] = result.stderr
        payload = _json_payload(
            label or cmd[0],
            "ok" if result.returncode == 0 else "error",
            data=data,
            warnings=warnings,
        )
        _emit_json(payload, json_output=True)
    return result.returncode


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


class _PersistedModuleGraphState(NamedTuple):
    graph: dict[str, Path]
    explicit_imports: set[str]
    dirty_modules: set[str]


@dataclass(frozen=True)
class _ScopedLoweringInputs:
    known_modules_by_module: dict[str, tuple[str, ...]]
    known_func_defaults_by_module: dict[str, dict[str, dict[str, Any]]]
    pgo_hot_function_names_by_module: dict[str, tuple[str, ...]]
    type_facts_by_module: dict[str, TypeFacts | None]


@dataclass(frozen=True)
class _ScopedLoweringInputView:
    known_modules: tuple[str, ...]
    known_func_defaults: dict[str, dict[str, Any]]
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
            object.__setattr__(
                self, "known_modules_set", frozenset(self.known_modules)
            )
        if (
            not self.pgo_hot_function_names_payload
            and self.pgo_hot_function_names
        ):
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
    logical_source_path_by_module: dict[str, str]
    entry_override_by_module: dict[str, str | None]
    module_is_namespace_by_module: dict[str, bool]
    module_is_package_by_module: dict[str, bool]
    frontend_module_costs: dict[str, float] | None
    stdlib_like_by_module: dict[str, bool] | None


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
    module_graph: dict[str, Path]
    module_sources: dict[str, str]
    project_root: Path | None
    module_resolution_cache: "_ModuleResolutionCache"
    parse_codec: "ParseCodec"
    type_hint_policy: "TypeHintPolicy"
    fallback_policy: "FallbackPolicy"
    type_facts: dict[str, Any] | None
    enable_phi: bool
    known_modules: Collection[str]
    stdlib_allowlist: Collection[str]
    known_func_defaults: dict[str, dict[str, Any]]
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


@dataclass(frozen=True)
class _FrontendLayerRuntimeHooks:
    warnings: list[str]
    frontend_parallel_details: MutableMapping[str, Any]
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]]
    record_frontend_timing: Callable[..., None]
    integrate_module_frontend_result: Callable[..., str | None]
    accumulate_midend_diagnostics: Callable[..., None]
    fail: Callable[[str, bool, str], dict[str, Any] | None]
    json_output: bool
    run_serial_frontend_lower: Callable[
        [str, Path],
        tuple[
            dict[str, Any] | None,
            "_FrontendModuleResultTimings | None",
            dict[str, Any] | None,
        ],
    ]


@dataclass(frozen=True)
class _SerialFrontendLoweringContext:
    syntax_error_modules: Mapping[str, Any]
    module_trees: Mapping[str, ast.AST]
    module_sources: Mapping[str, str]
    generated_module_source_paths: Mapping[str, str]
    module_resolution_cache: "_ModuleResolutionCache"
    project_root: Path | None
    dirty_lowering_modules: Collection[str]
    parse_codec: "ParseCodec"
    type_hint_policy: "TypeHintPolicy"
    fallback_policy: "FallbackPolicy"
    type_facts: dict[str, Any] | None
    enable_phi: bool
    known_modules: Collection[str]
    stdlib_allowlist: Collection[str]
    known_func_defaults: dict[str, dict[str, Any]]
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


@dataclass(frozen=True)
class _SerialFrontendLoweringHooks:
    record_frontend_timing: Callable[..., None]
    fail: Callable[[str, bool, str], dict[str, Any] | None]
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
    type_facts: dict[str, Any] | None
    enable_phi: bool
    known_modules: Collection[str]
    known_classes: Mapping[str, Any]
    stdlib_allowlist: Collection[str]
    known_func_defaults: dict[str, dict[str, Any]]
    module_chunking: bool
    module_chunk_max_ops: int
    optimization_profile: str
    pgo_hot_function_names: Collection[str]
    frontend_phase_timeout: float | None


@dataclass
class _RuntimeArtifactState:
    runtime_lib: Path | None = None
    runtime_wasm: Path | None = None
    runtime_reloc_wasm: Path | None = None
    runtime_wasm_ready: bool = False
    runtime_reloc_wasm_ready: bool = False


class _ModuleLowerError(RuntimeError):
    def __init__(self, message: str, *, timed_out: bool = False) -> None:
        super().__init__(message)
        self.timed_out = timed_out


def _fresh_frontend_parallel_layer_state() -> _FrontendParallelLayerState:
    return _FrontendParallelLayerState()


def _run_command_timed(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
    verbose: bool = False,
    capture_output: bool = False,
) -> _TimedResult:
    cmd = [str(part) for part in cmd]
    if verbose:
        print(f"Running: {shlex.join(cmd)}")
    start = time.perf_counter()
    result = subprocess.run(
        cmd,
        env=env,
        cwd=cwd,
        capture_output=capture_output,
        text=True,
    )
    duration = time.perf_counter() - start
    return _TimedResult(
        result.returncode,
        result.stdout or "",
        result.stderr or "",
        duration,
    )


def _format_duration(seconds: float) -> str:
    if seconds < 0:
        seconds = 0.0
    if seconds < 0.001:
        return f"{seconds * 1_000_000:.0f} µs"
    if seconds < 1:
        return f"{seconds * 1000:.1f} ms"
    if seconds < 60:
        return f"{seconds:.3f} s"
    return f"{seconds / 60:.2f} min"


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _git_rev(root: Path) -> str | None:
    try:
        result = subprocess.run(
            ["git", "-C", str(root), "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    value = result.stdout.strip()
    return value or None


def _abi_version_error(value: str) -> str | None:
    cleaned = value.strip()
    match = _ABI_VERSION_RE.match(cleaned)
    if match is None:
        return "abi_version must be MAJOR.MINOR[.PATCH] (e.g., 0.1)"
    major = int(match.group(1))
    minor = int(match.group(2))
    if major != _SUPPORTED_PKG_ABI_MAJOR or minor != _SUPPORTED_PKG_ABI_MINOR:
        return f"unsupported abi_version {cleaned} (supported: {_SUPPORTED_PKG_ABI})"
    return None


def _manifest_errors(manifest: dict[str, Any]) -> list[str]:
    required = [
        "name",
        "version",
        "abi_version",
        "target",
        "capabilities",
        "deterministic",
        "effects",
    ]
    errors: list[str] = []
    for key in required:
        if key not in manifest:
            errors.append(f"missing {key}")
    name = manifest.get("name")
    version = manifest.get("version")
    abi_version = manifest.get("abi_version")
    target = manifest.get("target")
    capabilities = manifest.get("capabilities")
    deterministic = manifest.get("deterministic")
    effects = manifest.get("effects")
    exports = manifest.get("exports")
    if name is not None and not isinstance(name, str):
        errors.append("name must be a string")
    if version is not None and not isinstance(version, str):
        errors.append("version must be a string")
    if abi_version is not None and not isinstance(abi_version, str):
        errors.append("abi_version must be a string")
    if isinstance(abi_version, str):
        abi_error = _abi_version_error(abi_version)
        if abi_error:
            errors.append(abi_error)
    if target is not None and not isinstance(target, str):
        errors.append("target must be a string")
    if capabilities is not None:
        if not isinstance(capabilities, list) or not all(
            isinstance(item, str) for item in capabilities
        ):
            errors.append("capabilities must be a list of strings")
    if deterministic is not None and not isinstance(deterministic, bool):
        errors.append("deterministic must be a boolean")
    if effects is not None and not isinstance(effects, (list, str)):
        errors.append("effects must be a list or string")
    if exports is not None:
        if not isinstance(exports, list) or not all(
            isinstance(item, str) for item in exports
        ):
            errors.append("exports must be a list of strings")
    return errors


def _normalize_effects(value: Any) -> list[str]:
    if value is None:
        return []
    if isinstance(value, list):
        normalized: list[str] = []
        for entry in value:
            if isinstance(entry, str):
                stripped = entry.strip()
                if stripped:
                    normalized.append(stripped)
        return normalized
    if isinstance(value, str):
        return _split_tokens(value)
    return []


def _load_manifest(path: Path) -> dict[str, Any] | None:
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return None


def _write_zip_member(zf: zipfile.ZipFile, name: str, data: bytes) -> None:
    info = zipfile.ZipInfo(name)
    info.date_time = (1980, 1, 1, 0, 0, 0)
    info.compress_type = zipfile.ZIP_DEFLATED
    zf.writestr(info, data)


def _wheel_record_line(path: str, data: bytes) -> str:
    digest = hashlib.sha256(data).digest()
    encoded = base64.urlsafe_b64encode(digest).decode("ascii").rstrip("=")
    return f"{path},sha256={encoded},{len(data)}"


def _coerce_str_list(
    value: Any,
    field: str,
    errors: list[str],
    *,
    allow_empty: bool = True,
) -> list[str]:
    if value is None:
        return []
    if isinstance(value, str):
        stripped = value.strip()
        if stripped:
            return [stripped]
        if allow_empty:
            return []
        errors.append(f"{field} must not be empty")
        return []
    if isinstance(value, list):
        items: list[str] = []
        for entry in value:
            if isinstance(entry, str):
                stripped = entry.strip()
                if stripped:
                    items.append(stripped)
                elif not allow_empty:
                    errors.append(f"{field} must not include empty entries")
            else:
                errors.append(f"{field} entries must be strings")
        return items
    errors.append(f"{field} must be a string or list of strings")
    return []


def _module_parts(module_name: str) -> list[str] | None:
    stripped = module_name.strip()
    if not stripped:
        return None
    parts = stripped.split(".")
    if any(_PY_IDENTIFIER_RE.match(part) is None for part in parts):
        return None
    return parts


def _wheel_token(value: str) -> str:
    cleaned = _WHEEL_TOKEN_RE.sub("_", value.strip())
    cleaned = cleaned.strip("._")
    return cleaned or "unknown"


def _wheel_version_token(value: str) -> str:
    cleaned = _WHEEL_VERSION_RE.sub("_", value.strip())
    cleaned = cleaned.strip("._")
    return cleaned or "0"


def _cpu_baseline(target_triple: str | None) -> str:
    """Return the CPU baseline label for the given target triple.

    When no target-cpu=native is set, Cranelift uses the architecture's
    generic baseline.  This helper returns a human-readable label for
    build metadata.
    """
    triple = (target_triple or _host_target_triple()).lower()
    if triple.startswith("x86_64") or triple.startswith("x86-64"):
        return "x86-64"
    if triple.startswith("aarch64") or triple.startswith("arm64"):
        return "aarch64"
    if triple.startswith("wasm32"):
        return "wasm32"
    return "generic"


def _extension_binary_suffix(target_triple: str | None = None) -> str:
    target = (target_triple or "").strip().lower()
    if "windows" in target:
        return ".pyd"
    if os.name == "nt" and not target:
        return ".pyd"
    return ".so"


def _host_target_triple() -> str:
    system = platform.system().lower()
    arch = platform.machine().lower() or "unknown"
    arch_aliases = {
        "amd64": "x86_64",
        "x86-64": "x86_64",
        "arm64": "aarch64",
    }
    arch = arch_aliases.get(arch, arch)
    if system == "darwin":
        return f"{arch}-apple-darwin"
    if system == "linux":
        return f"{arch}-unknown-linux-gnu"
    if system == "windows":
        return f"{arch}-pc-windows-msvc"
    return f"{arch}-{system}"


def _default_molt_c_api_version(molt_root: Path) -> str:
    header = molt_root / "include" / "molt" / "molt.h"
    try:
        text = header.read_text()
    except OSError:
        return "1"
    match = re.search(
        r"^\s*#\s*define\s+MOLT_C_API_VERSION\s+([0-9]+)u?\s*$",
        text,
        flags=re.MULTILINE,
    )
    if match is None:
        return "1"
    return match.group(1)


def _wheel_filename_tags(path: Path) -> tuple[str, str, str] | None:
    if path.suffix != ".whl":
        return None
    parts = path.stem.split("-")
    if len(parts) < 5:
        return None
    python_tag = parts[-3]
    abi_tag = parts[-2]
    platform_tag = parts[-1]
    if not python_tag or not abi_tag or not platform_tag:
        return None
    return python_tag, abi_tag, platform_tag


def _is_extension_manifest(manifest: Mapping[str, Any]) -> bool:
    extension_keys = {
        "molt_c_api_version",
        "abi_tag",
        "target_triple",
        "platform_tag",
        "module",
        "wheel",
        "extension",
    }
    matched = sum(1 for key in extension_keys if key in manifest)
    return matched >= 2


def _validate_extension_manifest(
    manifest: Mapping[str, Any],
    *,
    manifest_dir: Path,
    wheel_path: Path | None,
    require_capabilities: bool,
    required_abi: str | None,
    require_checksum: bool = False,
    warn_missing_checksum: bool = False,
) -> ExtensionManifestValidation:
    errors: list[str] = []
    warnings: list[str] = []
    required_fields = (
        "molt_c_api_version",
        "capabilities",
        "abi_tag",
        "target_triple",
        "platform_tag",
        "module",
    )
    for field in required_fields:
        if field not in manifest:
            errors.append(f"Missing manifest field: {field}")

    manifest_abi = manifest.get("molt_c_api_version")
    if not isinstance(manifest_abi, str):
        errors.append("molt_c_api_version must be a string")
        manifest_abi = ""
    elif _MOLT_C_API_VERSION_RE.match(manifest_abi.strip()) is None:
        errors.append(
            f"molt_c_api_version must be MAJOR[.MINOR[.PATCH]] (got {manifest_abi!r})"
        )
        manifest_abi = ""
    else:
        manifest_abi = manifest_abi.strip()

    capabilities_value = manifest.get("capabilities")
    manifest_capabilities: list[str] = []
    if not isinstance(capabilities_value, list) or not all(
        isinstance(item, str) for item in capabilities_value
    ):
        errors.append("capabilities must be a list of strings")
    else:
        manifest_capabilities = [
            item.strip() for item in capabilities_value if item.strip()
        ]
    if require_capabilities and not manifest_capabilities:
        errors.append(
            "Capabilities are required but manifest capability list is empty."
        )

    if required_abi is not None and manifest_abi and manifest_abi != required_abi:
        errors.append(
            f"ABI mismatch: required {required_abi}, manifest has {manifest_abi}"
        )

    manifest_abi_tag: str | None = None
    abi_tag_value = manifest.get("abi_tag")
    if isinstance(abi_tag_value, str):
        manifest_abi_tag = abi_tag_value
    if manifest_abi_tag is not None and manifest_abi:
        expected_abi_tag = f"molt_abi{manifest_abi.split('.', 1)[0]}"
        if manifest_abi_tag != expected_abi_tag:
            errors.append(
                f"ABI tag mismatch: expected {expected_abi_tag}, found {manifest_abi_tag}"
            )

    resolved_wheel = wheel_path
    if resolved_wheel is not None:
        resolved_wheel = resolved_wheel.expanduser()
        if not resolved_wheel.is_absolute():
            resolved_wheel = (manifest_dir / resolved_wheel).absolute()

    wheel_field = manifest.get("wheel")
    if resolved_wheel is None and isinstance(wheel_field, str) and wheel_field.strip():
        candidate = Path(wheel_field).expanduser()
        if not candidate.is_absolute():
            candidate = (manifest_dir / candidate).absolute()
        if candidate.exists():
            resolved_wheel = candidate
        else:
            warnings.append(f"Wheel path referenced by manifest not found: {candidate}")

    wheel_tags: tuple[str, str, str] | None = None
    if resolved_wheel is not None and resolved_wheel.exists():
        wheel_tags = _wheel_filename_tags(resolved_wheel)
        if wheel_tags is None:
            errors.append(f"Invalid wheel filename format: {resolved_wheel.name}")
        else:
            _python_tag, wheel_abi_tag, wheel_platform_tag = wheel_tags
            if manifest_abi_tag is not None and wheel_abi_tag != manifest_abi_tag:
                errors.append(
                    f"Wheel ABI tag mismatch: wheel has {wheel_abi_tag}, "
                    f"manifest has {manifest_abi_tag}"
                )
            manifest_platform = manifest.get("platform_tag")
            if (
                isinstance(manifest_platform, str)
                and wheel_platform_tag != manifest_platform
            ):
                errors.append(
                    f"Wheel platform tag mismatch: wheel has {wheel_platform_tag}, "
                    f"manifest has {manifest_platform}"
                )

        expected_wheel_sha = manifest.get("wheel_sha256")
        if isinstance(expected_wheel_sha, str) and expected_wheel_sha.strip():
            actual_wheel_sha = _sha256_file(resolved_wheel)
            if actual_wheel_sha != expected_wheel_sha.strip():
                errors.append("wheel_sha256 does not match wheel contents")
        elif require_checksum:
            errors.append("wheel_sha256 missing")
        elif warn_missing_checksum:
            warnings.append("wheel_sha256 missing")

        extension_entry = manifest.get("extension")
        expected_extension_sha = manifest.get("extension_sha256")
        if isinstance(extension_entry, str) and extension_entry.strip():
            try:
                with zipfile.ZipFile(resolved_wheel) as zf:
                    ext_bytes = zf.read(extension_entry)
            except KeyError:
                errors.append(f"Wheel is missing extension entry: {extension_entry}")
            except (OSError, zipfile.BadZipFile) as exc:
                errors.append(f"Failed to read wheel extension payload: {exc}")
            else:
                if (
                    isinstance(expected_extension_sha, str)
                    and expected_extension_sha.strip()
                ):
                    actual_extension_sha = hashlib.sha256(ext_bytes).hexdigest()
                    if actual_extension_sha != expected_extension_sha.strip():
                        errors.append("extension_sha256 does not match wheel entry")
                elif require_checksum:
                    errors.append("extension_sha256 missing")
                elif warn_missing_checksum:
                    warnings.append("extension_sha256 missing")
        elif require_checksum:
            errors.append("extension path missing")
    else:
        if require_checksum:
            errors.append(
                "wheel artifact required for checksum verification is missing"
            )
        else:
            warnings.append(
                "Wheel artifact not found; wheel tag and checksum checks skipped."
            )

    return ExtensionManifestValidation(
        errors=errors,
        warnings=warnings,
        wheel_path=resolved_wheel,
        abi_version=manifest_abi,
        abi_tag=manifest_abi_tag,
        capabilities=manifest_capabilities,
        wheel_tags=wheel_tags,
    )


def _compiler_metadata() -> tuple[str | None, str | None]:
    compiler_root = Path(__file__).resolve().parents[2]
    pyproject = _load_toml(compiler_root / "pyproject.toml")
    version = pyproject.get("project", {}).get("version")
    git_rev = _git_rev(compiler_root)
    return version if isinstance(version, str) else None, git_rev


def _sbom_component_hashes(pkg: dict[str, Any]) -> list[dict[str, str]]:
    digests: set[str] = set()
    sdist = pkg.get("sdist")
    if isinstance(sdist, dict):
        digest = sdist.get("hash", "")
        if isinstance(digest, str) and digest:
            digests.add(digest)
    for wheel in pkg.get("wheels", []):
        if not isinstance(wheel, dict):
            continue
        digest = wheel.get("hash", "")
        if isinstance(digest, str) and digest:
            digests.add(digest)
    hashes: list[dict[str, str]] = []
    for entry in sorted(digests):
        if ":" in entry:
            algo, digest = entry.split(":", 1)
        else:
            algo, digest = "sha256", entry
        if digest:
            hashes.append({"alg": algo.upper(), "content": digest})
    return hashes


def _sbom_component_for_lock_pkg(
    pkg: dict[str, Any],
    allow: dict[str, set[str]],
) -> dict[str, Any] | None:
    name = pkg.get("name")
    if not isinstance(name, str) or not name.strip():
        return None
    source = pkg.get("source", {})
    if isinstance(source, dict) and source.get("virtual") == ".":
        return None
    version = pkg.get("version")
    if not isinstance(version, str):
        version = None
    norm = _normalize_name(name)
    purl = f"pkg:pypi/{norm}"
    if version:
        purl = f"{purl}@{version}"
    tier, reason = _classify_tier(name, pkg, allow)
    component: dict[str, Any] = {
        "type": "library",
        "name": name,
        "bom-ref": purl,
        "purl": purl,
    }
    if version:
        component["version"] = version
    hashes = _sbom_component_hashes(pkg)
    if hashes:
        component["hashes"] = hashes
    properties = [
        {"name": "molt.tier", "value": tier},
        {"name": "molt.tier_reason", "value": reason},
    ]
    if isinstance(source, dict):
        if source.get("git"):
            properties.append({"name": "molt.source", "value": "git"})
            if isinstance(source.get("git"), str):
                properties.append({"name": "molt.source_git", "value": source["git"]})
        elif source.get("path"):
            properties.append({"name": "molt.source", "value": "path"})
    component["properties"] = properties
    return component


def _sbom_dependencies(
    project_root: Path,
) -> tuple[list[dict[str, Any]], list[str], list[str]]:
    warnings: list[str] = []
    lock_path = project_root / "uv.lock"
    if not lock_path.exists():
        warnings.append("uv.lock not found; SBOM excludes Python dependencies.")
        return [], [], warnings
    lock = _load_toml(lock_path)
    pyproject = _load_toml(project_root / "pyproject.toml")
    allow = _dep_allowlists(pyproject)
    components: list[dict[str, Any]] = []
    refs: list[str] = []
    packages = lock.get("package", [])
    if not packages:
        warnings.append("uv.lock contains no package entries.")
        return [], [], warnings
    for pkg in packages:
        if not isinstance(pkg, dict):
            continue
        component = _sbom_component_for_lock_pkg(pkg, allow)
        if component is None:
            continue
        components.append(component)
    components.sort(key=lambda entry: (entry.get("name", ""), entry.get("version", "")))
    for component in components:
        ref = component.get("bom-ref")
        if isinstance(ref, str):
            refs.append(ref)
    return components, refs, warnings


def _build_sbom(
    *,
    manifest: dict[str, Any],
    artifact_path: Path,
    checksum: str,
    project_root: Path,
    format_name: str = "cyclonedx",
) -> tuple[dict[str, Any], list[str]]:
    if format_name == "cyclonedx":
        return _build_cyclonedx_sbom(
            manifest=manifest,
            artifact_path=artifact_path,
            checksum=checksum,
            project_root=project_root,
        )
    if format_name == "spdx":
        return _build_spdx_sbom(
            manifest=manifest,
            artifact_path=artifact_path,
            checksum=checksum,
            project_root=project_root,
        )
    raise ValueError(f"Unsupported SBOM format: {format_name}")


def _build_cyclonedx_sbom(
    *,
    manifest: dict[str, Any],
    artifact_path: Path,
    checksum: str,
    project_root: Path,
) -> tuple[dict[str, Any], list[str]]:
    warnings: list[str] = []
    compiler_version, compiler_rev = _compiler_metadata()
    rustc_version = _rustc_version()
    if rustc_version:
        rustc_version = rustc_version.splitlines()[0].strip() or rustc_version
    name = manifest.get("name", "molt_pkg")
    version = manifest.get("version", "0.0.0")
    target = manifest.get("target", "unknown")
    abi_version = manifest.get("abi_version")
    deterministic = manifest.get("deterministic")
    effects = manifest.get("effects")
    capabilities = manifest.get("capabilities")
    component_ref = f"pkg:molt/{_normalize_name(str(name))}@{version}"
    component = {
        "type": "library",
        "name": name,
        "version": version,
        "bom-ref": component_ref,
        "purl": component_ref,
        "hashes": [{"alg": "SHA-256", "content": checksum}],
        "properties": [
            {"name": "molt.target", "value": str(target)},
            {"name": "molt.abi_version", "value": str(abi_version)},
            {"name": "molt.deterministic", "value": str(deterministic)},
        ],
    }
    properties = cast(list[dict[str, str]], component["properties"])
    if effects is not None:
        properties.append({"name": "molt.effects", "value": json.dumps(effects)})
    if capabilities is not None:
        properties.append(
            {"name": "molt.capabilities", "value": json.dumps(capabilities)}
        )
    properties.append({"name": "molt.artifact", "value": str(artifact_path)})
    meta_properties: list[dict[str, str]] = []
    if compiler_version:
        meta_properties.append(
            {"name": "molt.compiler.version", "value": compiler_version}
        )
    if compiler_rev:
        meta_properties.append({"name": "molt.compiler.git_rev", "value": compiler_rev})
    if rustc_version:
        meta_properties.append({"name": "molt.rustc.version", "value": rustc_version})
    components, dependency_refs, dep_warnings = _sbom_dependencies(project_root)
    warnings.extend(dep_warnings)
    sbom: dict[str, Any] = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "tools": [
                {
                    "vendor": "molt",
                    "name": "molt",
                    "version": compiler_version or "unknown",
                }
            ],
            "component": component,
        },
    }
    if meta_properties:
        sbom["metadata"]["properties"] = meta_properties
    if components:
        sbom["components"] = components
    if dependency_refs:
        sbom["dependencies"] = [{"ref": component_ref, "dependsOn": dependency_refs}]
    return sbom, warnings


def _spdx_id(base: str) -> str:
    cleaned = _OUTPUT_BASE_SAFE_RE.sub("-", base).strip(".-")
    if not cleaned:
        cleaned = "package"
    return f"SPDXRef-{cleaned}"


def _spdx_checksum(value: str | None) -> list[dict[str, str]] | None:
    digest = _normalize_sha256(value)
    if not digest:
        return None
    return [{"algorithm": "SHA256", "checksumValue": digest}]


def _spdx_package_entry(
    *,
    name: str,
    version: str | None,
    checksum: str | None,
    purl: str | None,
    spdx_id: str,
) -> dict[str, Any]:
    package: dict[str, Any] = {
        "SPDXID": spdx_id,
        "name": name,
        "downloadLocation": "NOASSERTION",
        "licenseConcluded": "NOASSERTION",
        "licenseDeclared": "NOASSERTION",
        "filesAnalyzed": False,
    }
    if version:
        package["versionInfo"] = version
    checksums = _spdx_checksum(checksum)
    if checksums:
        package["checksums"] = checksums
    if purl:
        package["externalRefs"] = [
            {
                "referenceCategory": "PACKAGE-MANAGER",
                "referenceType": "purl",
                "referenceLocator": purl,
            }
        ]
    return package


def _build_spdx_sbom(
    *,
    manifest: dict[str, Any],
    artifact_path: Path,
    checksum: str,
    project_root: Path,
) -> tuple[dict[str, Any], list[str]]:
    warnings: list[str] = []
    compiler_version, _compiler_rev = _compiler_metadata()
    name = manifest.get("name", "molt_pkg")
    version = manifest.get("version", "0.0.0")
    target = manifest.get("target", "unknown")
    namespace_seed = f"{name}-{version}-{target}-{checksum}"
    namespace_token = _OUTPUT_BASE_SAFE_RE.sub("-", namespace_seed).strip(".-")
    if not namespace_token:
        namespace_token = "molt"
    document_namespace = f"https://molt.dev/spdx/{namespace_token}"
    created = "1970-01-01T00:00:00Z"
    tool_version = compiler_version or "unknown"
    root_purl = f"pkg:molt/{_normalize_name(str(name))}@{version}"
    root_id = _spdx_id(f"{name}-{version}")

    packages: list[dict[str, Any]] = []
    packages.append(
        _spdx_package_entry(
            name=str(name),
            version=str(version),
            checksum=checksum,
            purl=root_purl,
            spdx_id=root_id,
        )
    )
    components, dependency_refs, dep_warnings = _sbom_dependencies(project_root)
    warnings.extend(dep_warnings)
    relationships: list[dict[str, str]] = []
    if components:
        for component in components:
            dep_name = str(component.get("name") or "dependency")
            dep_version = component.get("version")
            dep_id = _spdx_id(f"{dep_name}-{dep_version or 'unknown'}")
            dep_checksum = None
            hashes = component.get("hashes")
            if isinstance(hashes, list):
                for entry in hashes:
                    if (
                        isinstance(entry, dict)
                        and entry.get("alg") == "SHA-256"
                        and isinstance(entry.get("content"), str)
                    ):
                        dep_checksum = entry.get("content")
                        break
            dep_purl = component.get("purl") if isinstance(component, dict) else None
            packages.append(
                _spdx_package_entry(
                    name=dep_name,
                    version=str(dep_version) if dep_version else None,
                    checksum=dep_checksum,
                    purl=dep_purl if isinstance(dep_purl, str) else None,
                    spdx_id=dep_id,
                )
            )
            relationships.append(
                {
                    "spdxElementId": root_id,
                    "relationshipType": "DEPENDS_ON",
                    "relatedSpdxElement": dep_id,
                }
            )

    sbom: dict[str, Any] = {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": f"molt-{name}-{version}",
        "documentNamespace": document_namespace,
        "creationInfo": {
            "created": created,
            "creators": [f"Tool: molt {tool_version}"],
        },
        "documentDescribes": [root_id],
        "packages": packages,
    }
    if relationships:
        sbom["relationships"] = relationships
    return sbom, warnings


def _is_macho(path: Path) -> bool:
    try:
        data = path.read_bytes()[:4]
    except OSError:
        return False
    if len(data) < 4:
        return False
    be = int.from_bytes(data, "big")
    le = int.from_bytes(data, "little")
    magic_values = {
        0xFEEDFACE,
        0xCEFAEDFE,
        0xFEEDFACF,
        0xCFFAEDFE,
        0xCAFEBABE,
        0xBEBAFECA,
    }
    return be in magic_values or le in magic_values


def _cosign_key_hash(key_path: Path) -> str | None:
    try:
        return _sha256_file(key_path)
    except OSError:
        return None


def _cosign_sign_blob(
    artifact_path: Path,
    key: str,
    *,
    tlog_upload: bool = False,
) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="molt_cosign_") as tmpdir:
        sig_path = Path(tmpdir) / "artifact.sig"
        cert_path = Path(tmpdir) / "artifact.pem"
        cmd = [
            "cosign",
            "sign-blob",
            "--yes",
            "--key",
            key,
            "--output-signature",
            str(sig_path),
            "--output-certificate",
            str(cert_path),
        ]
        if not tlog_upload:
            cmd.append("--tlog-upload=false")
        cmd.append(str(artifact_path))
        result = subprocess.run(cmd, capture_output=True, text=True, check=False)
        if result.returncode != 0:
            detail = (result.stderr or result.stdout).strip() or "unknown error"
            raise RuntimeError(f"cosign sign-blob failed: {detail}")
        signature = sig_path.read_text().strip()
        certificate = cert_path.read_text().strip()
    metadata: dict[str, Any] = {
        "tool": {"name": "cosign"},
        "signature": {"format": "cosign-blob", "value": signature},
    }
    if certificate:
        metadata["signature"]["certificate"] = certificate
    key_path = Path(key).expanduser()
    if key_path.exists():
        key_hash = _cosign_key_hash(key_path)
        if key_hash:
            metadata["key"] = {"sha256": key_hash}
    return metadata


def _codesign_identity_info(artifact_path: Path) -> dict[str, Any]:
    result = subprocess.run(
        ["codesign", "--display", "--verbose=4", str(artifact_path)],
        capture_output=True,
        text=True,
        check=False,
    )
    output = (result.stderr or "") + (result.stdout or "")
    info: dict[str, Any] = {"tool": {"name": "codesign"}}
    authorities: list[str] = []
    for line in output.splitlines():
        if line.startswith("Authority="):
            authorities.append(line.split("=", 1)[1].strip())
        elif line.startswith("TeamIdentifier="):
            info["team_id"] = line.split("=", 1)[1].strip()
        elif line.startswith("Identifier="):
            info["identifier"] = line.split("=", 1)[1].strip()
        elif line.startswith("Format="):
            info["format"] = line.split("=", 1)[1].strip()
    if authorities:
        info["authorities"] = authorities
    return info


def _codesign_sign(artifact_path: Path, identity: str) -> dict[str, Any]:
    cmd = [
        "codesign",
        "--force",
        "--sign",
        identity,
        "--timestamp=none",
        str(artifact_path),
    ]
    result = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip() or "unknown error"
        raise RuntimeError(f"codesign failed: {detail}")
    info = _codesign_identity_info(artifact_path)
    metadata: dict[str, Any] = {"tool": {"name": "codesign"}}
    metadata.update(info)
    return metadata


def _select_signer(preferred: str | None, *, artifact_path: Path | None) -> str | None:
    selected = preferred
    if selected in {"auto", "", None}:
        if (
            sys.platform == "darwin"
            and shutil.which("codesign")
            and (artifact_path is None or _is_macho(artifact_path))
        ):
            return "codesign"
        if shutil.which("cosign"):
            return "cosign"
        if sys.platform == "darwin" and shutil.which("codesign"):
            return "codesign"
        return None
    return selected


def _sign_artifact(
    *,
    artifact_path: Path,
    sign: bool,
    signer: str | None,
    signing_key: str | None,
    signing_identity: str | None,
    tlog_upload: bool,
) -> tuple[dict[str, Any] | None, str | None]:
    if not sign:
        return None, None
    selected = _select_signer(signer, artifact_path=artifact_path)
    if selected is None:
        raise RuntimeError("No signing tool available (cosign/codesign not found)")
    if selected == "cosign":
        key = signing_key or os.environ.get("COSIGN_KEY")
        if not key:
            raise RuntimeError("cosign signing requires --signing-key or COSIGN_KEY")
        cosign_meta = _cosign_sign_blob(artifact_path, key, tlog_upload=tlog_upload)
        return cosign_meta, selected
    if selected == "codesign":
        if sys.platform != "darwin":
            raise RuntimeError("codesign signing is only available on macOS")
        if not _is_macho(artifact_path):
            raise RuntimeError("codesign requires a Mach-O artifact")
        identity = signing_identity or os.environ.get("MOLT_CODESIGN_IDENTITY")
        if not identity:
            raise RuntimeError(
                "codesign signing requires --signing-identity or MOLT_CODESIGN_IDENTITY"
            )
        codesign_meta = _codesign_sign(artifact_path, identity)
        return codesign_meta, selected
    raise RuntimeError(f"Unsupported signer: {selected}")


def _signature_metadata(
    *,
    artifact_path: Path,
    checksum: str,
    signer_meta: dict[str, Any] | None,
    signer: str | None,
    signature_name: str | None,
    signature_checksum: str | None,
) -> dict[str, Any]:
    metadata: dict[str, Any] = {
        "schema_version": 1,
        "artifact": {"path": str(artifact_path), "sha256": checksum},
    }
    signed = signer_meta is not None or signature_name is not None
    metadata["status"] = "signed" if signed else "unsigned"
    if not signed:
        metadata["reason"] = "signing disabled"
    signature_info: dict[str, Any] = {
        "status": "signed" if signature_name or signer_meta is not None else "unsigned",
        "algorithm": "sha256",
    }
    if signature_name:
        signature_info["file"] = signature_name
    if signature_checksum:
        signature_info["checksum"] = signature_checksum
    metadata["signature"] = signature_info
    if signature_name:
        metadata["signature_file"] = {
            "name": signature_name,
            "sha256": signature_checksum,
        }
    if signer_meta is not None:
        metadata["signer"] = signer_meta
        if signer:
            metadata["signer"]["selected"] = signer
    return metadata


@dataclass(frozen=True)
class TrustPolicy:
    cosign_keys: set[str]
    cosign_cert_substrings: list[str]
    codesign_team_ids: set[str]
    codesign_identifiers: set[str]
    codesign_authorities: set[str]


def _normalize_sha256(value: str | None) -> str | None:
    if not value:
        return None
    cleaned = value.strip().lower()
    if cleaned.startswith("sha256:"):
        cleaned = cleaned[len("sha256:") :]
    return cleaned


def _load_trust_policy(path: Path) -> TrustPolicy:
    if not path.exists():
        raise FileNotFoundError(f"Trust policy not found: {path}")
    if path.suffix == ".json":
        data = json.loads(path.read_text())
    else:
        data = tomllib.loads(path.read_text())
    cosign = data.get("cosign", {})
    codesign = data.get("codesign", {})
    cosign_keys: set[str] = set()
    for key in cosign.get("keys", []):
        if not isinstance(key, str):
            continue
        normalized = _normalize_sha256(key)
        if normalized:
            cosign_keys.add(normalized)
    cosign_cert_substrings = [
        value
        for value in cosign.get("certificates", [])
        if isinstance(value, str) and value
    ]
    codesign_team_ids = {
        value
        for value in codesign.get("team_ids", [])
        if isinstance(value, str) and value
    }
    codesign_identifiers = {
        value
        for value in codesign.get("identifiers", [])
        if isinstance(value, str) and value
    }
    codesign_authorities = {
        value
        for value in codesign.get("authorities", [])
        if isinstance(value, str) and value
    }
    return TrustPolicy(
        cosign_keys=cosign_keys,
        cosign_cert_substrings=cosign_cert_substrings,
        codesign_team_ids=codesign_team_ids,
        codesign_identifiers=codesign_identifiers,
        codesign_authorities=codesign_authorities,
    )


def _trust_policy_allows(
    signer: str | None, signer_meta: dict[str, Any] | None, policy: TrustPolicy
) -> tuple[bool, str]:
    if signer is None:
        return False, "missing signer metadata"
    if signer == "cosign":
        if signer_meta is None:
            return False, "missing cosign metadata"
        key_meta = signer_meta.get("key", {}) if isinstance(signer_meta, dict) else {}
        key_hash = _normalize_sha256(
            key_meta.get("sha256") if isinstance(key_meta, dict) else None
        )
        if policy.cosign_keys and key_hash and key_hash in policy.cosign_keys:
            return True, "cosign key trusted"
        if policy.cosign_cert_substrings:
            cert = None
            signature = signer_meta.get("signature")
            if isinstance(signature, dict):
                cert = signature.get("certificate")
            if isinstance(cert, str):
                for token in policy.cosign_cert_substrings:
                    if token in cert:
                        return True, "cosign certificate trusted"
        return False, "cosign signer not in trusted policy"
    if signer == "codesign":
        if signer_meta is None:
            return False, "missing codesign metadata"
        team_id = signer_meta.get("team_id") if isinstance(signer_meta, dict) else None
        if policy.codesign_team_ids and isinstance(team_id, str):
            if team_id in policy.codesign_team_ids:
                return True, "codesign team trusted"
        identifier = (
            signer_meta.get("identifier") if isinstance(signer_meta, dict) else None
        )
        if policy.codesign_identifiers and isinstance(identifier, str):
            if identifier in policy.codesign_identifiers:
                return True, "codesign identifier trusted"
        authorities = (
            signer_meta.get("authorities") if isinstance(signer_meta, dict) else None
        )
        if policy.codesign_authorities and isinstance(authorities, list):
            for authority in authorities:
                if (
                    isinstance(authority, str)
                    and authority in policy.codesign_authorities
                ):
                    return True, "codesign authority trusted"
        return False, "codesign signer not in trusted policy"
    return False, f"unsupported signer {signer}"


def _resolve_signature_tool(
    signer: str | None,
    signer_meta: dict[str, Any] | None,
    artifact_path: Path,
    signature_bytes: bytes | None,
) -> str | None:
    if signer and signer != "auto":
        return signer
    if isinstance(signer_meta, dict):
        selected = signer_meta.get("selected")
        if isinstance(selected, str) and selected:
            return selected
        tool = signer_meta.get("tool")
        if isinstance(tool, dict):
            name = tool.get("name")
            if isinstance(name, str) and name:
                return name
    if _is_macho(artifact_path):
        return "codesign"
    if signature_bytes is not None:
        return "cosign"
    return None


def _verify_cosign_signature(
    artifact_path: Path, signature_bytes: bytes, signing_key: str
) -> None:
    with tempfile.TemporaryDirectory(prefix="molt_cosign_verify_") as tmpdir:
        sig_path = Path(tmpdir) / "artifact.sig"
        sig_path.write_bytes(signature_bytes)
        cmd = [
            "cosign",
            "verify-blob",
            "--key",
            signing_key,
            "--signature",
            str(sig_path),
            "--insecure-ignore-tlog",
            str(artifact_path),
        ]
        result = subprocess.run(cmd, capture_output=True, text=True, check=False)
        if result.returncode != 0:
            detail = (result.stderr or result.stdout).strip() or "unknown error"
            raise RuntimeError(f"cosign verify-blob failed: {detail}")


def _verify_codesign_signature(artifact_path: Path) -> None:
    result = subprocess.run(
        ["codesign", "--verify", "--verbose=4", str(artifact_path)],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip() or "unknown error"
        raise RuntimeError(f"codesign verify failed: {detail}")


def _module_name_from_path(path: Path, roots: list[Path], stdlib_root: Path) -> str:
    resolved = path.resolve()
    resolved_roots = tuple(root.resolve() for root in roots)
    resolved_stdlib_root = stdlib_root.resolve()
    cpython_test_root: Path | None = None
    cpython_dir = os.environ.get("MOLT_REGRTEST_CPYTHON_DIR")
    if cpython_dir:
        cpython_test_root = (Path(cpython_dir) / "Lib" / "test").resolve()
    return _module_name_from_resolved_path(
        resolved,
        resolved_roots=resolved_roots,
        resolved_stdlib_root=resolved_stdlib_root,
        cpython_test_root=cpython_test_root,
    )


def _module_name_from_resolved_path(
    resolved: Path,
    *,
    resolved_roots: tuple[Path, ...],
    resolved_stdlib_root: Path,
    cpython_test_root: Path | None,
) -> str:
    resolved_parts = resolved.parts
    rel_parts = _relative_parts_if_within(resolved_parts, resolved_stdlib_root.parts)
    if rel_parts is not None:
        module_name = _module_name_from_relative_parts(
            rel_parts, fallback_parent=resolved.parent.name
        )
        if module_name is not None:
            return module_name

    best_rel_parts: tuple[str, ...] | None = None
    best_len = -1
    for root_resolved in resolved_roots:
        if cpython_test_root is not None and root_resolved == cpython_test_root:
            continue
        candidate_parts = _relative_parts_if_within(resolved_parts, root_resolved.parts)
        if candidate_parts is None:
            continue
        root_len = len(root_resolved.parts)
        if root_len > best_len:
            best_len = root_len
            best_rel_parts = candidate_parts
    if best_rel_parts is None:
        # Paths outside known module roots should still compile deterministically as
        # top-level modules instead of leaking absolute-path segments into module ids.
        if resolved.name == "__init__.py":
            return resolved.parent.name or "__init__"
        return resolved.stem
    module_name = _module_name_from_relative_parts(
        best_rel_parts, fallback_parent=resolved.parent.name
    )
    if module_name is not None:
        return module_name
    return resolved.parent.name or resolved.stem


@functools.lru_cache(maxsize=4096)
def _expand_module_chain_cached(name: str) -> tuple[str, ...]:
    name = name.strip()
    if not name:
        return ()
    parts = name.split(".")
    if any(not part or not part.isidentifier() for part in parts):
        return ()
    return tuple(".".join(parts[:idx]) for idx in range(1, len(parts) + 1))


def _expand_module_chain(name: str) -> list[str]:
    return list(_expand_module_chain_cached(name))


def _resolve_root_override(var: str) -> Path | None:
    override = os.environ.get(var)
    if not override:
        return None
    path = Path(override).expanduser()
    if not path.is_absolute():
        path = (Path.cwd() / path).absolute()
    if path.exists():
        return path
    return None


def _has_molt_repo_markers(path: Path) -> bool:
    return (path / "runtime/molt-runtime/Cargo.toml").exists() and (
        path / "src/molt/cli.py"
    ).exists()


@functools.lru_cache(maxsize=64)
def _find_project_root_cached(start_text: str, override_text: str | None) -> Path:
    if override_text:
        override = Path(override_text)
        if override.exists():
            return override
    start = Path(start_text)
    for parent in [start] + list(start.parents):
        if _has_project_markers(parent):
            return parent
    return start.parent


def _has_project_markers(path: Path) -> bool:
    return (
        (path / "pyproject.toml").exists()
        or (path / ".git").exists()
        or _has_molt_repo_markers(path)
    )


def _find_project_root(start: Path) -> Path:
    override = _resolve_root_override("MOLT_PROJECT_ROOT")
    override_text = str(override) if override is not None else None
    return _find_project_root_cached(str(start), override_text)


@functools.lru_cache(maxsize=64)
def _find_molt_root_cached(
    candidate_texts: tuple[str, ...],
    override_text: str | None,
) -> Path:
    if override_text:
        override = Path(override_text)
        if override.exists():
            return override
    candidates = tuple(Path(text) for text in candidate_texts)
    for candidate in candidates:
        for parent in [candidate] + list(candidate.parents):
            if _has_molt_repo_markers(parent):
                return parent
    module_path = Path(__file__).resolve()
    for parent in [module_path] + list(module_path.parents):
        if _has_molt_repo_markers(parent):
            return parent
    if candidates:
        return candidates[0]
    return Path.cwd()


def _find_molt_root(*candidates: Path) -> Path:
    override = _resolve_root_override("MOLT_PROJECT_ROOT")
    override_text = str(override) if override is not None else None
    return _find_molt_root_cached(
        tuple(str(candidate) for candidate in candidates),
        override_text,
    )


def _require_molt_root(
    molt_root: Path,
    json_output: bool,
    command: str,
) -> int | None:
    runtime_toml = molt_root / "runtime/molt-runtime/Cargo.toml"
    backend_toml = molt_root / "runtime/molt-backend/Cargo.toml"
    if runtime_toml.exists() and backend_toml.exists():
        return None
    message = (
        f"Molt runtime sources not found under {molt_root}. "
        "Set MOLT_PROJECT_ROOT to the Molt repo root or run from within the Molt repo."
    )
    return _fail(message, json_output, command=command)


def _stdlib_root_path() -> Path:
    override = os.environ.get("MOLT_PROJECT_ROOT")
    if override:
        root = Path(override).expanduser()
        if not root.is_absolute():
            root = (Path.cwd() / root).absolute()
        candidate = root / "src/molt/stdlib"
        if candidate.exists():
            return candidate.resolve()
    candidate = Path(__file__).resolve().parent / "stdlib"
    if candidate.exists():
        return candidate.resolve()
    return Path("src/molt/stdlib").resolve()


def _resolve_module_path(module_name: str, roots: list[Path]) -> Path | None:
    return _resolve_module_path_parts(tuple(module_name.split(".")), roots)


def _resolve_module_path_parts(
    parts: tuple[str, ...], roots: list[Path]
) -> Path | None:
    if not parts:
        return None
    module_filename = f"{parts[-1]}.py"
    for root in roots:
        root_text = os.fspath(root)
        pkg_text = os.path.join(root_text, *parts, "__init__.py")
        if os.path.isfile(pkg_text):
            return Path(pkg_text)
        if len(parts) == 1:
            mod_text = os.path.join(root_text, module_filename)
        else:
            mod_text = os.path.join(root_text, *parts[:-1], module_filename)
        if os.path.isfile(mod_text):
            return Path(mod_text)
    return None


@dataclass
class _ModuleResolutionCache:
    roots_cache: dict[str, list[Path]] = field(default_factory=dict)
    resolve_cache: dict[str, Path | None] = field(default_factory=dict)
    namespace_dir_cache: dict[str, bool] = field(default_factory=dict)
    resolved_path_cache: dict[Path, Path] = field(default_factory=dict)
    resolved_roots_cache: dict[tuple[Path, ...], tuple[Path, ...]] = field(
        default_factory=dict
    )
    source_cache: dict[Path, str] = field(default_factory=dict)
    source_error_cache: dict[Path, Exception] = field(default_factory=dict)
    ast_cache: dict[tuple[Path, str], ast.AST] = field(default_factory=dict)
    ast_error_cache: dict[tuple[Path, str], SyntaxError] = field(default_factory=dict)
    module_name_cache: dict[tuple[Path, tuple[Path, ...], Path, Path | None], str] = (
        field(default_factory=dict)
    )
    module_name_context_key: tuple[tuple[Path, ...], Path, Path | None] | None = None
    module_name_context_cache: dict[Path, str] = field(default_factory=dict)
    stdlib_path_cache: dict[tuple[Path, Path], bool] = field(default_factory=dict)
    import_scan_cache: dict[tuple[Path, str | None, bool, bool], tuple[str, ...]] = (
        field(default_factory=dict)
    )
    path_stat_cache: dict[Path, os.stat_result] = field(default_factory=dict)
    path_stat_error_cache: dict[Path, OSError] = field(default_factory=dict)
    module_parts_cache: dict[str, tuple[str, ...]] = field(default_factory=dict)
    cpython_test_root_cache: Path | None = None
    cpython_test_root_cache_populated: bool = False

    def roots_for_module(
        self,
        module_name: str,
        roots: list[Path],
        stdlib_root: Path,
        stdlib_allowlist: set[str],
    ) -> list[Path]:
        candidate_roots = self.roots_cache.get(module_name)
        if candidate_roots is None:
            candidate_roots = _roots_for_module(
                module_name, roots, stdlib_root, stdlib_allowlist
            )
            self.roots_cache[module_name] = candidate_roots
        return candidate_roots

    def module_parts(self, module_name: str) -> tuple[str, ...]:
        cached = self.module_parts_cache.get(module_name)
        if cached is None:
            cached = tuple(module_name.split("."))
            self.module_parts_cache[module_name] = cached
        return cached

    def resolve_module(
        self,
        module_name: str,
        roots: list[Path],
        stdlib_root: Path,
        stdlib_allowlist: set[str],
    ) -> Path | None:
        cache_key = module_name
        if module_name.startswith("molt.stdlib."):
            cache_key = f"stdlib:{module_name}"
        if cache_key not in self.resolve_cache:
            if cache_key.startswith("stdlib:"):
                stdlib_candidate = module_name[len("molt.stdlib.") :]
                self.resolve_cache[cache_key] = _resolve_module_path_parts(
                    self.module_parts(stdlib_candidate), [stdlib_root]
                )
            else:
                candidate_roots = self.roots_for_module(
                    module_name, roots, stdlib_root, stdlib_allowlist
                )
                self.resolve_cache[cache_key] = _resolve_module_path_parts(
                    self.module_parts(module_name), candidate_roots
                )
        return self.resolve_cache[cache_key]

    def has_namespace_dir(
        self,
        module_name: str,
        roots: list[Path],
        stdlib_root: Path,
        stdlib_allowlist: set[str],
    ) -> bool:
        has_namespace_dir = self.namespace_dir_cache.get(module_name)
        if has_namespace_dir is None:
            candidate_roots = self.roots_for_module(
                module_name, roots, stdlib_root, stdlib_allowlist
            )
            has_namespace_dir = _has_namespace_dir(module_name, candidate_roots)
            self.namespace_dir_cache[module_name] = has_namespace_dir
        return has_namespace_dir

    def resolved_path(self, path: Path) -> Path:
        resolved = self.resolved_path_cache.get(path)
        if resolved is None:
            if path.is_absolute() and "." not in path.parts and ".." not in path.parts:
                resolved = path
            else:
                resolved = path.resolve()
            self.resolved_path_cache[path] = resolved
        return resolved

    def resolved_roots(self, roots: list[Path]) -> tuple[Path, ...]:
        roots_key = tuple(roots)
        resolved = self.resolved_roots_cache.get(roots_key)
        if resolved is None:
            resolved = tuple(self.resolved_path(root) for root in roots)
            self.resolved_roots_cache[roots_key] = resolved
        return resolved

    def module_name_from_path(
        self, path: Path, roots: list[Path], stdlib_root: Path
    ) -> str:
        resolved = self.resolved_path(path)
        resolved_roots = self.resolved_roots(roots)
        resolved_stdlib_root = self.resolved_path(stdlib_root)
        cpython_test_root = self.cpython_test_root()
        context_key = (resolved_roots, resolved_stdlib_root, cpython_test_root)
        if self.module_name_context_key == context_key:
            cached = self.module_name_context_cache.get(resolved)
            if cached is not None:
                return cached
        else:
            self.module_name_context_key = context_key
            self.module_name_context_cache.clear()
        cache_key = (
            resolved,
            resolved_roots,
            resolved_stdlib_root,
            cpython_test_root,
        )
        cached = self.module_name_cache.get(cache_key)
        if cached is not None:
            self.module_name_context_cache[resolved] = cached
            return cached
        module_name = _module_name_from_resolved_path(
            resolved,
            resolved_roots=resolved_roots,
            resolved_stdlib_root=resolved_stdlib_root,
            cpython_test_root=cpython_test_root,
        )
        self.module_name_cache[cache_key] = module_name
        self.module_name_context_cache[resolved] = module_name
        return module_name

    def cpython_test_root(self) -> Path | None:
        if not self.cpython_test_root_cache_populated:
            cpython_dir = os.environ.get("MOLT_REGRTEST_CPYTHON_DIR")
            if cpython_dir:
                self.cpython_test_root_cache = self.resolved_path(
                    Path(cpython_dir) / "Lib" / "test"
                )
            self.cpython_test_root_cache_populated = True
        return self.cpython_test_root_cache

    def is_stdlib_path(self, path: Path, stdlib_root: Path) -> bool:
        resolved_path = self.resolved_path(path)
        resolved_stdlib_root = self.resolved_path(stdlib_root)
        cache_key = (resolved_path, resolved_stdlib_root)
        cached = self.stdlib_path_cache.get(cache_key)
        if cached is None:
            cached = _is_stdlib_resolved_path(resolved_path, resolved_stdlib_root)
            self.stdlib_path_cache[cache_key] = cached
        return cached

    def read_module_source(self, path: Path) -> str:
        cache_key = self.resolved_path(path)
        source = self.source_cache.get(cache_key)
        if source is not None:
            return source
        cached_error = self.source_error_cache.get(cache_key)
        if cached_error is not None:
            raise cached_error
        try:
            source = _read_module_source(path)
        except (OSError, SyntaxError, UnicodeDecodeError) as exc:
            self.source_error_cache[cache_key] = exc
            raise
        self.source_cache[cache_key] = source
        return source

    def path_stat(self, path: Path) -> os.stat_result:
        cache_key = self.resolved_path(path)
        cached = self.path_stat_cache.get(cache_key)
        if cached is not None:
            return cached
        cached_error = self.path_stat_error_cache.get(cache_key)
        if cached_error is not None:
            raise cached_error
        try:
            stat_result = path.stat()
        except OSError as exc:
            self.path_stat_error_cache[cache_key] = exc
            raise
        self.path_stat_cache[cache_key] = stat_result
        return stat_result

    def parse_module_ast(self, path: Path, source: str, *, filename: str) -> ast.AST:
        cache_key = (self.resolved_path(path), filename)
        tree = self.ast_cache.get(cache_key)
        if tree is not None:
            return tree
        cached_error = self.ast_error_cache.get(cache_key)
        if cached_error is not None:
            raise cached_error
        try:
            tree = ast.parse(source, filename=filename)
        except SyntaxError as exc:
            self.ast_error_cache[cache_key] = exc
            raise
        self.ast_cache[cache_key] = tree
        return tree

    def collect_imports(
        self,
        path: Path,
        tree: ast.AST,
        *,
        module_name: str | None = None,
        is_package: bool = False,
        include_nested: bool = True,
    ) -> tuple[str, ...]:
        cache_key = (
            self.resolved_path(path),
            module_name,
            is_package,
            include_nested,
        )
        cached = self.import_scan_cache.get(cache_key)
        if cached is not None:
            return cached
        imports = _collect_imports(
            tree,
            module_name,
            is_package,
            include_nested=include_nested,
        )
        cached_imports = tuple(imports)
        self.import_scan_cache[cache_key] = cached_imports
        return cached_imports


def _resolve_entry_module(
    module_name: str, roots: list[Path]
) -> tuple[str, Path] | None:
    stripped = module_name.strip()
    if not stripped:
        return None
    main_name = f"{stripped}.__main__"
    main_path = _resolve_module_path(main_name, roots)
    if main_path is not None:
        return main_name, main_path
    mod_path = _resolve_module_path(stripped, roots)
    if mod_path is not None:
        return stripped, mod_path
    return None


def _output_base_for_entry(entry_module: str, source_path: Path) -> str:
    base = entry_module.rsplit(".", 1)[-1] or source_path.stem
    if base == "__main__" and "." in entry_module:
        base = entry_module.rsplit(".", 2)[-2]
    return base


def _resolve_module_roots(
    project_root: Path,
    cwd_root: Path,
    *,
    respect_pythonpath: bool,
) -> list[Path]:
    module_roots: list[Path] = []
    extra_roots = os.environ.get("MOLT_MODULE_ROOTS", "")
    if extra_roots:
        for entry in extra_roots.split(os.pathsep):
            if not entry:
                continue
            entry_path = Path(entry).expanduser()
            if entry_path.exists():
                module_roots.append(entry_path)
    for root in (project_root, cwd_root):
        if root.exists():
            module_roots.append(root)
        src_root = root / "src"
        if src_root.exists():
            module_roots.append(src_root)
        module_roots.extend(_vendor_roots(root))
    if respect_pythonpath:
        pythonpath = os.environ.get("PYTHONPATH", "")
        if pythonpath:
            for entry in pythonpath.split(os.pathsep):
                if not entry:
                    continue
                entry_path = Path(entry).expanduser()
                if entry_path.exists():
                    module_roots.append(entry_path)
    return list(dict.fromkeys(root.resolve() for root in module_roots))


def _build_args_respect_pythonpath(args: list[str]) -> bool:
    if any(arg == "--no-respect-pythonpath" for arg in args):
        return False
    return any(arg == "--respect-pythonpath" for arg in args)


def _has_namespace_dir(module_name: str, roots: list[Path]) -> bool:
    rel = Path(*module_name.split("."))
    for root in roots:
        candidate = root / rel
        if candidate.exists() and candidate.is_dir():
            return True
    return False


def _collect_namespace_parents(
    module_graph: dict[str, Path],
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    explicit_imports: set[str] | None = None,
    *,
    resolver_cache: _ModuleResolutionCache | None = None,
) -> set[str]:
    namespace_parents: set[str] = set()
    resolution_cache = resolver_cache or _ModuleResolutionCache()

    def maybe_add(name: str) -> None:
        if name in module_graph:
            return
        if (
            resolution_cache.resolve_module(name, roots, stdlib_root, stdlib_allowlist)
            is not None
        ):
            return
        if resolution_cache.has_namespace_dir(
            name, roots, stdlib_root, stdlib_allowlist
        ):
            namespace_parents.add(name)

    for module_name in module_graph:
        parts = module_name.split(".")
        for idx in range(1, len(parts)):
            maybe_add(".".join(parts[:idx]))

    if explicit_imports:
        for name in explicit_imports:
            for candidate in _expand_module_chain_cached(name):
                maybe_add(candidate)
    return namespace_parents


def _namespace_paths(name: str, roots: list[Path]) -> list[str]:
    rel = Path(*name.split("."))
    paths: list[str] = []
    for root in roots:
        candidate = root / rel
        if candidate.exists() and candidate.is_dir():
            paths.append(str(candidate))
    return list(dict.fromkeys(paths))


def _spec_parent(spec_name: str, is_package: bool) -> str:
    if is_package:
        return spec_name
    return spec_name.rpartition(".")[0]


def _is_modulespec_ctor(node: ast.AST) -> bool:
    if isinstance(node, ast.Name):
        return node.id == "ModuleSpec"
    if isinstance(node, ast.Attribute):
        return node.attr == "ModuleSpec"
    return False


def _parse_modulespec_override(
    value: ast.AST,
) -> tuple[str, bool | None] | None:
    if not isinstance(value, ast.Call):
        return None
    if not _is_modulespec_ctor(value.func):
        return None
    spec_name = None
    if value.args:
        first = value.args[0]
        if isinstance(first, ast.Constant) and isinstance(first.value, str):
            spec_name = first.value
    for kw in value.keywords:
        if (
            kw.arg == "name"
            and spec_name is None
            and isinstance(kw.value, ast.Constant)
            and isinstance(kw.value.value, str)
        ):
            spec_name = kw.value.value
    if spec_name is None:
        return None
    is_package = None
    if len(value.args) >= 4:
        arg = value.args[3]
        if isinstance(arg, ast.Constant) and isinstance(arg.value, bool):
            is_package = arg.value
    for kw in value.keywords:
        if (
            kw.arg == "is_package"
            and isinstance(kw.value, ast.Constant)
            and isinstance(kw.value.value, bool)
        ):
            is_package = kw.value.value
    return spec_name, is_package


def _infer_module_overrides(
    tree: ast.AST,
) -> tuple[bool, str | None, bool, str | None, bool | None]:
    package_override_set = False
    package_override: str | None = None
    spec_override_set = False
    spec_override: str | None = None
    spec_override_is_package: bool | None = None
    for stmt in getattr(tree, "body", []):
        if isinstance(stmt, ast.Assign):
            targets = stmt.targets
            value = stmt.value
        elif isinstance(stmt, ast.AnnAssign) and stmt.value is not None:
            targets = [stmt.target]
            value = stmt.value
        else:
            continue
        for target in targets:
            if not isinstance(target, ast.Name):
                continue
            if target.id == "__package__":
                package_override_set = True
                if isinstance(value, ast.Constant) and isinstance(value.value, str):
                    package_override = value.value
                elif isinstance(value, ast.Constant) and value.value is None:
                    package_override = None
                else:
                    package_override = None
            elif target.id == "__spec__":
                if isinstance(value, ast.Constant) and value.value is None:
                    spec_override_set = False
                    spec_override = None
                    spec_override_is_package = None
                else:
                    parsed = _parse_modulespec_override(value)
                    if parsed is None:
                        continue
                    spec_override_set = True
                    spec_override, spec_override_is_package = parsed
    return (
        package_override_set,
        package_override,
        spec_override_set,
        spec_override,
        spec_override_is_package,
    )


def _package_root_for_override(source_path: Path, package_name: str) -> Path | None:
    parts = [part for part in package_name.split(".") if part]
    if not parts:
        return None
    package_dir = source_path.parent
    if len(parts) > len(package_dir.parts):
        return None
    if tuple(package_dir.parts[-len(parts) :]) != tuple(parts):
        return None
    root = package_dir
    for _ in parts:
        root = root.parent
    return root


def _write_namespace_module(name: str, paths: list[str], output_dir: Path) -> Path:
    safe = re.sub(r"[^0-9A-Za-z_]+", "_", name.replace(".", "_")).strip("_")
    if not safe:
        safe = "root"
    stub_path = output_dir / f"namespace_{safe}.py"
    lines = [
        '"""Auto-generated namespace package stub for Molt."""',
        "",
        f"__package__ = {name!r}",
        f"__path__ = {paths!r}",
        "try:",
        "    spec = __spec__",
        "except NameError:",
        "    spec = None",
        "if spec is not None:",
        "    try:",
        "        spec.submodule_search_locations = list(__path__)",
        "    except Exception:",
        "        pass",
        "",
    ]
    stub_path.parent.mkdir(parents=True, exist_ok=True)
    _write_text_if_changed(stub_path, "\n".join(lines))
    return stub_path


def _logical_generated_module_path(module_name: str) -> str:
    safe = re.sub(r"[^0-9A-Za-z_]+", "_", module_name).strip("_")
    if not safe:
        safe = "module"
    return f"/__molt_generated__/{safe}.py"


def _collect_package_parents(
    module_graph: dict[str, Path],
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    *,
    resolver_cache: _ModuleResolutionCache | None = None,
) -> None:
    resolution_cache = resolver_cache or _ModuleResolutionCache()
    pending: set[str] = set()
    for module_name in list(module_graph):
        parts = module_name.split(".")
        for idx in range(1, len(parts)):
            pending.add(".".join(parts[:idx]))

    while pending:
        parent = pending.pop()
        if parent in module_graph:
            continue
        resolved = resolution_cache.resolve_module(
            parent, roots, stdlib_root, stdlib_allowlist
        )
        if resolved is None or resolved.name != "__init__.py":
            continue
        module_graph[parent] = resolved
        parent_parts = parent.split(".")
        for idx in range(1, len(parent_parts)):
            ancestor = ".".join(parent_parts[:idx])
            if ancestor not in module_graph:
                pending.add(ancestor)


def _resolve_relative_import(
    module_name: str,
    *,
    is_package: bool,
    level: int,
    module: str | None,
    package_override: str | None = None,
    package_override_set: bool = False,
    spec_override: str | None = None,
    spec_override_set: bool = False,
    spec_override_is_package: bool | None = None,
) -> str | None:
    if level <= 0:
        return module
    package = ""
    if package_override_set:
        package = package_override or ""
    else:
        if spec_override_set and spec_override:
            override_is_package = (
                spec_override_is_package
                if spec_override_is_package is not None
                else is_package
            )
            package = _spec_parent(spec_override, override_is_package)
        else:
            if is_package:
                package = module_name
            elif "." in module_name:
                package = module_name.rsplit(".", 1)[0]
            else:
                package = ""
    if not package:
        return None
    parts = package.split(".")
    if level > len(parts):
        return None
    base_parts = parts[: len(parts) - (level - 1)]
    base_name = ".".join(base_parts)
    if module:
        if base_name:
            return f"{base_name}.{module}"
        return module
    return base_name or None


def _collect_imports(
    tree: ast.AST,
    module_name: str | None = None,
    is_package: bool = False,
    *,
    include_nested: bool = True,
) -> list[str]:
    imports: list[str] = []
    needs_typing = False
    type_alias_cls = getattr(ast, "TypeAlias", None)
    module_string_constants: dict[str, str] = {}
    helper_string_functions: dict[str, tuple[list[str], ast.expr]] = {}
    helper_param_import_positions: dict[str, set[int]] = {}
    helper_import_arg_exprs: dict[str, tuple[list[str], list[ast.expr]]] = {}
    (
        package_override_set,
        package_override,
        spec_override_set,
        spec_override,
        spec_override_is_package,
    ) = _infer_module_overrides(tree)
    module_body = list(getattr(tree, "body", []))
    function_walks: list[
        tuple[ast.FunctionDef | ast.AsyncFunctionDef, tuple[ast.AST, ...]]
    ] = []

    def _importlib_target(func: ast.expr) -> str | None:
        if isinstance(func, ast.Attribute):
            parts: list[str] = []
            current: ast.expr | None = func
            while isinstance(current, ast.Attribute):
                parts.append(current.attr)
                current = current.value
            if isinstance(current, ast.Name):
                parts.append(current.id)
                return ".".join(reversed(parts))
        return None

    def _resolve_string_sequence(
        node: ast.expr, bindings: dict[str, object], seen: set[str]
    ) -> list[str] | None:
        if isinstance(node, (ast.Tuple, ast.List)):
            out: list[str] = []
            for element in node.elts:
                value = _resolve_string_constant(element, bindings, seen)
                if value is None:
                    return None
                out.append(value)
            return out
        if isinstance(node, ast.Name):
            bound = bindings.get(node.id)
            if isinstance(bound, list) and all(isinstance(item, str) for item in bound):
                return list(cast(list[str], bound))
        return None

    def _resolve_string_constant(
        node: ast.expr,
        bindings: dict[str, object] | None = None,
        seen: set[str] | None = None,
    ) -> str | None:
        bindings = bindings or {}
        seen = seen or set()
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            return node.value
        if isinstance(node, ast.Name):
            bound = bindings.get(node.id)
            if isinstance(bound, str):
                return bound
            return module_string_constants.get(node.id)
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            left = _resolve_string_constant(node.left, bindings, seen)
            right = _resolve_string_constant(node.right, bindings, seen)
            if left is not None and right is not None:
                return left + right
            return None
        if isinstance(node, ast.Call):
            if (
                isinstance(node.func, ast.Attribute)
                and node.func.attr == "join"
                and len(node.args) == 1
            ):
                sep = _resolve_string_constant(node.func.value, bindings, seen)
                if sep is None:
                    return None
                items = _resolve_string_sequence(node.args[0], bindings, seen)
                if items is None:
                    return None
                return sep.join(items)
            if isinstance(node.func, ast.Name):
                func_name = node.func.id
                if func_name in seen:
                    return None
                helper = helper_string_functions.get(func_name)
                if helper is None:
                    return None
                params, expr = helper
                if len(node.args) != len(params) or node.keywords:
                    return None
                child_bindings: dict[str, object] = dict(bindings)
                for param, arg in zip(params, node.args):
                    scalar = _resolve_string_constant(arg, bindings, seen)
                    if scalar is not None:
                        child_bindings[param] = scalar
                        continue
                    seq = _resolve_string_sequence(arg, bindings, seen)
                    if seq is not None:
                        child_bindings[param] = seq
                        continue
                    return None
                return _resolve_string_constant(
                    expr, child_bindings, seen | {func_name}
                )
        return None

    if include_nested and isinstance(tree, ast.Module):
        for stmt in module_body:
            if isinstance(stmt, ast.Assign) and len(stmt.targets) == 1:
                target = stmt.targets[0]
                if isinstance(target, ast.Name):
                    value = _resolve_string_constant(stmt.value)
                    if value is not None:
                        module_string_constants[target.id] = value
            elif isinstance(stmt, ast.AnnAssign) and isinstance(stmt.target, ast.Name):
                value = _resolve_string_constant(stmt.value) if stmt.value else None
                if value is not None:
                    module_string_constants[stmt.target.id] = value
            elif isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
                stmt_nodes = tuple(ast.walk(stmt))
                function_walks.append((stmt, stmt_nodes))
                if len(stmt.body) != 1 or not isinstance(stmt.body[0], ast.Return):
                    continue
                ret_expr = stmt.body[0].value
                if ret_expr is None:
                    continue
                params = [
                    arg.arg
                    for arg in (
                        list(stmt.args.posonlyargs)
                        + list(stmt.args.args)
                        + list(stmt.args.kwonlyargs)
                    )
                ]
                if stmt.args.vararg is not None or stmt.args.kwarg is not None:
                    continue
                helper_string_functions[stmt.name] = (params, ret_expr)

        for stmt, stmt_nodes in function_walks:
            params = [
                arg.arg
                for arg in (
                    list(stmt.args.posonlyargs)
                    + list(stmt.args.args)
                    + list(stmt.args.kwonlyargs)
                )
            ]
            if stmt.args.vararg is not None:
                params.append(stmt.args.vararg.arg)
            if stmt.args.kwarg is not None:
                params.append(stmt.args.kwarg.arg)
            if not params:
                continue
            param_set = set(params)
            param_positions = {name: idx for idx, name in enumerate(params)}
            for node in stmt_nodes:
                if not isinstance(node, ast.Call) or not node.args:
                    continue
                target = _importlib_target(node.func)
                if target not in {
                    "importlib.import_module",
                    "importlib.util.find_spec",
                }:
                    continue
                first = node.args[0]
                helper_entry = helper_import_arg_exprs.get(stmt.name)
                if helper_entry is None:
                    helper_import_arg_exprs[stmt.name] = (params, [first])
                else:
                    helper_entry[1].append(first)
                if isinstance(first, ast.Name) and first.id in param_set:
                    pos = param_positions[first.id]
                    helper_param_import_positions.setdefault(stmt.name, set()).add(pos)

        for node in ast.walk(tree):
            if not isinstance(node, ast.Call):
                continue
            if not isinstance(node.func, ast.Name):
                continue
            positions = helper_param_import_positions.get(node.func.id)
            if not positions:
                positions = set()
            for pos in positions:
                if pos < len(node.args):
                    resolved = _resolve_string_constant(node.args[pos])
                    if resolved is not None:
                        imports.append(resolved)
            helper_expr_entry = helper_import_arg_exprs.get(node.func.id)
            if helper_expr_entry is None:
                continue
            params, exprs = helper_expr_entry
            if len(node.args) < len(params):
                continue
            call_bindings: dict[str, object] = {}
            for idx, param in enumerate(params):
                arg = node.args[idx]
                scalar = _resolve_string_constant(arg)
                if scalar is not None:
                    call_bindings[param] = scalar
                    continue
                seq = _resolve_string_sequence(arg, {}, set())
                if seq is not None:
                    call_bindings[param] = seq
            for expr in exprs:
                resolved = _resolve_string_constant(expr, call_bindings, set())
                if resolved is not None:
                    imports.append(resolved)

    if include_nested and isinstance(tree, ast.Module):
        scan_nodes = ast.walk(tree)
    elif include_nested:
        scan_nodes = tuple(ast.walk(tree))
    elif isinstance(tree, ast.Module):
        scan_nodes = module_body
    else:
        scan_nodes = tuple(ast.walk(tree))

    for node in scan_nodes:
        if isinstance(node, ast.Import):
            for alias in node.names:
                imports.append(alias.name)
            continue
        if isinstance(node, ast.ImportFrom):
            if node.level:
                if module_name:
                    resolved = _resolve_relative_import(
                        module_name,
                        is_package=is_package,
                        level=node.level,
                        module=node.module,
                        package_override=package_override,
                        package_override_set=package_override_set,
                        spec_override=spec_override,
                        spec_override_set=spec_override_set,
                        spec_override_is_package=spec_override_is_package,
                    )
                    if resolved:
                        imports.append(resolved)
                        for alias in node.names:
                            if alias.name != "*":
                                imports.append(f"{resolved}.{alias.name}")
                continue
            if node.module:
                imports.append(node.module)
                for alias in node.names:
                    if alias.name != "*":
                        imports.append(f"{node.module}.{alias.name}")
            continue
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            if getattr(node, "type_params", None):
                needs_typing = True
            continue
        if type_alias_cls is not None and isinstance(node, type_alias_cls):
            needs_typing = True
            continue
        if isinstance(node, ast.Call) and node.args:
            target = _importlib_target(node.func)
            if target in {"importlib.import_module", "importlib.util.find_spec"}:
                resolved = _resolve_string_constant(node.args[0])
                if resolved is not None:
                    imports.append(resolved)
            continue
    if needs_typing:
        imports.append("typing")
    return imports


def _is_stdlib_path(path: Path, stdlib_root: Path) -> bool:
    resolved = path.resolve()
    resolved_stdlib_root = stdlib_root.resolve()
    return _is_stdlib_resolved_path(resolved, resolved_stdlib_root)


def _is_stdlib_resolved_path(resolved: Path, resolved_stdlib_root: Path) -> bool:
    return (
        _relative_parts_if_within(resolved.parts, resolved_stdlib_root.parts)
        is not None
    )


def _relative_parts_if_within(
    candidate_parts: tuple[str, ...], root_parts: tuple[str, ...]
) -> tuple[str, ...] | None:
    if len(candidate_parts) < len(root_parts):
        return None
    if candidate_parts[: len(root_parts)] != root_parts:
        return None
    return candidate_parts[len(root_parts) :]


def _module_name_from_relative_parts(
    rel_parts: tuple[str, ...], *, fallback_parent: str
) -> str | None:
    if not rel_parts:
        return None
    if rel_parts[-1] == "__init__.py":
        package_parts = rel_parts[:-1]
        if package_parts:
            return ".".join(package_parts)
        return fallback_parent or None
    last = rel_parts[-1]
    if last.endswith(".py"):
        rel_parts = (*rel_parts[:-1], last[:-3])
    filtered = tuple(part for part in rel_parts if part)
    if not filtered:
        return fallback_parent or None
    return ".".join(filtered)


def _module_dependencies_from_imports(
    module_name: str,
    module_graph: dict[str, Path],
    imports: Iterable[str],
) -> set[str]:
    deps: set[str] = set()
    for name in imports:
        for candidate in _expand_module_chain_cached(name):
            if candidate == "molt" and module_name.startswith("molt."):
                continue
            if candidate in module_graph and candidate != module_name:
                deps.add(candidate)
            if candidate.startswith("molt.stdlib."):
                stdlib_candidate = candidate[len("molt.stdlib.") :]
                if stdlib_candidate in module_graph and stdlib_candidate != module_name:
                    deps.add(stdlib_candidate)
    return deps


def _module_dependencies(
    tree: ast.AST,
    module_name: str,
    module_graph: dict[str, Path],
    *,
    imports: list[str] | None = None,
) -> set[str]:
    path = module_graph.get(module_name)
    is_package = path is not None and path.name == "__init__.py"
    collected_imports = (
        imports
        if imports is not None
        else _collect_imports(tree, module_name, is_package)
    )
    return _module_dependencies_from_imports(
        module_name,
        module_graph,
        collected_imports,
    )


@dataclass(frozen=True)
class ModuleSyntaxErrorInfo:
    message: str
    filename: str
    lineno: int | None
    offset: int | None
    text: str | None


def _read_module_source(path: Path) -> str:
    with path.open("rb") as handle:
        first_line = handle.readline()
        second_line = handle.readline()
        has_utf8_bom = first_line.startswith(codecs.BOM_UTF8)
        has_encoding_cookie = any(
            tokenize.cookie_re.match(line.decode("latin-1", errors="ignore"))
            for line in (first_line, second_line)
            if line
        )
        if not has_utf8_bom and not has_encoding_cookie:
            return (first_line + second_line + handle.read()).decode("utf-8")
    with tokenize.open(path) as handle:
        return handle.read()


def _syntax_error_info_from_exception(
    exc: Exception, *, path: Path
) -> ModuleSyntaxErrorInfo:
    if isinstance(exc, SyntaxError):
        message = exc.msg or str(exc)
        lineno = exc.lineno
        offset = exc.offset
        text = exc.text
        filename = exc.filename or str(path)
    elif isinstance(exc, UnicodeDecodeError):
        message = str(exc)
        lineno = 1
        offset = exc.start + 1 if exc.start is not None else None
        text = None
        filename = str(path)
    else:
        message = str(exc)
        lineno = None
        offset = None
        text = None
        filename = str(path)
    if isinstance(text, str):
        text = text.rstrip("\n")
    return ModuleSyntaxErrorInfo(
        message=message,
        filename=filename,
        lineno=lineno,
        offset=offset,
        text=text,
    )


def _format_syntax_error_message(info: ModuleSyntaxErrorInfo) -> str:
    if info.lineno is None:
        return info.message
    filename = Path(info.filename).name if info.filename else "<unknown>"
    return f"{info.message} ({filename}, line {info.lineno})"


def _syntax_error_stub_ast(info: ModuleSyntaxErrorInfo) -> ast.Module:
    msg = _format_syntax_error_message(info)
    err_name = ast.Name(id="err", ctx=ast.Store())
    err_value = ast.Name(id="err", ctx=ast.Load())
    stmts: list[ast.stmt] = [
        ast.Assign(
            targets=[err_name],
            value=ast.Call(
                func=ast.Name(id="SyntaxError", ctx=ast.Load()),
                args=[ast.Constant(msg)],
                keywords=[],
            ),
        )
    ]
    attr_values = [
        ("lineno", info.lineno),
        ("offset", info.offset),
        ("filename", Path(info.filename).name if info.filename else None),
        ("text", info.text),
    ]
    for attr_name, value in attr_values:
        if value is None:
            continue
        stmts.append(
            ast.Assign(
                targets=[
                    ast.Attribute(
                        value=err_value,
                        attr=attr_name,
                        ctx=ast.Store(),
                    )
                ],
                value=ast.Constant(value),
            )
        )
    stmts.append(ast.Raise(exc=err_value, cause=None))
    module = ast.Module(body=stmts, type_ignores=[])
    return ast.fix_missing_locations(module)


def _default_spec_for_expr(expr: ast.expr) -> dict[str, Any]:
    if isinstance(expr, ast.Constant):
        return {"const": True, "value": expr.value}
    return {"const": False}


def _default_specs_from_args(args: ast.arguments) -> list[dict[str, Any]]:
    default_specs = [_default_spec_for_expr(expr) for expr in args.defaults]
    if not args.kwonlyargs or not args.kw_defaults:
        return default_specs
    kwonly_names = [arg.arg for arg in args.kwonlyargs]
    kwonly_pairs = list(zip(kwonly_names, args.kw_defaults))
    suffix: list[tuple[str, ast.expr]] = []
    for name, expr in reversed(kwonly_pairs):
        if expr is None:
            break
        suffix.append((name, expr))
    for name, expr in reversed(suffix):
        spec = _default_spec_for_expr(expr)
        spec["kwonly"] = True
        spec["name"] = name
        default_specs.append(spec)
    return default_specs


def _collect_func_defaults(tree: ast.AST) -> dict[str, dict[str, Any]]:
    defaults: dict[str, dict[str, Any]] = {}
    if not isinstance(tree, ast.Module):
        return defaults
    for stmt in tree.body:
        if not isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        if stmt.args.vararg or stmt.args.kwarg:
            continue
        params = [
            arg.arg
            for arg in (stmt.args.posonlyargs + stmt.args.args + stmt.args.kwonlyargs)
        ]
        default_specs = _default_specs_from_args(stmt.args)
        defaults[stmt.name] = {"params": len(params), "defaults": default_specs}
    return defaults


def _topo_sort_modules(
    module_graph: dict[str, Path], module_deps: dict[str, set[str]]
) -> list[str]:
    in_degree = {name: 0 for name in module_graph}
    dependents = _reverse_module_dependencies(module_deps, module_graph)
    for name, deps in module_deps.items():
        for dep in deps:
            in_degree[name] += 1
    ready = deque(sorted(name for name, degree in in_degree.items() if degree == 0))
    order: list[str] = []
    while ready:
        name = ready.popleft()
        order.append(name)
        for child in sorted(dependents[name]):
            in_degree[child] -= 1
            if in_degree[child] == 0:
                ready.append(child)
    if len(order) != len(module_graph):
        remaining = sorted(name for name in module_graph if name not in order)
        order.extend(remaining)
    return order


def _analyze_module_schedule(
    module_graph: Mapping[str, Path],
    module_deps: Mapping[str, set[str]],
) -> tuple[
    list[str],
    dict[str, set[str]],
    bool,
    list[list[str]],
    dict[str, frozenset[str]],
]:
    module_names = set(module_graph)
    in_degree = {name: 0 for name in module_names}
    reverse_module_deps = _reverse_module_dependencies(dict(module_deps), module_names)
    for name, deps in module_deps.items():
        for dep in deps:
            if dep in module_names and name in in_degree:
                in_degree[name] += 1
    ready = deque(sorted(name for name, degree in in_degree.items() if degree == 0))
    order: list[str] = []
    while ready:
        name = ready.popleft()
        order.append(name)
        for child in sorted(reverse_module_deps.get(name, ())):
            if child not in in_degree:
                continue
            in_degree[child] -= 1
            if in_degree[child] == 0:
                ready.append(child)
    has_back_edges = len(order) != len(module_names)
    if has_back_edges:
        remaining = sorted(name for name in module_names if name not in order)
        order.extend(remaining)
    layers = _module_dependency_layers(order, dict(module_deps))
    module_dep_closures = _module_dependency_closures(
        dict(module_deps),
        module_names,
        module_order=order,
        has_back_edges=has_back_edges,
    )
    return order, reverse_module_deps, has_back_edges, layers, module_dep_closures


def _reverse_module_dependencies(
    module_deps: dict[str, set[str]],
    module_names: Collection[str],
) -> dict[str, set[str]]:
    dependents: dict[str, set[str]] = {name: set() for name in module_names}
    for name, deps in module_deps.items():
        if name not in dependents:
            dependents[name] = set()
        for dep in deps:
            dependents.setdefault(dep, set()).add(name)
    return dependents


def _dependent_module_closure(
    dirty_modules: Collection[str],
    module_deps: dict[str, set[str]],
    module_names: Collection[str],
    reverse_module_deps: Mapping[str, set[str]] | None = None,
) -> set[str]:
    dependents = (
        reverse_module_deps
        if reverse_module_deps is not None
        else _reverse_module_dependencies(module_deps, module_names)
    )
    closure: set[str] = {name for name in dirty_modules if name in dependents}
    queue = deque(sorted(closure))
    while queue:
        module_name = queue.popleft()
        for dependent in sorted(dependents.get(module_name, ())):
            if dependent not in closure:
                closure.add(dependent)
                queue.append(dependent)
    return closure


def _module_dependency_closure(
    module_name: str,
    module_deps: dict[str, set[str]],
) -> set[str]:
    closure: set[str] = {module_name}
    queue = deque([module_name])
    while queue:
        current = queue.popleft()
        for dep in sorted(module_deps.get(current, ())):
            if dep not in closure:
                closure.add(dep)
                queue.append(dep)
    return closure


def _module_dependency_closures(
    module_deps: dict[str, set[str]],
    module_names: Collection[str],
    *,
    module_order: Sequence[str] | None = None,
    has_back_edges: bool = False,
) -> dict[str, frozenset[str]]:
    if module_order is not None and not has_back_edges:
        closures: dict[str, frozenset[str]] = {}
        for module_name in tuple(module_order):
            closure: set[str] = {module_name}
            for dep in module_deps.get(module_name, ()):
                closure.update(closures.get(dep, frozenset({dep})))
            closures[module_name] = frozenset(closure)
        for module_name in module_names:
            closures.setdefault(module_name, frozenset({module_name}))
        return closures
    closures: dict[str, frozenset[str]] = {}
    for module_name in sorted(module_names):
        closures[module_name] = frozenset(
            _module_dependency_closure(module_name, module_deps)
        )
    return closures


def _scoped_known_func_defaults(
    module_name: str,
    *,
    module_deps: dict[str, set[str]],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    module_dep_closures: dict[str, frozenset[str]] | None = None,
) -> dict[str, dict[str, dict[str, Any]]]:
    scoped_names = module_dep_closures.get(module_name) if module_dep_closures else None
    if scoped_names is None:
        scoped_names = _module_dependency_closure(module_name, module_deps)
    return {
        name: known_func_defaults[name]
        for name in sorted(scoped_names)
        if name in known_func_defaults
    }


def _scoped_known_modules(
    module_name: str,
    *,
    module_deps: dict[str, set[str]],
    known_modules: Collection[str],
    module_dep_closures: dict[str, frozenset[str]] | None = None,
) -> tuple[str, ...]:
    scoped_names = module_dep_closures.get(module_name) if module_dep_closures else None
    if scoped_names is None:
        scoped_names = _module_dependency_closure(module_name, module_deps)
    known_modules_set = set(known_modules)
    return tuple(
        sorted(
            name
            for name in scoped_names
            if name == module_name or name in known_modules_set
        )
    )


def _scoped_known_classes(
    module_name: str,
    *,
    module_deps: dict[str, set[str]],
    known_classes: dict[str, Any],
    module_dep_closures: dict[str, frozenset[str]] | None = None,
) -> dict[str, Any]:
    scoped_modules = (
        module_dep_closures.get(module_name) if module_dep_closures else None
    )
    if scoped_modules is None:
        scoped_modules = _module_dependency_closure(module_name, module_deps)
    return {
        class_name: class_info
        for class_name, class_info in known_classes.items()
        if isinstance(class_info, dict) and class_info.get("module") in scoped_modules
    }


def _scoped_type_facts(
    module_name: str,
    *,
    module_deps: dict[str, set[str]],
    type_facts: TypeFacts | None,
    module_dep_closures: dict[str, frozenset[str]] | None = None,
) -> TypeFacts | None:
    if type_facts is None:
        return None
    scoped_modules = (
        module_dep_closures.get(module_name) if module_dep_closures else None
    )
    if scoped_modules is None:
        scoped_modules = _module_dependency_closure(module_name, module_deps)
    modules = getattr(type_facts, "modules", None)
    if not isinstance(modules, dict):
        return type_facts
    filtered_modules = {
        name: module for name, module in modules.items() if name in scoped_modules
    }
    if len(filtered_modules) == len(modules):
        return type_facts
    return TypeFacts(
        schema_version=type_facts.schema_version,
        created_at=type_facts.created_at,
        tool=type_facts.tool,
        strict=type_facts.strict,
        modules=filtered_modules,
    )


def _build_scoped_lowering_inputs(
    module_names: Collection[str],
    *,
    module_deps: dict[str, set[str]],
    module_dep_closures: dict[str, frozenset[str]],
    known_modules: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    pgo_hot_function_names: Collection[str],
    type_facts: TypeFacts | None,
) -> _ScopedLoweringInputs:
    scoped_known_modules_by_module: dict[str, tuple[str, ...]] = {}
    scoped_known_func_defaults_by_module: dict[str, dict[str, dict[str, Any]]] = {}
    scoped_pgo_hot_function_names_by_module: dict[str, tuple[str, ...]] = {}
    scoped_type_facts_by_module: dict[str, TypeFacts | None] = {}
    for module_name in sorted(module_names):
        scoped_known_modules_by_module[module_name] = _scoped_known_modules(
            module_name,
            module_deps=module_deps,
            known_modules=known_modules,
            module_dep_closures=module_dep_closures,
        )
        scoped_known_func_defaults_by_module[module_name] = _scoped_known_func_defaults(
            module_name,
            module_deps=module_deps,
            known_func_defaults=known_func_defaults,
            module_dep_closures=module_dep_closures,
        )
        scoped_pgo_hot_function_names_by_module[module_name] = (
            _scoped_pgo_hot_function_names(module_name, pgo_hot_function_names)
        )
        scoped_type_facts_by_module[module_name] = _scoped_type_facts(
            module_name,
            module_deps=module_deps,
            type_facts=type_facts,
            module_dep_closures=module_dep_closures,
        )
    return _ScopedLoweringInputs(
        known_modules_by_module=scoped_known_modules_by_module,
        known_func_defaults_by_module=scoped_known_func_defaults_by_module,
        pgo_hot_function_names_by_module=scoped_pgo_hot_function_names_by_module,
        type_facts_by_module=scoped_type_facts_by_module,
    )


def _build_scoped_known_classes_snapshot(
    module_names: Collection[str],
    *,
    module_deps: dict[str, set[str]],
    module_dep_closures: dict[str, frozenset[str]],
    known_classes_snapshot: dict[str, Any],
) -> dict[str, dict[str, Any]]:
    scoped_known_classes_by_module: dict[str, dict[str, Any]] = {}
    for module_name in sorted(module_names):
        scoped_known_classes_by_module[module_name] = _scoped_known_classes(
            module_name,
            module_deps=module_deps,
            known_classes=known_classes_snapshot,
            module_dep_closures=module_dep_closures,
        )
    return scoped_known_classes_by_module


def _scoped_known_classes_view(
    module_name: str,
    *,
    module_deps: dict[str, set[str]],
    known_classes_snapshot: dict[str, Any],
    module_dep_closures: dict[str, frozenset[str]] | None = None,
    scoped_known_classes_by_module: Mapping[str, dict[str, Any]] | None = None,
) -> dict[str, Any]:
    if (
        scoped_known_classes_by_module is not None
        and module_name in scoped_known_classes_by_module
    ):
        return scoped_known_classes_by_module[module_name]
    return _scoped_known_classes(
        module_name,
        module_deps=module_deps,
        known_classes=known_classes_snapshot,
        module_dep_closures=module_dep_closures,
    )


def _scoped_lowering_input_view(
    module_name: str,
    *,
    module_deps: dict[str, set[str]],
    known_modules: Collection[str],
    known_func_defaults: dict[str, dict[str, dict[str, Any]]],
    pgo_hot_function_names: Collection[str],
    type_facts: TypeFacts | None,
    module_dep_closures: dict[str, frozenset[str]] | None = None,
    scoped_lowering_inputs: _ScopedLoweringInputs | None = None,
    known_modules_sorted: tuple[str, ...] | None = None,
    pgo_hot_function_names_sorted: tuple[str, ...] | None = None,
) -> _ScopedLoweringInputView:
    if (
        scoped_lowering_inputs is not None
        and module_name in scoped_lowering_inputs.known_modules_by_module
    ):
        scoped_known_modules = scoped_lowering_inputs.known_modules_by_module[module_name]
    else:
        known_modules_scope_source: Collection[str]
        if known_modules_sorted is None:
            known_modules_scope_source = known_modules
        else:
            known_modules_scope_source = known_modules_sorted
        scoped_known_modules = _scoped_known_modules(
            module_name,
            module_deps=module_deps,
            known_modules=known_modules_scope_source,
            module_dep_closures=module_dep_closures,
        )
    if (
        scoped_lowering_inputs is not None
        and module_name in scoped_lowering_inputs.known_func_defaults_by_module
    ):
        scoped_known_func_defaults = (
            scoped_lowering_inputs.known_func_defaults_by_module[module_name]
        )
    else:
        scoped_known_func_defaults = _scoped_known_func_defaults(
            module_name,
            module_deps=module_deps,
            known_func_defaults=known_func_defaults,
            module_dep_closures=module_dep_closures,
        )
    if (
        scoped_lowering_inputs is not None
        and module_name in scoped_lowering_inputs.pgo_hot_function_names_by_module
    ):
        scoped_pgo_hot_function_names = (
            scoped_lowering_inputs.pgo_hot_function_names_by_module[module_name]
        )
    else:
        pgo_hot_functions_scope_source: Collection[str]
        if pgo_hot_function_names_sorted is None:
            pgo_hot_functions_scope_source = pgo_hot_function_names
        else:
            pgo_hot_functions_scope_source = pgo_hot_function_names_sorted
        scoped_pgo_hot_function_names = _scoped_pgo_hot_function_names(
            module_name,
            pgo_hot_functions_scope_source,
        )
    if (
        scoped_lowering_inputs is not None
        and module_name in scoped_lowering_inputs.type_facts_by_module
    ):
        scoped_type_facts = scoped_lowering_inputs.type_facts_by_module[module_name]
    else:
        scoped_type_facts = _scoped_type_facts(
            module_name,
            module_deps=module_deps,
            type_facts=type_facts,
            module_dep_closures=module_dep_closures,
        )
    return _ScopedLoweringInputView(
        known_modules=scoped_known_modules,
        known_func_defaults=scoped_known_func_defaults,
        pgo_hot_function_names=scoped_pgo_hot_function_names,
        type_facts=scoped_type_facts,
        known_modules_payload=list(scoped_known_modules),
        known_modules_set=frozenset(scoped_known_modules),
        pgo_hot_function_names_payload=list(scoped_pgo_hot_function_names),
        pgo_hot_function_names_set=frozenset(scoped_pgo_hot_function_names),
    )


def _build_module_lowering_metadata(
    module_graph: Mapping[str, Path],
    *,
    generated_module_source_paths: Mapping[str, str],
    entry_module: str,
    namespace_module_names: Collection[str],
) -> tuple[
    dict[str, str],
    dict[str, str | None],
    dict[str, bool],
    dict[str, bool],
]:
    logical_source_path_by_module: dict[str, str] = {}
    entry_override_by_module: dict[str, str | None] = {}
    module_is_namespace_by_module: dict[str, bool] = {}
    module_is_package_by_module: dict[str, bool] = {}
    namespace_modules = set(namespace_module_names)
    for module_name in sorted(module_graph):
        module_path = module_graph[module_name]
        logical_source_path_by_module[module_name] = generated_module_source_paths.get(
            module_name, str(module_path)
        )
        entry_override_by_module[module_name] = (
            None
            if module_name == entry_module and entry_module != "__main__"
            else entry_module
        )
        module_is_namespace_by_module[module_name] = module_name in namespace_modules
        module_is_package_by_module[module_name] = module_path.name == "__init__.py"
    return (
        logical_source_path_by_module,
        entry_override_by_module,
        module_is_namespace_by_module,
        module_is_package_by_module,
    )


def _scoped_pgo_hot_function_names(
    module_name: str,
    pgo_hot_function_names: Collection[str],
) -> tuple[str, ...]:
    if not pgo_hot_function_names:
        return ()
    module_prefix_a = f"{module_name}::"
    module_prefix_b = f"{module_name}."
    init_symbol = SimpleTIRGenerator.module_init_symbol(module_name)
    scoped = {
        symbol
        for symbol in pgo_hot_function_names
        if symbol.startswith(module_prefix_a)
        or symbol.startswith(module_prefix_b)
        or symbol == init_symbol
        or symbol == f"{module_name}::{init_symbol}"
        or symbol == f"{module_name}.{init_symbol}"
    }
    return tuple(sorted(scoped))


@functools.lru_cache(maxsize=8)
def _stdlib_allowlist_cached(project_root_text: str | None) -> frozenset[str]:
    allowlist: set[str] = set()
    spec_path = Path("docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md")
    if not spec_path.exists():
        if project_root_text:
            spec_path = (
                Path(project_root_text)
                / "docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md"
            )
        else:
            spec_path = (
                Path(__file__).resolve().parents[2]
                / "docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md"
            )
    if not spec_path.exists():
        return allowlist
    for line in spec_path.read_text().splitlines():
        if not line.startswith("|"):
            continue
        if line.startswith("| ---"):
            continue
        parts = [part.strip() for part in line.strip().strip("|").split("|")]
        if not parts:
            continue
        module_name = parts[0]
        if not module_name or module_name == "Module":
            continue
        for entry in module_name.split("/"):
            entry = entry.strip()
            if entry:
                allowlist.add(entry)
    return frozenset(allowlist)


def _stdlib_allowlist() -> set[str]:
    project_root = os.environ.get("MOLT_PROJECT_ROOT")
    return set(_stdlib_allowlist_cached(project_root))


_INTRINSIC_CALL_NAMES = {
    "load_intrinsic",
    "require_intrinsic",
    "require_optional_intrinsic",
    "_load_intrinsic",
    "_intrinsic_load",
    "_intrinsics_require",
    "_intrinsic_require",
    "_require_intrinsic",
    "_require_callable_intrinsic",
}
_STDLIB_PROBE_INTRINSIC = "molt_stdlib_probe"


def _stdlib_module_intrinsic_status(path: Path) -> str:
    try:
        source = path.read_text(encoding="utf-8")
    except Exception:
        return "python-only"

    if path.name == "_intrinsics.py":
        return "intrinsic-backed"

    intrinsic_names: set[str] = set()
    try:
        tree = ast.parse(source)
    except SyntaxError:
        return "python-only"

    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        call_name: str | None = None
        if isinstance(node.func, ast.Name):
            call_name = node.func.id
        elif isinstance(node.func, ast.Attribute):
            call_name = node.func.attr
        if call_name not in _INTRINSIC_CALL_NAMES and call_name != "_lazy_intrinsic":
            continue
        first: ast.expr | None = None
        if node.args:
            first = node.args[0]
        else:
            for keyword in node.keywords:
                if keyword.arg == "name":
                    first = keyword.value
                    break
        if not isinstance(first, ast.Constant) or not isinstance(first.value, str):
            continue
        name = first.value
        if name.startswith("molt_"):
            intrinsic_names.add(name)

    if not intrinsic_names:
        return "python-only"
    if intrinsic_names == {_STDLIB_PROBE_INTRINSIC}:
        return "probe-only"
    return "intrinsic-backed"


def _enforce_intrinsic_stdlib(
    module_graph: dict[str, Path],
    stdlib_root: Path,
    json_output: bool,
) -> int | None:
    missing: list[str] = []
    probe_only: list[str] = []
    stdlib_root = stdlib_root.resolve()
    for name, path in module_graph.items():
        if not path or not path.suffix == ".py":
            continue
        try:
            path.resolve().relative_to(stdlib_root)
        except ValueError:
            continue
        status = _stdlib_module_intrinsic_status(path)
        if status == "python-only":
            missing.append(name)
        elif status == "probe-only":
            probe_only.append(name)
    if not missing:
        return None
    missing.sort()
    probe_only.sort()
    message = (
        "Intrinsic-only stdlib enforcement failed. These modules are Python-only "
        "and must be lowered to Rust intrinsics (or become thin intrinsic wrappers):\n"
        + "\n".join(f"  - {name}" for name in missing)
    )
    if probe_only:
        message += (
            "\n\nProbe-only modules in this build (thin wrappers + policy gate only):\n"
            + "\n".join(f"  - {name}" for name in probe_only)
        )
    return _fail(message, json_output, command="build")


def _is_stdlib_module(name: str, stdlib_allowlist: set[str]) -> bool:
    if name.startswith("molt."):
        return False
    if name in stdlib_allowlist:
        return True
    top = name.split(".", 1)[0]
    return top in stdlib_allowlist


def _roots_for_module(
    module_name: str,
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
) -> list[Path]:
    if _is_stdlib_module(module_name, stdlib_allowlist):
        if module_name == "test.tokenizedata" or module_name.startswith(
            "test.tokenizedata."
        ):
            return [stdlib_root] + [root for root in roots if root != stdlib_root]
        if module_name == "test" or module_name.startswith("test."):
            if os.environ.get("MOLT_REGRTEST_CPYTHON_DIR"):
                return roots
        return [stdlib_root]
    return roots


def _ensure_core_stdlib_modules(
    module_graph: dict[str, Path], stdlib_root: Path
) -> None:
    for name in (
        "builtins",
        "sys",
        "types",
        "importlib",
        "importlib.util",
        "importlib.machinery",
    ):
        path = _resolve_module_path(name, [stdlib_root])
        if path is not None:
            module_graph.setdefault(name, path)


def _record_module_reason(
    module_reasons: dict[str, set[str]],
    module_name: str,
    reason: str,
) -> None:
    module_reasons.setdefault(module_name, set()).add(reason)


def _merge_module_graph_with_reason(
    module_graph: dict[str, Path],
    additions: dict[str, Path],
    module_reasons: dict[str, set[str]],
    reason: str,
) -> None:
    for name, path in additions.items():
        _record_module_reason(module_reasons, name, reason)
        module_graph.setdefault(name, path)


def _record_new_module_reasons(
    module_graph: dict[str, Path],
    before_names: set[str],
    module_reasons: dict[str, set[str]],
    reason: str,
) -> None:
    for name in module_graph:
        if name in before_names:
            continue
        _record_module_reason(module_reasons, name, reason)


def _build_reason_summary(
    module_reasons: dict[str, set[str]],
) -> dict[str, int]:
    summary: dict[str, int] = {}
    for reasons in module_reasons.values():
        for reason in reasons:
            summary[reason] = summary.get(reason, 0) + 1
    return {name: summary[name] for name in sorted(summary)}


def _build_diagnostics_enabled() -> bool:
    return _coerce_bool(os.environ.get("MOLT_BUILD_DIAGNOSTICS", ""), False)


def _build_allocation_diagnostics_enabled() -> bool:
    return _coerce_bool(os.environ.get("MOLT_BUILD_ALLOCATIONS", ""), False)


def _resolve_build_diagnostics_verbosity(raw: str | None) -> str:
    value = (raw or "").strip().lower()
    if value in {"", "default", "normal", "standard"}:
        return "default"
    if value in {"summary", "compact", "brief"}:
        return "summary"
    if value in {"full", "verbose", "detailed"}:
        return "full"
    return "default"


def _phase_duration_map(phase_starts: dict[str, float]) -> dict[str, float]:
    if not phase_starts:
        return {}
    starts = sorted(phase_starts.items(), key=lambda item: item[1])
    durations: dict[str, float] = {}
    for idx, (name, started) in enumerate(starts):
        if idx + 1 < len(starts):
            ended = starts[idx + 1][1]
        else:
            ended = time.perf_counter()
        durations[name] = round(max(0.0, ended - started), 6)
    return durations


def _resolve_build_diagnostics_path(
    output_spec: str,
    artifacts_root: Path,
) -> Path:
    path = Path(output_spec).expanduser()
    if not path.is_absolute():
        path = artifacts_root / path
    return path


def _capture_build_allocation_diagnostics(*, top_n: int = 10) -> dict[str, Any] | None:
    if not tracemalloc.is_tracing():
        return None
    current_bytes, peak_bytes = tracemalloc.get_traced_memory()
    snapshot = tracemalloc.take_snapshot()
    top_allocations: list[dict[str, Any]] = []
    for stat in snapshot.statistics("lineno")[: max(0, top_n)]:
        frame = stat.traceback[0]
        top_allocations.append(
            {
                "file": frame.filename,
                "line": frame.lineno,
                "size_bytes": stat.size,
                "count": stat.count,
            }
        )
    return {
        "current_bytes": current_bytes,
        "peak_bytes": peak_bytes,
        "top": top_allocations,
    }


def _emit_build_diagnostics(
    *,
    diagnostics: dict[str, Any] | None,
    diagnostics_path: Path | None,
    json_output: bool,
    verbosity: str = "default",
) -> None:
    if diagnostics is None:
        return
    if diagnostics_path is not None:
        diagnostics_path.parent.mkdir(parents=True, exist_ok=True)
        diagnostics_path.write_text(json.dumps(diagnostics, indent=2) + "\n")
    if json_output:
        return
    resolved_verbosity = _resolve_build_diagnostics_verbosity(verbosity)
    summary_only = resolved_verbosity == "summary"
    full_details = resolved_verbosity == "full"
    phase_sec = diagnostics.get("phase_sec", {})
    total_sec = diagnostics.get("total_sec")
    module_count = diagnostics.get("module_count")
    reason_summary = diagnostics.get("module_reason_summary", {})
    midend = diagnostics.get("midend", {})
    frontend_parallel = diagnostics.get("frontend_parallel", {})
    frontend_modules_top = diagnostics.get("frontend_module_timings_top", [])
    allocations = diagnostics.get("allocations", {})
    print("Build diagnostics:", file=sys.stderr)
    if isinstance(total_sec, (int, float)):
        print(f"- total_sec: {total_sec:.6f}", file=sys.stderr)
    if isinstance(module_count, int):
        print(f"- modules: {module_count}", file=sys.stderr)
    if isinstance(phase_sec, dict):
        for name in sorted(phase_sec):
            value = phase_sec[name]
            if isinstance(value, (int, float)):
                print(f"- phase.{name}: {value:.6f}s", file=sys.stderr)
    if isinstance(reason_summary, dict):
        for name in sorted(reason_summary):
            value = reason_summary[name]
            if isinstance(value, int):
                print(f"- reason.{name}: {value}", file=sys.stderr)
    if isinstance(allocations, dict):
        current_bytes = allocations.get("current_bytes")
        peak_bytes = allocations.get("peak_bytes")
        if isinstance(current_bytes, int):
            print(f"- alloc.current_bytes: {current_bytes}", file=sys.stderr)
        if isinstance(peak_bytes, int):
            print(f"- alloc.peak_bytes: {peak_bytes}", file=sys.stderr)
        top_allocations = allocations.get("top")
        if not summary_only and isinstance(top_allocations, list):
            limit = 20 if full_details else 10
            for idx, item in enumerate(top_allocations[:limit], start=1):
                if not isinstance(item, dict):
                    continue
                file_name = str(item.get("file", ""))
                line_no = int(item.get("line", 0))
                size_bytes = int(item.get("size_bytes", 0))
                count = int(item.get("count", 0))
                print(
                    "- alloc.top."
                    f"{idx}: {file_name}:{line_no} size_bytes={size_bytes} count={count}",
                    file=sys.stderr,
                )
    if isinstance(frontend_modules_top, list):
        limit = 20 if full_details else 10
        for idx, item in enumerate(frontend_modules_top[:limit], start=1):
            if not isinstance(item, dict):
                continue
            module_name = str(item.get("module", ""))
            total_s = float(item.get("total_s", 0.0))
            visit_s = float(item.get("visit_s", 0.0))
            lower_s = float(item.get("lower_s", 0.0))
            print(
                "- frontend.hotspot."
                f"{idx}: {module_name} total_s={total_s:.6f} "
                f"visit_s={visit_s:.6f} lower_s={lower_s:.6f}",
                file=sys.stderr,
            )
    if isinstance(frontend_parallel, dict):
        enabled = bool(frontend_parallel.get("enabled", False))
        workers = int(frontend_parallel.get("workers", 0))
        mode = str(frontend_parallel.get("mode", "serial"))
        print(
            f"- frontend_parallel: enabled={enabled} workers={workers} mode={mode}",
            file=sys.stderr,
        )
        reason = frontend_parallel.get("reason")
        if isinstance(reason, str) and reason:
            print(f"- frontend_parallel.reason: {reason}", file=sys.stderr)
        policy = frontend_parallel.get("policy")
        if isinstance(policy, dict):
            min_modules = int(policy.get("min_modules", 0))
            min_predicted_cost = float(policy.get("min_predicted_cost", 0.0))
            target_cost = float(policy.get("target_cost_per_worker", 0.0))
            print(
                "- frontend_parallel.policy: "
                f"min_modules={min_modules} "
                f"min_predicted_cost={min_predicted_cost:.3f} "
                f"target_cost_per_worker={target_cost:.3f}",
                file=sys.stderr,
            )
        layer_stats = frontend_parallel.get("layers")
        if not summary_only and isinstance(layer_stats, list):
            limit = 20 if full_details else 10
            print(f"- frontend_parallel.layers: {len(layer_stats)}", file=sys.stderr)
            for item in layer_stats[:limit]:
                if not isinstance(item, dict):
                    continue
                layer_index = int(item.get("index", 0)) + 1
                layer_mode = str(item.get("mode", "serial"))
                layer_modules = int(item.get("module_count", 0))
                layer_candidates = int(item.get("candidate_count", 0))
                layer_workers = int(item.get("workers", 0))
                queue_ms_total = float(item.get("queue_ms_total", 0.0))
                wait_ms_total = float(item.get("wait_ms_total", 0.0))
                exec_ms_total = float(item.get("exec_ms_total", 0.0))
                print(
                    "- frontend_parallel.layer."
                    f"{layer_index}: mode={layer_mode} modules={layer_modules} "
                    f"candidates={layer_candidates} workers={layer_workers} "
                    f"queue_ms={queue_ms_total:.3f} wait_ms={wait_ms_total:.3f} "
                    f"exec_ms={exec_ms_total:.3f}",
                    file=sys.stderr,
                )
            if len(layer_stats) > limit:
                print(
                    f"- frontend_parallel.layers_truncated: {len(layer_stats) - limit}",
                    file=sys.stderr,
                )
        worker_stats = frontend_parallel.get("worker_summary")
        if not summary_only and isinstance(worker_stats, dict):
            worker_count = int(worker_stats.get("count", 0))
            queue_ms_total = float(worker_stats.get("queue_ms_total", 0.0))
            wait_ms_total = float(worker_stats.get("wait_ms_total", 0.0))
            exec_ms_total = float(worker_stats.get("exec_ms_total", 0.0))
            queue_ms_max = float(worker_stats.get("queue_ms_max", 0.0))
            wait_ms_max = float(worker_stats.get("wait_ms_max", 0.0))
            exec_ms_max = float(worker_stats.get("exec_ms_max", 0.0))
            print(
                "- frontend_parallel.worker_ms: "
                f"count={worker_count} queue_total={queue_ms_total:.3f} "
                f"wait_total={wait_ms_total:.3f} exec_total={exec_ms_total:.3f} "
                f"queue_max={queue_ms_max:.3f} wait_max={wait_ms_max:.3f} "
                f"exec_max={exec_ms_max:.3f}",
                file=sys.stderr,
            )
    if isinstance(midend, dict):
        requested_profile = midend.get("requested_profile")
        if isinstance(requested_profile, str) and requested_profile:
            print(f"- midend.profile: {requested_profile}", file=sys.stderr)
        policy_config = midend.get("policy_config")
        if isinstance(policy_config, dict):
            profile_override = policy_config.get("profile_override")
            if isinstance(profile_override, str) and profile_override:
                print(
                    f"- midend.policy.profile_override: {profile_override}",
                    file=sys.stderr,
                )
            hot_tier_promotion_enabled = policy_config.get("hot_tier_promotion_enabled")
            if isinstance(hot_tier_promotion_enabled, bool):
                print(
                    "- midend.policy.hot_tier_promotion_enabled: "
                    f"{hot_tier_promotion_enabled}",
                    file=sys.stderr,
                )
            budget_override_ms = policy_config.get("budget_override_ms")
            if isinstance(budget_override_ms, (int, float)):
                print(
                    f"- midend.policy.budget_override_ms: {budget_override_ms:.4f}",
                    file=sys.stderr,
                )
            budget_alpha = policy_config.get("budget_alpha")
            budget_beta = policy_config.get("budget_beta")
            budget_scale = policy_config.get("budget_scale")
            if all(
                isinstance(value, (int, float))
                for value in (budget_alpha, budget_beta, budget_scale)
            ):
                print(
                    "- midend.policy.budget_formula: "
                    f"alpha={float(budget_alpha):.4f} "
                    f"beta={float(budget_beta):.4f} "
                    f"scale={float(budget_scale):.4f}",
                    file=sys.stderr,
                )
        degraded_functions = midend.get("degraded_functions")
        if isinstance(degraded_functions, int):
            print(
                f"- midend.degraded_functions: {degraded_functions}",
                file=sys.stderr,
            )
        tier_summary = midend.get("tier_summary")
        if isinstance(tier_summary, dict):
            for tier in sorted(tier_summary):
                value = tier_summary[tier]
                if isinstance(value, int):
                    print(f"- midend.tier.{tier}: {value}", file=sys.stderr)
        tier_base_summary = midend.get("tier_base_summary")
        if isinstance(tier_base_summary, dict):
            for tier in sorted(tier_base_summary):
                value = tier_base_summary[tier]
                if isinstance(value, int):
                    print(f"- midend.tier_base.{tier}: {value}", file=sys.stderr)
        promoted_functions = midend.get("promoted_functions")
        if isinstance(promoted_functions, int):
            print(
                f"- midend.promoted_functions: {promoted_functions}",
                file=sys.stderr,
            )
        promotion_source_summary = midend.get("promotion_source_summary")
        if isinstance(promotion_source_summary, dict):
            for source in sorted(promotion_source_summary):
                value = promotion_source_summary[source]
                if isinstance(value, int):
                    print(
                        f"- midend.promotion_source.{source}: {value}",
                        file=sys.stderr,
                    )
        reason_counts = midend.get("degrade_reason_summary")
        if isinstance(reason_counts, dict):
            for reason in sorted(reason_counts):
                value = reason_counts[reason]
                if isinstance(value, int):
                    print(f"- midend.degrade_reason.{reason}: {value}", file=sys.stderr)
        hotspots = midend.get("pass_hotspots_top")
        if not summary_only and isinstance(hotspots, list):
            limit = 20 if full_details else 10
            for idx, item in enumerate(hotspots[:limit], start=1):
                if not isinstance(item, dict):
                    continue
                module_name = str(item.get("module", ""))
                function_name = str(item.get("function", ""))
                pass_name = str(item.get("pass", ""))
                total_ms = float(item.get("ms_total", 0.0))
                p95_ms = float(item.get("ms_p95", 0.0))
                print(
                    "- midend.hotspot."
                    f"{idx}: {module_name}::{function_name}:{pass_name} "
                    f"total_ms={total_ms:.3f} p95_ms={p95_ms:.3f}",
                    file=sys.stderr,
                )
        function_hotspots = midend.get("function_hotspots_top")
        if not summary_only and isinstance(function_hotspots, list):
            limit = 20 if full_details else 10
            for idx, item in enumerate(function_hotspots[:limit], start=1):
                if not isinstance(item, dict):
                    continue
                module_name = str(item.get("module", ""))
                function_name = str(item.get("function", ""))
                spent_ms = float(item.get("spent_ms", 0.0))
                budget_ms = float(item.get("budget_ms", 0.0))
                degraded = bool(item.get("degraded", False))
                print(
                    "- midend.function_hotspot."
                    f"{idx}: {module_name}::{function_name} "
                    f"spent_ms={spent_ms:.3f} budget_ms={budget_ms:.3f} "
                    f"degraded={degraded}",
                    file=sys.stderr,
                )
        promotion_hotspots = midend.get("promotion_hotspots_top")
        if not summary_only and isinstance(promotion_hotspots, list):
            limit = 20 if full_details else 10
            for idx, item in enumerate(promotion_hotspots[:limit], start=1):
                if not isinstance(item, dict):
                    continue
                module_name = str(item.get("module", ""))
                function_name = str(item.get("function", ""))
                tier_base = str(item.get("tier_base", ""))
                tier_effective = str(item.get("tier_effective", ""))
                source = str(item.get("source", ""))
                signal = str(item.get("signal", ""))
                spent_ms = float(item.get("spent_ms", 0.0))
                print(
                    "- midend.promotion_hotspot."
                    f"{idx}: {module_name}::{function_name} "
                    f"{tier_base}->{tier_effective} source={source} "
                    f"signal={signal} spent_ms={spent_ms:.3f}",
                    file=sys.stderr,
                )
        degrade_hotspots = midend.get("degrade_event_hotspots_top")
        if not summary_only and isinstance(degrade_hotspots, list):
            limit = 20 if full_details else 10
            for idx, item in enumerate(degrade_hotspots[:limit], start=1):
                if not isinstance(item, dict):
                    continue
                module_name = str(item.get("module", ""))
                function_name = str(item.get("function", ""))
                reason = str(item.get("reason", ""))
                action = str(item.get("action", ""))
                spent_ms = float(item.get("spent_ms", 0.0))
                print(
                    "- midend.degrade_hotspot."
                    f"{idx}: {module_name}::{function_name} reason={reason} "
                    f"action={action} spent_ms={spent_ms:.3f}",
                    file=sys.stderr,
                )
        budget_util_avg = midend.get("budget_utilization_avg")
        budget_util_p95 = midend.get("budget_utilization_p95")
        over_budget = midend.get("functions_over_budget")
        under_50 = midend.get("functions_under_50pct_budget")
        if isinstance(budget_util_avg, (int, float)):
            print(
                f"- midend.budget_utilization_avg: {budget_util_avg:.4f}",
                file=sys.stderr,
            )
        if isinstance(budget_util_p95, (int, float)):
            print(
                f"- midend.budget_utilization_p95: {budget_util_p95:.4f}",
                file=sys.stderr,
            )
        if isinstance(over_budget, int):
            print(
                f"- midend.functions_over_budget: {over_budget}",
                file=sys.stderr,
            )
        if isinstance(under_50, int):
            print(
                f"- midend.functions_under_50pct_budget: {under_50}",
                file=sys.stderr,
            )
        budget_ranked_functions: list[dict[str, Any]] = []
        if not summary_only and isinstance(function_hotspots, list):
            for item in function_hotspots:
                if not isinstance(item, dict):
                    continue
                b_ms = float(item.get("budget_ms", 0.0))
                s_ms = float(item.get("spent_ms", 0.0))
                if b_ms > 0.0:
                    budget_ranked_functions.append(
                        {
                            "module": str(item.get("module", "")),
                            "function": str(item.get("function", "")),
                            "ratio": s_ms / b_ms,
                            "spent_ms": s_ms,
                            "budget_ms": b_ms,
                        }
                    )
            budget_ranked_functions.sort(key=lambda x: -x["ratio"])
            limit = 10 if full_details else 5
            for idx, item in enumerate(budget_ranked_functions[:limit], start=1):
                print(
                    "- midend.budget_top."
                    f"{idx}: {item['module']}::{item['function']} "
                    f"ratio={item['ratio']:.4f} "
                    f"spent_ms={item['spent_ms']:.3f} "
                    f"budget_ms={item['budget_ms']:.3f}",
                    file=sys.stderr,
                )
        pass_wall_ranked = midend.get("pass_wall_time_ranked")
        if not summary_only and isinstance(pass_wall_ranked, list):
            limit = 10 if full_details else 3
            for idx, item in enumerate(pass_wall_ranked[:limit], start=1):
                if not isinstance(item, dict):
                    continue
                pass_name = str(item.get("pass", ""))
                ms_total = float(item.get("ms_total", 0.0))
                print(
                    "- midend.pass_wall_top."
                    f"{idx}: {pass_name} ms_total={ms_total:.3f}",
                    file=sys.stderr,
                )
        promo_candidates = midend.get("promotion_candidates")
        if not summary_only and isinstance(promo_candidates, list) and promo_candidates:
            print(
                f"- midend.promotion_candidates: {len(promo_candidates)}",
                file=sys.stderr,
            )
    if diagnostics_path is not None:
        print(f"- wrote: {diagnostics_path}", file=sys.stderr)


def _midend_sample_percentile(samples: list[float], pct: float) -> float:
    if not samples:
        return 0.0
    ordered = sorted(samples)
    idx = max(0, min(len(ordered) - 1, int((len(ordered) - 1) * pct)))
    return float(ordered[idx])


def _midend_sample_p95(samples: list[float]) -> float:
    return _midend_sample_percentile(samples, 0.95)


def _midend_policy_config_snapshot() -> dict[str, Any]:
    profile_override = os.environ.get("MOLT_MIDEND_PROFILE", "").strip().lower()
    budget_override_raw = os.environ.get("MOLT_MIDEND_BUDGET_MS", "").strip()
    budget_override_ms: float | None = None
    if budget_override_raw:
        try:
            budget_override_ms = max(0.0, float(budget_override_raw))
        except ValueError:
            budget_override_ms = None
    hot_promotion_enabled = os.environ.get(
        "MOLT_MIDEND_HOT_TIER_PROMOTION", "1"
    ).strip().lower() not in {"0", "false", "no", "off"}

    def _float_env(name: str, default: float) -> float:
        raw = os.environ.get(name, "").strip()
        if not raw:
            return default
        try:
            return float(raw)
        except ValueError:
            return default

    return {
        "profile_override": profile_override or None,
        "hot_tier_promotion_enabled": hot_promotion_enabled,
        "budget_override_ms": budget_override_ms,
        "budget_alpha": _float_env("MOLT_MIDEND_BUDGET_ALPHA", 0.03),
        "budget_beta": _float_env("MOLT_MIDEND_BUDGET_BETA", 0.75),
        "budget_scale": _float_env("MOLT_MIDEND_BUDGET_SCALE", 1.0),
    }


def _duration_ms_from_ns(start_ns: Any, end_ns: Any) -> float:
    if not isinstance(start_ns, int):
        return 0.0
    if not isinstance(end_ns, int):
        return 0.0
    delta_ns = end_ns - start_ns
    if delta_ns <= 0:
        return 0.0
    return round(delta_ns / 1_000_000.0, 6)


def _normalize_midend_pass_stat(raw: dict[str, Any]) -> dict[str, Any]:
    samples = [
        float(sample)
        for sample in raw.get("samples_ms", [])
        if isinstance(sample, (int, float))
    ]
    ms_total = float(raw.get("ms_total", 0.0))
    ms_max = float(raw.get("ms_max", 0.0))
    return {
        "attempted": int(raw.get("attempted", 0)),
        "accepted": int(raw.get("accepted", 0)),
        "rejected": int(raw.get("rejected", 0)),
        "degraded": int(raw.get("degraded", 0)),
        "ms_total": round(max(0.0, ms_total), 6),
        "ms_max": round(max(0.0, ms_max), 6),
        "ms_p50": round(_midend_sample_percentile(samples, 0.50), 6),
        "ms_p75": round(_midend_sample_percentile(samples, 0.75), 6),
        "ms_p90": round(_midend_sample_percentile(samples, 0.90), 6),
        "ms_p95": round(_midend_sample_percentile(samples, 0.95), 6),
        "ms_p99": round(_midend_sample_percentile(samples, 0.99), 6),
        "sample_count": len(samples),
    }


def _build_midend_diagnostics_payload(
    *,
    requested_profile: BuildProfile,
    policy_outcomes_by_function: dict[str, dict[str, Any]],
    pass_stats_by_function: dict[str, dict[str, dict[str, Any]]],
) -> dict[str, Any] | None:
    if not policy_outcomes_by_function and not pass_stats_by_function:
        return None

    normalized_policy: dict[str, dict[str, Any]] = {}
    tier_summary: dict[str, int] = {}
    tier_base_summary: dict[str, int] = {}
    reason_summary: dict[str, int] = {}
    promotion_source_summary: dict[str, int] = {}
    effective_profiles: set[str] = set()
    degraded_functions = 0
    promoted_functions = 0
    function_hotspots: list[dict[str, Any]] = []
    degrade_event_hotspots: list[dict[str, Any]] = []
    promotion_hotspots: list[dict[str, Any]] = []

    for function_key in sorted(policy_outcomes_by_function):
        module_name, _, function_name = function_key.partition("::")
        raw_outcome = policy_outcomes_by_function[function_key]
        degrade_events: list[dict[str, Any]] = []
        for event in raw_outcome.get("degrade_events", []):
            if not isinstance(event, dict):
                continue
            reason = str(event.get("reason", ""))
            stage = str(event.get("stage", ""))
            action = str(event.get("action", ""))
            spent_ms = float(event.get("spent_ms", 0.0))
            normalized_event = {
                "reason": reason,
                "stage": stage,
                "action": action,
                "spent_ms": spent_ms,
            }
            if "value" in event:
                normalized_event["value"] = event["value"]
            degrade_events.append(normalized_event)
            degrade_event_hotspots.append(
                {
                    "module": module_name,
                    "function": function_name or module_name,
                    "reason": reason,
                    "stage": stage,
                    "action": action,
                    "spent_ms": round(max(0.0, spent_ms), 6),
                }
            )
            if reason:
                reason_summary[reason] = reason_summary.get(reason, 0) + 1
        profile = str(raw_outcome.get("profile", ""))
        tier = str(
            raw_outcome.get(
                "tier_effective",
                raw_outcome.get("tier", ""),
            )
        )
        tier_base = str(raw_outcome.get("tier_base", tier))
        tier_source = str(raw_outcome.get("tier_source", ""))
        promoted = bool(raw_outcome.get("promoted", False))
        promotion_source = str(raw_outcome.get("promotion_source", ""))
        promotion_signal = str(raw_outcome.get("promotion_signal", ""))
        if profile:
            effective_profiles.add(profile)
        if tier:
            tier_summary[tier] = tier_summary.get(tier, 0) + 1
        if tier_base:
            tier_base_summary[tier_base] = tier_base_summary.get(tier_base, 0) + 1
        degraded = bool(raw_outcome.get("degraded", False))
        if degraded:
            degraded_functions += 1
        if promoted:
            promoted_functions += 1
            if promotion_source:
                promotion_source_summary[promotion_source] = (
                    promotion_source_summary.get(promotion_source, 0) + 1
                )
        spent_ms = float(raw_outcome.get("spent_ms", 0.0))
        budget_ms = float(raw_outcome.get("budget_ms", 0.0))
        function_hotspots.append(
            {
                "module": module_name,
                "function": function_name or module_name,
                "profile": profile,
                "tier": tier,
                "tier_base": tier_base,
                "spent_ms": round(max(0.0, spent_ms), 6),
                "budget_ms": round(max(0.0, budget_ms), 6),
                "degraded": degraded,
                "promoted": promoted,
            }
        )
        if promoted:
            promotion_hotspots.append(
                {
                    "module": module_name,
                    "function": function_name or module_name,
                    "tier_base": tier_base,
                    "tier_effective": tier,
                    "source": promotion_source,
                    "signal": promotion_signal,
                    "spent_ms": round(max(0.0, spent_ms), 6),
                }
            )
        normalized_policy[function_key] = {
            "profile": profile,
            "tier": tier,
            "tier_effective": tier,
            "tier_base": tier_base,
            "tier_source": tier_source,
            "promoted": promoted,
            "promotion_source": promotion_source,
            "promotion_signal": promotion_signal,
            "budget_ms": budget_ms,
            "spent_ms": spent_ms,
            "degraded": degraded,
            "degrade_events": degrade_events,
        }

    normalized_pass_stats: dict[str, dict[str, dict[str, Any]]] = {}
    hotspots: list[dict[str, Any]] = []
    for function_key in sorted(pass_stats_by_function):
        module_name, _, function_name = function_key.partition("::")
        per_pass = pass_stats_by_function[function_key]
        normalized_per_pass: dict[str, dict[str, Any]] = {}
        for pass_name in sorted(per_pass):
            normalized = _normalize_midend_pass_stat(per_pass[pass_name])
            normalized_per_pass[pass_name] = normalized
            hotspots.append(
                {
                    "module": module_name,
                    "function": function_name or module_name,
                    "pass": pass_name,
                    "ms_total": normalized["ms_total"],
                    "ms_p95": normalized["ms_p95"],
                    "attempted": normalized["attempted"],
                    "accepted": normalized["accepted"],
                    "degraded": normalized["degraded"],
                }
            )
        normalized_pass_stats[function_key] = normalized_per_pass

    hotspots.sort(
        key=lambda item: (
            -float(item["ms_total"]),
            item["module"],
            item["function"],
            item["pass"],
        )
    )
    p95_hotspots = sorted(
        hotspots,
        key=lambda item: (
            -float(item["ms_p95"]),
            item["module"],
            item["function"],
            item["pass"],
        ),
    )
    function_hotspots.sort(
        key=lambda item: (
            -float(item["spent_ms"]),
            item["module"],
            item["function"],
        )
    )
    degrade_event_hotspots.sort(
        key=lambda item: (
            -float(item["spent_ms"]),
            item["module"],
            item["function"],
            item["reason"],
            item["action"],
        )
    )
    promotion_hotspots.sort(
        key=lambda item: (
            -float(item["spent_ms"]),
            item["module"],
            item["function"],
            item["tier_base"],
            item["tier_effective"],
        )
    )

    promotion_candidates: list[dict[str, Any]] = []
    budget_utilizations: list[float] = []
    functions_over_budget = 0
    functions_under_50pct_budget = 0
    for function_key in sorted(policy_outcomes_by_function):
        raw_outcome = policy_outcomes_by_function[function_key]
        module_name, _, function_name = function_key.partition("::")
        allow_hot = bool(raw_outcome.get("allow_hot_promotion", False))
        was_promoted = bool(raw_outcome.get("promoted", False))
        if allow_hot and not was_promoted:
            promotion_candidates.append(
                {
                    "module": module_name,
                    "function": function_name or module_name,
                    "tier": str(raw_outcome.get("tier", "")),
                    "budget_ms": round(
                        max(0.0, float(raw_outcome.get("budget_ms", 0.0))), 6
                    ),
                    "spent_ms": round(
                        max(0.0, float(raw_outcome.get("spent_ms", 0.0))), 6
                    ),
                }
            )
        s_ms = max(0.0, float(raw_outcome.get("spent_ms", 0.0)))
        b_ms = max(0.0, float(raw_outcome.get("budget_ms", 0.0)))
        if b_ms > 0.0:
            utilization = s_ms / b_ms
            budget_utilizations.append(utilization)
            if s_ms > b_ms:
                functions_over_budget += 1
            if s_ms < 0.5 * b_ms:
                functions_under_50pct_budget += 1
    promotion_candidates.sort(
        key=lambda item: (
            -float(item["spent_ms"]),
            item["module"],
            item["function"],
        )
    )
    budget_utilization_avg = 0.0
    budget_utilization_p95 = 0.0
    if budget_utilizations:
        budget_utilization_avg = sum(budget_utilizations) / len(budget_utilizations)
        budget_utilization_p95 = _midend_sample_percentile(budget_utilizations, 0.95)

    pass_aggregate_wall_ms: dict[str, float] = {}
    for function_key in pass_stats_by_function:
        per_pass = pass_stats_by_function[function_key]
        for pass_name, raw_stat in per_pass.items():
            ms_total = float(raw_stat.get("ms_total", 0.0))
            pass_aggregate_wall_ms[pass_name] = pass_aggregate_wall_ms.get(
                pass_name, 0.0
            ) + max(0.0, ms_total)
    pass_wall_ranked = sorted(pass_aggregate_wall_ms.items(), key=lambda kv: -kv[1])

    return {
        "requested_profile": requested_profile,
        "effective_profiles": sorted(effective_profiles),
        "policy_config": _midend_policy_config_snapshot(),
        "function_count": max(
            len(normalized_policy),
            len(normalized_pass_stats),
        ),
        "degraded_functions": degraded_functions,
        "promoted_functions": promoted_functions,
        "tier_summary": {name: tier_summary[name] for name in sorted(tier_summary)},
        "tier_base_summary": {
            name: tier_base_summary[name] for name in sorted(tier_base_summary)
        },
        "promotion_source_summary": {
            name: promotion_source_summary[name]
            for name in sorted(promotion_source_summary)
        },
        "degrade_reason_summary": {
            name: reason_summary[name] for name in sorted(reason_summary)
        },
        "budget_utilization_avg": round(budget_utilization_avg, 6),
        "budget_utilization_p95": round(budget_utilization_p95, 6),
        "functions_over_budget": functions_over_budget,
        "functions_under_50pct_budget": functions_under_50pct_budget,
        "promotion_candidates": promotion_candidates[:20],
        "pass_wall_time_ranked": [
            {"pass": name, "ms_total": round(ms, 6)} for name, ms in pass_wall_ranked
        ],
        "policy_outcomes_by_function": normalized_policy,
        "pass_stats_by_function": normalized_pass_stats,
        "function_hotspots_top": function_hotspots[:10],
        "promotion_hotspots_top": promotion_hotspots[:10],
        "degrade_event_hotspots_top": degrade_event_hotspots[:10],
        "pass_hotspots_top": hotspots[:10],
        "pass_hotspots_p95_top": p95_hotspots[:10],
    }


def _resolve_frontend_parallel_module_workers() -> int:
    raw = os.environ.get("MOLT_FRONTEND_PARALLEL_MODULES", "").strip().lower()
    if not raw:
        return 0
    if raw in {"0", "false", "no", "off"}:
        return 0
    if raw in {"auto", "1", "true", "yes", "on"}:
        cpu_count = os.cpu_count() or 1
        return max(2, cpu_count)
    try:
        parsed = int(raw)
    except ValueError:
        return 0
    if parsed < 2:
        return 0
    return parsed


def _resolve_frontend_parallel_min_modules() -> int:
    raw = os.environ.get("MOLT_FRONTEND_PARALLEL_MIN_MODULES", "").strip()
    if not raw:
        return 2
    try:
        parsed = int(raw)
    except ValueError:
        return 2
    return max(2, parsed)


def _resolve_frontend_parallel_min_predicted_cost() -> float:
    raw = os.environ.get("MOLT_FRONTEND_PARALLEL_MIN_PREDICTED_COST", "").strip()
    if not raw:
        return 32768.0
    try:
        parsed = float(raw)
    except ValueError:
        return 32768.0
    return max(0.0, parsed)


def _resolve_frontend_parallel_target_cost_per_worker() -> float:
    raw = os.environ.get("MOLT_FRONTEND_PARALLEL_TARGET_COST_PER_WORKER", "").strip()
    if not raw:
        return 65536.0
    try:
        parsed = float(raw)
    except ValueError:
        return 65536.0
    return max(1.0, parsed)


def _resolve_frontend_parallel_stdlib_min_cost_scale() -> float:
    raw = os.environ.get("MOLT_FRONTEND_PARALLEL_STDLIB_MIN_COST_SCALE", "").strip()
    if not raw:
        return 0.5
    try:
        parsed = float(raw)
    except ValueError:
        return 0.5
    return max(0.0, parsed)


def _looks_like_stdlib_module_name(module_name: str) -> bool:
    if module_name == "molt.stdlib" or module_name.startswith("molt.stdlib."):
        return True
    root = module_name.split(".", 1)[0]
    return root in {
        "__future__",
        "_collections_abc",
        "abc",
        "builtins",
        "collections",
        "dataclasses",
        "importlib",
        "os",
        "pathlib",
        "runpy",
        "signal",
        "sys",
        "test",
        "typing",
        "warnings",
        "zipfile",
        "zipimport",
    }


def _predict_frontend_module_cost(
    module_name: str,
    module_sources: dict[str, str],
    module_deps: dict[str, set[str]],
) -> float:
    source = module_sources.get(module_name, "")
    source_cost = max(1.0, float(len(source)))
    dep_cost = float(max(0, len(module_deps.get(module_name, set()))) * 512)
    return source_cost + dep_cost


def _build_frontend_module_costs(
    module_names: Collection[str],
    *,
    module_sources: Mapping[str, str],
    module_deps: Mapping[str, set[str]],
) -> dict[str, float]:
    module_costs: dict[str, float] = {}
    for module_name in sorted(module_names):
        source = module_sources.get(module_name, "")
        source_cost = max(1.0, float(len(source)))
        dep_cost = float(max(0, len(module_deps.get(module_name, set()))) * 512)
        module_costs[module_name] = source_cost + dep_cost
    return module_costs


def _build_stdlib_like_module_flags(
    module_names: Collection[str],
) -> dict[str, bool]:
    return {
        module_name: _looks_like_stdlib_module_name(module_name)
        for module_name in sorted(module_names)
    }


def _build_module_graph_metadata(
    module_graph: Mapping[str, Path],
    *,
    generated_module_source_paths: Mapping[str, str],
    entry_module: str,
    namespace_module_names: Collection[str],
    module_sources: Mapping[str, str] | None = None,
    module_deps: Mapping[str, set[str]] | None = None,
) -> _ModuleGraphMetadata:
    (
        logical_source_path_by_module,
        entry_override_by_module,
        module_is_namespace_by_module,
        module_is_package_by_module,
    ) = _build_module_lowering_metadata(
        module_graph,
        generated_module_source_paths=generated_module_source_paths,
        entry_module=entry_module,
        namespace_module_names=namespace_module_names,
    )
    frontend_module_costs = None
    if module_sources is not None and module_deps is not None:
        frontend_module_costs = _build_frontend_module_costs(
            module_graph,
            module_sources=module_sources,
            module_deps=module_deps,
        )
    stdlib_like_by_module = (
        _build_stdlib_like_module_flags(module_graph)
        if module_deps is not None
        else None
    )
    return _ModuleGraphMetadata(
        logical_source_path_by_module=logical_source_path_by_module,
        entry_override_by_module=entry_override_by_module,
        module_is_namespace_by_module=module_is_namespace_by_module,
        module_is_package_by_module=module_is_package_by_module,
        frontend_module_costs=frontend_module_costs,
        stdlib_like_by_module=stdlib_like_by_module,
    )


def _module_lowering_metadata_view(
    module_name: str,
    *,
    module_path: Path,
    module_graph_metadata: _ModuleGraphMetadata,
    path_stat_by_module: Mapping[str, os.stat_result | None] | None = None,
) -> _ModuleLoweringMetadataView:
    return _ModuleLoweringMetadataView(
        logical_source_path=module_graph_metadata.logical_source_path_by_module[
            module_name
        ],
        entry_override=module_graph_metadata.entry_override_by_module[module_name],
        module_is_namespace=module_graph_metadata.module_is_namespace_by_module[
            module_name
        ],
        is_package=module_graph_metadata.module_is_package_by_module[module_name],
        path_stat=(
            path_stat_by_module[module_name]
            if path_stat_by_module is not None
            else None
        ),
    )


def _module_lowering_execution_view(
    module_name: str,
    *,
    module_path: Path,
    module_graph_metadata: _ModuleGraphMetadata,
    module_deps: dict[str, set[str]],
    known_modules: Collection[str],
    known_func_defaults: dict[str, dict[str, Any]],
    pgo_hot_function_names: Collection[str],
    type_facts: TypeFacts | None,
    known_classes_snapshot: dict[str, Any],
    module_dep_closures: dict[str, frozenset[str]],
    path_stat_by_module: Mapping[str, os.stat_result | None] | None = None,
    scoped_lowering_inputs: _ScopedLoweringInputs | None = None,
    known_modules_sorted: tuple[str, ...] | None = None,
    pgo_hot_function_names_sorted: tuple[str, ...] | None = None,
    scoped_known_classes_by_module: Mapping[str, dict[str, Any]] | None = None,
) -> _ModuleLoweringExecutionView:
    metadata = _module_lowering_metadata_view(
        module_name,
        module_path=module_path,
        module_graph_metadata=module_graph_metadata,
        path_stat_by_module=path_stat_by_module,
    )
    scoped_inputs = _scoped_lowering_input_view(
        module_name,
        module_deps=module_deps,
        known_modules=known_modules,
        known_func_defaults=known_func_defaults,
        pgo_hot_function_names=pgo_hot_function_names,
        type_facts=type_facts,
        module_dep_closures=module_dep_closures,
        scoped_lowering_inputs=scoped_lowering_inputs,
        known_modules_sorted=known_modules_sorted,
        pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
    )
    scoped_known_classes = _scoped_known_classes_view(
        module_name,
        module_deps=module_deps,
        known_classes_snapshot=known_classes_snapshot,
        module_dep_closures=module_dep_closures,
        scoped_known_classes_by_module=scoped_known_classes_by_module,
    )
    return _ModuleLoweringExecutionView(
        metadata=metadata,
        scoped_inputs=scoped_inputs,
        scoped_known_classes=scoped_known_classes,
    )


def _choose_frontend_parallel_layer_workers(
    *,
    candidates: list[str],
    module_sources: dict[str, str],
    module_deps: dict[str, set[str]],
    module_costs: Mapping[str, float] | None = None,
    stdlib_like_by_module: Mapping[str, bool] | None = None,
    max_workers: int,
    min_modules: int,
    min_predicted_cost: float,
    target_cost_per_worker: float,
) -> dict[str, Any]:
    candidate_count = len(candidates)
    if candidate_count < min_modules:
        return {
            "enabled": False,
            "workers": 1,
            "reason": "layer_module_count_below_min",
            "predicted_cost_total": 0.0,
            "effective_min_predicted_cost": round(min_predicted_cost, 3),
            "stdlib_candidates": 0,
        }
    predicted_cost_total = 0.0
    for name in candidates:
        if module_costs is not None and name in module_costs:
            predicted_cost_total += module_costs[name]
        else:
            predicted_cost_total += _predict_frontend_module_cost(
                name, module_sources, module_deps
            )
    stdlib_candidates = sum(
        1
        for name in candidates
        if (
            stdlib_like_by_module[name]
            if stdlib_like_by_module is not None and name in stdlib_like_by_module
            else _looks_like_stdlib_module_name(name)
        )
    )
    effective_min_predicted_cost = float(min_predicted_cost)
    if stdlib_candidates > 0:
        effective_min_predicted_cost *= (
            _resolve_frontend_parallel_stdlib_min_cost_scale()
        )
    if predicted_cost_total < effective_min_predicted_cost:
        return {
            "enabled": False,
            "workers": 1,
            "reason": "layer_predicted_cost_below_min",
            "predicted_cost_total": round(predicted_cost_total, 3),
            "effective_min_predicted_cost": round(effective_min_predicted_cost, 3),
            "stdlib_candidates": stdlib_candidates,
        }
    scaled_workers = int(
        (predicted_cost_total / max(1.0, target_cost_per_worker)) + 0.999
    )
    chosen_workers = min(
        max_workers,
        candidate_count,
        max(2, scaled_workers),
    )
    return {
        "enabled": chosen_workers >= 2,
        "workers": max(1, chosen_workers),
        "reason": "enabled",
        "predicted_cost_total": round(predicted_cost_total, 3),
        "effective_min_predicted_cost": round(effective_min_predicted_cost, 3),
        "stdlib_candidates": stdlib_candidates,
    }


def _module_dependency_layers(
    module_order: list[str],
    module_deps: dict[str, set[str]],
) -> list[list[str]]:
    if not module_order:
        return []
    depth_by_module: dict[str, int] = {}
    for name in module_order:
        deps = [
            dep
            for dep in module_deps.get(name, set())
            if dep in depth_by_module and dep != name
        ]
        if not deps:
            depth_by_module[name] = 0
            continue
        depth_by_module[name] = max(depth_by_module[dep] for dep in deps) + 1
    grouped: dict[int, list[str]] = {}
    for name in module_order:
        grouped.setdefault(depth_by_module.get(name, 0), []).append(name)
    return [grouped[level] for level in sorted(grouped)]


def _module_order_has_back_edges(
    module_order: list[str],
    module_deps: dict[str, set[str]],
) -> bool:
    seen: set[str] = set()
    module_set = set(module_order)
    for name in module_order:
        deps = {dep for dep in module_deps.get(name, set()) if dep in module_set}
        if not deps.issubset(seen):
            return True
        seen.add(name)
    return False


def _frontend_lower_module_worker(payload: dict[str, Any]) -> dict[str, Any]:
    worker_started_ns = time.time_ns()
    worker_pid = os.getpid()
    module_name = str(payload["module_name"])
    module_path = str(payload["module_path"])
    logical_source_path = str(payload.get("logical_source_path") or module_path)
    source = str(payload["source"])
    parse_codec = cast(ParseCodec, payload["parse_codec"])
    type_hint_policy = cast(TypeHintPolicy, payload["type_hint_policy"])
    fallback_policy = cast(FallbackPolicy, payload["fallback_policy"])
    module_is_namespace = bool(payload["module_is_namespace"])
    entry_module = cast(str | None, payload["entry_module"])
    enable_phi = bool(payload["enable_phi"])
    known_modules = set(cast(list[str], payload["known_modules"]))
    known_classes = cast(dict[str, Any], payload["known_classes"])
    stdlib_allowlist = set(cast(list[str], payload["stdlib_allowlist"]))
    known_func_defaults = cast(
        dict[str, dict[str, dict[str, Any]]], payload["known_func_defaults"]
    )
    module_chunking = bool(payload["module_chunking"])
    module_chunk_max_ops = int(payload["module_chunk_max_ops"])
    optimization_profile = cast(BuildProfile, payload["optimization_profile"])
    pgo_hot_functions = {
        symbol.strip()
        for symbol in cast(list[str], payload.get("pgo_hot_functions", []))
        if isinstance(symbol, str) and symbol.strip()
    }

    module_frontend_start = time.perf_counter()
    visit_s = 0.0
    lower_s = 0.0
    try:
        tree = ast.parse(source, filename=logical_source_path)
    except SyntaxError as exc:
        worker_finished_ns = time.time_ns()
        return {
            "ok": False,
            "error": f"Syntax error in {module_path}: {exc}",
            "timings": {
                "visit_s": visit_s,
                "lower_s": lower_s,
                "total_s": time.perf_counter() - module_frontend_start,
            },
            "worker": {
                "pid": worker_pid,
                "started_ns": worker_started_ns,
                "finished_ns": worker_finished_ns,
            },
        }
    gen = SimpleTIRGenerator(
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
        source_path=logical_source_path,
        module_name=module_name,
        module_is_namespace=module_is_namespace,
        entry_module=entry_module,
        enable_phi=enable_phi,
        known_modules=known_modules,
        known_classes=known_classes,
        stdlib_allowlist=stdlib_allowlist,
        known_func_defaults=known_func_defaults,
        module_chunking=module_chunking,
        module_chunk_max_ops=module_chunk_max_ops,
        optimization_profile=optimization_profile,
        pgo_hot_functions=pgo_hot_functions,
    )
    try:
        visit_start = time.perf_counter()
        gen.visit(tree)
        visit_s = time.perf_counter() - visit_start
        lower_start = time.perf_counter()
        ir = gen.to_json()
        lower_s = time.perf_counter() - lower_start
    except CompatibilityError as exc:
        worker_finished_ns = time.time_ns()
        return {
            "ok": False,
            "error": str(exc),
            "timings": {
                "visit_s": visit_s,
                "lower_s": lower_s,
                "total_s": time.perf_counter() - module_frontend_start,
            },
            "worker": {
                "pid": worker_pid,
                "started_ns": worker_started_ns,
                "finished_ns": worker_finished_ns,
            },
        }
    worker_finished_ns = time.time_ns()
    return {
        "ok": True,
        "functions": ir["functions"],
        "func_code_ids": dict(gen.func_code_ids),
        "local_class_names": sorted(gen.local_class_names),
        "local_classes": {
            class_name: gen.classes[class_name]
            for class_name in sorted(gen.local_class_names)
        },
        "midend_policy_outcomes_by_function": dict(
            gen.midend_policy_outcomes_by_function
        ),
        "midend_pass_stats_by_function": dict(gen.midend_pass_stats_by_function),
        "timings": {
            "visit_s": visit_s,
            "lower_s": lower_s,
            "total_s": time.perf_counter() - module_frontend_start,
        },
        "worker": {
            "pid": worker_pid,
            "started_ns": worker_started_ns,
            "finished_ns": worker_finished_ns,
        },
    }


def _requires_spawn_entry_override(
    module_graph: dict[str, Path], explicit_imports: set[str]
) -> bool:
    names: set[str] = set(module_graph)
    names.update(explicit_imports)
    for name in names:
        if name == ENTRY_OVERRIDE_SPAWN or name.startswith("multiprocessing."):
            return True
        if name == "multiprocessing":
            return True
    return False


def _discover_module_graph(
    entry_path: Path,
    roots: list[Path],
    module_roots: list[Path],
    stdlib_root: Path,
    project_root: Path | None,
    stdlib_allowlist: set[str],
    skip_modules: set[str] | None = None,
    stub_parents: set[str] | None = None,
    nested_stdlib_scan_modules: set[str] | None = None,
    resolver_cache: _ModuleResolutionCache | None = None,
) -> tuple[dict[str, Path], set[str]]:
    graph: dict[str, Path] = {}
    skip_modules = skip_modules or set()
    stub_parents = stub_parents or set()
    nested_stdlib_scan_modules = (
        STDLIB_NESTED_IMPORT_SCAN_MODULES
        if nested_stdlib_scan_modules is None
        else nested_stdlib_scan_modules
    )
    explicit_imports: set[str] = set()
    queue = [entry_path]
    queued_paths = {entry_path}
    resolution_cache = resolver_cache or _ModuleResolutionCache()

    persisted_graph_paths: dict[str, Path] = {}
    dirty_persisted_modules: set[str] = set()
    if project_root is not None:
        persisted_graph = _read_persisted_module_graph(
            project_root,
            entry_path,
            roots=roots,
            module_roots=module_roots,
            stdlib_root=stdlib_root,
            skip_modules=skip_modules,
            stub_parents=stub_parents,
            nested_stdlib_scan_modules=nested_stdlib_scan_modules,
            resolution_cache=resolution_cache,
        )
        if persisted_graph is not None:
            if not persisted_graph.dirty_modules:
                return persisted_graph.graph, persisted_graph.explicit_imports
            persisted_graph_paths = dict(persisted_graph.graph)
            dirty_persisted_modules = set(persisted_graph.dirty_modules)

    def resolve_candidate(candidate: str) -> Path | None:
        persisted_path = persisted_graph_paths.get(candidate)
        if persisted_path is not None and candidate not in dirty_persisted_modules:
            return persisted_path
        return resolution_cache.resolve_module(
            candidate, roots, stdlib_root, stdlib_allowlist
        )

    while queue:
        path = queue.pop()
        queued_paths.discard(path)
        module_name = resolution_cache.module_name_from_path(
            path, module_roots, stdlib_root
        )
        if module_name in graph:
            continue
        graph[module_name] = path
        is_package = path.name == "__init__.py"
        include_nested_imports = (
            not resolution_cache.is_stdlib_path(path, stdlib_root)
            or module_name in nested_stdlib_scan_modules
        )
        persisted_imports = None
        if project_root is not None:
            persisted_imports = _read_persisted_import_scan(
                project_root,
                path,
                module_name=module_name,
                is_package=is_package,
                include_nested=include_nested_imports,
            )
        if persisted_imports is None:
            try:
                source = resolution_cache.read_module_source(path)
            except (OSError, SyntaxError, UnicodeDecodeError):
                continue
            try:
                tree = resolution_cache.parse_module_ast(
                    path, source, filename=str(path)
                )
            except SyntaxError:
                continue
            imports = _load_module_imports(
                path,
                module_name=module_name,
                is_package=is_package,
                include_nested=include_nested_imports,
                tree=tree,
                resolution_cache=resolution_cache,
                project_root=project_root,
            )
        else:
            imports = persisted_imports
        for name in imports:
            explicit_imports.add(name)
            for candidate in _expand_module_chain_cached(name):
                if candidate in stub_parents:
                    continue
                if candidate.split(".", 1)[0] in skip_modules:
                    continue
                resolved = resolve_candidate(candidate)
                if resolved is not None and resolved not in queued_paths:
                    queued_paths.add(resolved)
                    queue.append(resolved)
    if project_root is not None:
        with contextlib.suppress(OSError):
            _write_persisted_module_graph(
                project_root,
                entry_path,
                roots=roots,
                module_roots=module_roots,
                stdlib_root=stdlib_root,
                skip_modules=skip_modules,
                stub_parents=stub_parents,
                nested_stdlib_scan_modules=nested_stdlib_scan_modules,
                graph=graph,
                explicit_imports=explicit_imports,
            )
    return graph, explicit_imports


def _latest_mtime(paths: list[Path]) -> float:
    latest = 0.0
    for path in paths:
        if path.is_dir():
            for item in path.rglob("*"):
                if item.is_file():
                    latest = max(latest, item.stat().st_mtime)
        elif path.exists():
            latest = max(latest, path.stat().st_mtime)
    return latest


@functools.lru_cache(maxsize=1)
def _rustc_version() -> str | None:
    try:
        result = subprocess.run(
            ["rustc", "-Vv"], capture_output=True, text=True, check=False
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    return result.stdout.strip()


@functools.lru_cache(maxsize=512)
def _resolved_artifact_hash_key(path_str: str) -> str:
    return hashlib.sha256(str(Path(path_str).resolve()).encode("utf-8")).hexdigest()[
        :16
    ]


@functools.lru_cache(maxsize=4096)
def _resolved_module_cache_key(path_str: str, *parts: str) -> str:
    return hashlib.sha256(
        "|".join((str(Path(path_str).resolve()), *parts)).encode("utf-8")
    ).hexdigest()[:24]


@functools.lru_cache(maxsize=1024)
def _module_graph_cache_key(
    entry_path: str,
    roots: tuple[str, ...],
    module_roots: tuple[str, ...],
    stdlib_root: str,
    skip_modules: tuple[str, ...],
    stub_parents: tuple[str, ...],
    nested_stdlib_scan_modules: tuple[str, ...],
) -> str:
    return hashlib.sha256(
        json.dumps(
            {
                "version": 1,
                "entry_path": str(Path(entry_path).resolve()),
                "roots": [str(Path(path).resolve()) for path in roots],
                "module_roots": [str(Path(path).resolve()) for path in module_roots],
                "stdlib_root": str(Path(stdlib_root).resolve()),
                "skip_modules": list(skip_modules),
                "stub_parents": list(stub_parents),
                "nested_stdlib_scan_modules": list(nested_stdlib_scan_modules),
            },
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()[:24]


def _runtime_fingerprint_path(
    project_root: Path,
    artifact: Path,
    cargo_profile: str,
    target_triple: str | None,
) -> Path:
    target = (target_triple or "native").replace(os.sep, "_").replace(":", "_")
    return _artifact_state_path(
        project_root,
        artifact,
        subdir="runtime_fingerprints",
        stem_suffix=f"{cargo_profile}.{target}",
        extension="fingerprint",
    )


@functools.lru_cache(maxsize=4096)
def _artifact_state_path_cached(
    build_state_root_str: str,
    artifact_path_str: str,
    artifact_name: str,
    subdir: str,
    stem_suffix: str,
    extension: str,
) -> Path:
    artifact_key = _resolved_artifact_hash_key(artifact_path_str)
    stem = (
        f"{artifact_name}.{stem_suffix}.{artifact_key}"
        if stem_suffix
        else f"{artifact_name}.{artifact_key}"
    )
    return Path(build_state_root_str) / subdir / f"{stem}.{extension}"


@functools.lru_cache(maxsize=512)
def _build_state_subdir_cached(build_state_root_str: str, subdir: str) -> Path:
    return Path(build_state_root_str) / subdir


def _artifact_state_path(
    project_root: Path,
    artifact: Path,
    *,
    subdir: str,
    stem_suffix: str,
    extension: str,
) -> Path:
    build_state_root = os.fspath(_build_state_root(project_root))
    return _artifact_state_path_cached(
        build_state_root,
        os.fspath(artifact),
        artifact.name,
        subdir,
        stem_suffix,
        extension,
    )


def _hash_runtime_file(path: Path, root: Path, hasher: Any) -> None:
    try:
        rel_path = path.relative_to(root)
        rel_bytes = str(rel_path).encode("utf-8")
    except ValueError:
        rel_bytes = str(path).encode("utf-8")
    hasher.update(rel_bytes)
    hasher.update(b"\0")
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(65536)
            if not chunk:
                break
            hasher.update(chunk)
    hasher.update(b"\0")


def _hash_source_tree_metadata(
    paths: list[Path],
    root: Path,
) -> tuple[str, int] | None:
    hasher = hashlib.sha256()
    file_count = 0
    try:
        for path in sorted(paths, key=lambda p: str(p)):
            if path.is_dir():
                for item in sorted(path.rglob("*"), key=lambda p: str(p)):
                    if not item.is_file():
                        continue
                    try:
                        stat = item.stat()
                    except OSError:
                        return None
                    try:
                        rel_path = item.relative_to(root)
                        rel_text = str(rel_path)
                    except ValueError:
                        rel_text = str(item)
                    hasher.update(rel_text.encode("utf-8"))
                    hasher.update(b"\0")
                    hasher.update(str(stat.st_size).encode("utf-8"))
                    hasher.update(b"\0")
                    hasher.update(str(stat.st_mtime_ns).encode("utf-8"))
                    hasher.update(b"\0")
                    file_count += 1
            elif path.exists():
                try:
                    stat = path.stat()
                except OSError:
                    return None
                try:
                    rel_path = path.relative_to(root)
                    rel_text = str(rel_path)
                except ValueError:
                    rel_text = str(path)
                hasher.update(rel_text.encode("utf-8"))
                hasher.update(b"\0")
                hasher.update(str(stat.st_size).encode("utf-8"))
                hasher.update(b"\0")
                hasher.update(str(stat.st_mtime_ns).encode("utf-8"))
                hasher.update(b"\0")
                file_count += 1
    except OSError:
        return None
    return hasher.hexdigest(), file_count


def _stored_fingerprint_matches_source_metadata(
    stored_fingerprint: dict[str, Any] | None,
    *,
    inputs_digest: str | None,
    rustc: str | None,
) -> bool:
    if stored_fingerprint is None or not inputs_digest:
        return False
    if stored_fingerprint.get("inputs_digest") != inputs_digest:
        return False
    if rustc:
        stored_rustc = stored_fingerprint.get("rustc")
        if stored_rustc is None or stored_rustc != rustc:
            return False
    return isinstance(stored_fingerprint.get("hash"), str) and bool(
        stored_fingerprint.get("hash")
    )


def _runtime_fingerprint(
    project_root: Path,
    *,
    cargo_profile: str,
    target_triple: str | None,
    rustflags: str,
    runtime_features: tuple[str, ...] = (),
    stored_fingerprint: dict[str, Any] | None = None,
) -> dict[str, str | None] | None:
    feature_list = tuple(_dedupe_preserve_order(sorted(runtime_features)))
    meta = f"profile:{cargo_profile}\ntarget:{target_triple or 'native'}\n"
    meta += f"rustflags:{rustflags}\n"
    meta += f"features:{','.join(feature_list)}\n"
    source_paths = _runtime_source_paths(project_root)
    rustc_info = _rustc_version()
    inputs_meta = _hash_source_tree_metadata(source_paths, project_root)
    inputs_digest = inputs_meta[0] if inputs_meta is not None else None
    if _stored_fingerprint_matches_source_metadata(
        stored_fingerprint,
        inputs_digest=inputs_digest,
        rustc=rustc_info,
    ):
        return {
            "hash": cast(str, stored_fingerprint.get("hash")),
            "rustc": rustc_info,
            "inputs_digest": inputs_digest,
        }

    hasher = hashlib.sha256()
    hasher.update(meta.encode("utf-8"))
    try:
        for path in sorted(source_paths, key=lambda p: str(p)):
            if path.is_dir():
                for item in sorted(path.rglob("*"), key=lambda p: str(p)):
                    if item.is_file():
                        _hash_runtime_file(item, project_root, hasher)
            elif path.exists():
                _hash_runtime_file(path, project_root, hasher)
    except OSError:
        return None
    return {
        "hash": hasher.hexdigest(),
        "rustc": rustc_info,
        "inputs_digest": inputs_digest,
    }


@functools.lru_cache(maxsize=32)
def _runtime_cargo_features_cached(
    target_triple: str | None,
    raw: str | None,
) -> tuple[str, ...]:
    if target_triple is not None and target_triple.startswith("wasm32"):
        return ()
    enabled = True if raw is None or raw.strip() == "" else _coerce_bool(raw, True)
    if not enabled:
        return ()
    return ("molt_tk_native",)


def _runtime_cargo_features(target_triple: str | None) -> tuple[str, ...]:
    return _runtime_cargo_features_cached(
        target_triple,
        os.environ.get("MOLT_RUNTIME_TK_NATIVE"),
    )


def _read_runtime_fingerprint(path: Path) -> dict[str, Any] | None:
    payload = _read_cached_json_object(path)
    if payload is not None:
        data = payload
    else:
        try:
            text = path.read_text().strip()
        except OSError:
            return None
        if not text:
            return None
        try:
            json.loads(text)
        except json.JSONDecodeError:
            return {"hash": text, "rustc": None, "inputs_digest": None}
        return None
    hash_value = data.get("hash")
    if not isinstance(hash_value, str) or not hash_value:
        return None
    rustc_value = data.get("rustc")
    inputs_digest = data.get("inputs_digest")
    if (rustc_value is None or isinstance(rustc_value, str)) and (
        inputs_digest is None or isinstance(inputs_digest, str)
    ):
        return data
    if rustc_value is not None and not isinstance(rustc_value, str):
        rustc_value = None
    if inputs_digest is not None and not isinstance(inputs_digest, str):
        inputs_digest = None
    return {"hash": hash_value, "rustc": rustc_value, "inputs_digest": inputs_digest}


def _write_runtime_fingerprint(path: Path, fingerprint: dict[str, str | None]) -> None:
    payload = {
        "version": 1,
        "hash": fingerprint.get("hash"),
        "rustc": fingerprint.get("rustc"),
        "inputs_digest": fingerprint.get("inputs_digest"),
    }
    _write_cached_json_object(path, payload)


def _write_text_if_changed(path: Path, content: str) -> None:
    try:
        existing = path.read_text()
    except OSError:
        existing = None
    if existing == content:
        return
    path.write_text(content)


def _check_lockfiles(
    project_root: Path,
    json_output: bool,
    warnings: list[str],
    deterministic: bool,
    deterministic_warn: bool,
    command: str,
) -> int | None:
    pyproject = project_root / "pyproject.toml"
    if not pyproject.exists():
        return None
    lock_path = project_root / "uv.lock"
    cargo_lock = project_root / "Cargo.lock"
    missing = []
    if not lock_path.exists():
        missing.append("uv.lock")
    if not cargo_lock.exists():
        missing.append("Cargo.lock")
    if missing and deterministic:
        missing_text = ", ".join(missing)
        message = (
            f"Missing lockfiles ({missing_text}); run `uv lock` and ensure Cargo.lock."
        )
        if deterministic_warn:
            warnings.append(message)
        else:
            return _fail(message, json_output, command=command)
    if missing:
        warnings.append(f"Missing lockfiles: {', '.join(missing)}")
        return None
    if deterministic:
        skip_uv_lock = os.environ.get("UV_NO_SYNC") == "1"
        if skip_uv_lock:
            warnings.append("Skipping uv.lock check because UV_NO_SYNC=1.")
        else:
            uv_error = _verify_uv_lock(project_root)
            if uv_error is not None:
                if deterministic_warn:
                    warnings.append(uv_error)
                else:
                    return _fail(uv_error, json_output, command=command)
        skip_cargo_lock = os.environ.get("MOLT_SKIP_CARGO_LOCK") == "1"
        if skip_cargo_lock:
            warnings.append("Skipping Cargo.lock check because MOLT_SKIP_CARGO_LOCK=1.")
        else:
            cargo_error = _verify_cargo_lock(project_root)
            if cargo_error is not None:
                if deterministic_warn:
                    warnings.append(cargo_error)
                else:
                    return _fail(cargo_error, json_output, command=command)
    return None


@functools.lru_cache(maxsize=256)
def _lock_check_cache_path_cached(
    project_root_str: str,
    name: str,
    cargo_target_override: str | None,
) -> Path:
    # The lock-check cache can grow (especially for Cargo metadata inputs).
    # Keep it colocated with Cargo build outputs when CARGO_TARGET_DIR is set so
    # developers can move all large artifacts onto an external volume.
    project_root = Path(project_root_str)
    if cargo_target_override:
        target_dir = Path(cargo_target_override)
        if not target_dir.is_absolute():
            target_dir = project_root / target_dir
    else:
        target_dir = project_root / "target"
    return target_dir / "lock_checks" / f"{name}.json"


def _lock_check_cache_path(project_root: Path, name: str) -> Path:
    return _lock_check_cache_path_cached(
        os.fspath(project_root),
        name,
        os.environ.get("CARGO_TARGET_DIR"),
    )


def _lock_check_inputs(
    project_root: Path, paths: list[Path]
) -> dict[str, dict[str, int]] | None:
    project_root_resolved = project_root.resolve()
    payload: dict[str, dict[str, int]] = {}
    for path in paths:
        try:
            stat = path.stat()
            resolved = path.resolve()
        except OSError:
            return None
        try:
            key = str(resolved.relative_to(project_root_resolved))
        except ValueError:
            key = str(resolved)
        payload[key] = {"mtime_ns": stat.st_mtime_ns, "size": stat.st_size}
    return {name: payload[name] for name in sorted(payload)}


def _load_lock_check_cache(path: Path) -> dict[str, Any] | None:
    try:
        data = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(data, dict):
        return None
    return data


def _is_lock_check_cache_valid(
    project_root: Path, name: str, inputs: dict[str, dict[str, int]] | None
) -> bool:
    if not inputs:
        return False
    payload = _load_lock_check_cache(_lock_check_cache_path(project_root, name))
    if payload is None:
        return False
    if payload.get("version") != _LOCK_CHECK_CACHE_VERSION:
        return False
    if payload.get("ok") is not True:
        return False
    return payload.get("inputs") == inputs


def _write_lock_check_cache(
    project_root: Path, name: str, inputs: dict[str, dict[str, int]] | None
) -> None:
    if not inputs:
        return
    path = _lock_check_cache_path(project_root, name)
    payload = {
        "version": _LOCK_CHECK_CACHE_VERSION,
        "ok": True,
        "checked_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "inputs": inputs,
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = path.with_suffix(path.suffix + ".tmp")
    tmp_path.write_text(json.dumps(payload, sort_keys=True) + "\n")
    tmp_path.replace(path)


def _verify_uv_lock(project_root: Path) -> str | None:
    if shutil.which("uv") is None:
        return "Deterministic builds require uv; install uv to validate uv.lock."
    inputs = _lock_check_inputs(
        project_root,
        [project_root / "pyproject.toml", project_root / "uv.lock"],
    )
    if _is_lock_check_cache_valid(project_root, "uv", inputs):
        return None
    try:
        result = subprocess.run(
            ["uv", "lock", "--check"],
            cwd=project_root,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        return f"Failed to run `uv lock --check`: {exc}"
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip() or "uv lock check failed"
        return f"uv.lock is out of date or invalid: {detail}"
    _write_lock_check_cache(project_root, "uv", inputs)
    return None


def _verify_cargo_lock(project_root: Path) -> str | None:
    if shutil.which("cargo") is None:
        return "Deterministic builds require cargo; install Rust toolchain to validate Cargo.lock."
    cargo_inputs = sorted(
        path
        for path in project_root.rglob("Cargo.toml")
        if "target" not in path.parts and ".git" not in path.parts
    )
    cargo_inputs.append(project_root / "Cargo.lock")
    inputs = _lock_check_inputs(project_root, cargo_inputs)
    if _is_lock_check_cache_valid(project_root, "cargo", inputs):
        return None
    try:
        result = subprocess.run(
            ["cargo", "metadata", "--locked", "--format-version", "1"],
            cwd=project_root,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        return f"Failed to run `cargo metadata --locked`: {exc}"
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip() or "cargo metadata failed"
        return f"Cargo.lock is out of date or invalid: {detail}"
    _write_lock_check_cache(project_root, "cargo", inputs)
    return None


def _artifact_needs_rebuild(
    artifact: Path,
    fingerprint: dict[str, str | None] | None,
    stored_fingerprint: dict[str, str | None] | None,
) -> bool:
    try:
        artifact.stat()
    except OSError:
        return True
    if fingerprint is None or stored_fingerprint is None:
        return True
    if stored_fingerprint.get("hash") != fingerprint.get("hash"):
        return True
    rustc = fingerprint.get("rustc")
    if rustc:
        stored_rustc = stored_fingerprint.get("rustc")
        return stored_rustc is None or stored_rustc != rustc
    return False


def _is_valid_wasm_binary(path: Path) -> bool:
    return _inspect_wasm_binary(path) == "valid"


def _inspect_wasm_binary(path: Path) -> Literal["missing", "invalid", "valid"]:
    try:
        with path.open("rb") as handle:
            magic = handle.read(8)
    except OSError:
        return "missing"
    if magic != b"\x00asm\x01\x00\x00\x00":
        return "invalid"
    return "valid"


def _is_valid_cached_backend_artifact(path: Path, *, is_wasm: bool) -> bool:
    if is_wasm:
        return _is_valid_wasm_binary(path)
    try:
        return path.stat().st_size > 0
    except OSError:
        return False


def _maybe_enable_sccache(env: dict[str, str]) -> None:
    if env.get("RUSTC_WRAPPER"):
        return
    mode = env.get("MOLT_USE_SCCACHE", "auto").strip().lower()
    if mode in {"0", "false", "no", "off"}:
        return
    sccache = shutil.which("sccache")
    if sccache is None:
        return
    env["RUSTC_WRAPPER"] = sccache


def _is_sccache_wrapper_failure(result: subprocess.CompletedProcess[str]) -> bool:
    stderr = result.stderr or ""
    stdout = result.stdout or ""
    combined = f"{stderr}\n{stdout}"
    return "sccache: error:" in combined or (
        "process didn't exit successfully" in combined and "sccache" in combined
    )


def _run_cargo_with_sccache_retry(
    cmd: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout: float | None,
    json_output: bool,
    label: str,
) -> subprocess.CompletedProcess[str]:
    build = subprocess.run(
        cmd,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    wrapper = env.get("RUSTC_WRAPPER", "")
    if (
        build.returncode != 0
        and wrapper
        and Path(wrapper).name == "sccache"
        and _is_sccache_wrapper_failure(build)
    ):
        retry_env = env.copy()
        retry_env.pop("RUSTC_WRAPPER", None)
        if not json_output:
            print(
                f"{label}: sccache wrapper failure detected; retrying without sccache.",
                file=sys.stderr,
            )
        build = subprocess.run(
            cmd,
            cwd=cwd,
            env=retry_env,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    return build


@functools.lru_cache(maxsize=256)
def _build_lock_dir_cached(project_root_str: str, build_state_root_str: str) -> Path:
    return Path(build_state_root_str) / "build_locks"


@contextmanager
def _build_lock(project_root: Path, name: str):
    if os.name != "posix":
        yield
        return
    try:
        import fcntl  # type: ignore
    except Exception:
        yield
        return
    lock_dir = _build_lock_dir_cached(
        os.fspath(project_root),
        os.fspath(_build_state_root(project_root)),
    )
    lock_dir.mkdir(parents=True, exist_ok=True)
    lock_path = lock_dir / f"{name}.lock"
    fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o666)
    timeout_raw = os.environ.get("MOLT_BUILD_LOCK_TIMEOUT", "").strip()
    lock_timeout: float | None = 300.0
    if timeout_raw:
        try:
            parsed = float(timeout_raw)
        except ValueError:
            parsed = 300.0
        lock_timeout = parsed if parsed > 0 else None
    try:
        deadline = time.monotonic() + lock_timeout if lock_timeout is not None else None
        while True:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except OSError as exc:
                if exc.errno not in (errno.EACCES, errno.EAGAIN):
                    raise
                if deadline is not None and time.monotonic() >= deadline:
                    raise RuntimeError(
                        "Timed out waiting for build lock "
                        f"{lock_path} after {lock_timeout:.1f}s. "
                        "Check for stale molt build/backend helper processes."
                    ) from exc
                time.sleep(0.05)
        yield
    finally:
        try:
            fcntl.flock(fd, fcntl.LOCK_UN)
        except OSError:
            pass
        os.close(fd)


def _load_molt_config(project_root: Path) -> dict[str, Any]:
    config: dict[str, Any] = {}
    molt_toml = project_root / "molt.toml"
    if molt_toml.exists():
        try:
            config.update(tomllib.loads(molt_toml.read_text()))
        except (OSError, tomllib.TOMLDecodeError):
            pass
    pyproject = project_root / "pyproject.toml"
    if pyproject.exists():
        try:
            data = tomllib.loads(pyproject.read_text())
        except (OSError, tomllib.TOMLDecodeError):
            data = {}
        tool_cfg = data.get("tool", {}).get("molt", {})
        if tool_cfg:
            config.setdefault("tool", {})
            config["tool"].setdefault("molt", {})
            config["tool"]["molt"].update(tool_cfg)
    return config


def _config_value(config: dict[str, Any], path: list[str]) -> Any | None:
    current: Any = config
    for key in path:
        if not isinstance(current, dict) or key not in current:
            return None
        current = current[key]
    return current


def _resolve_command_config(config: dict[str, Any], command: str) -> dict[str, Any]:
    cmd_cfg: dict[str, Any] = {}
    direct = _config_value(config, [command])
    if isinstance(direct, dict):
        cmd_cfg.update(direct)
    tool_cfg = _config_value(config, ["tool", "molt", command])
    if isinstance(tool_cfg, dict):
        cmd_cfg.update(tool_cfg)
    return cmd_cfg


def _resolve_build_config(config: dict[str, Any]) -> dict[str, Any]:
    return _resolve_command_config(config, "build")


def _resolve_capabilities_config(config: dict[str, Any]) -> CapabilityInput | None:
    for path in (["capabilities"], ["tool", "molt", "capabilities"]):
        caps = _config_value(config, path)
        if isinstance(caps, (list, str, dict)):
            return caps
    return None


def _coerce_bool(value: Any, default: bool) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        return value.strip().lower() in {"1", "true", "yes", "on"}
    return default


def _merge_optional_list(
    left: list[str] | None, right: list[str] | None
) -> list[str] | None:
    if left is None:
        return right
    if right is None:
        return left
    return _dedupe_preserve_order([*left, *right])


def _coerce_token_list(
    value: Any, field: str, errors: list[str]
) -> tuple[list[str], bool]:
    if value is None:
        return [], False
    if isinstance(value, list):
        tokens: list[str] = []
        for entry in value:
            if isinstance(entry, str):
                stripped = entry.strip()
                if stripped:
                    tokens.append(stripped)
            else:
                errors.append(f"{field} entries must be strings")
        return tokens, True
    if isinstance(value, str):
        return _split_tokens(value), True
    errors.append(f"{field} must be a list or string")
    return [], True


def _coerce_effects_list(
    value: Any, field: str, errors: list[str]
) -> tuple[list[str], bool]:
    if value is None:
        return [], False
    if isinstance(value, list):
        effects: list[str] = []
        for entry in value:
            if isinstance(entry, str):
                stripped = entry.strip()
                if stripped:
                    effects.append(stripped)
            else:
                errors.append(f"{field} entries must be strings")
        return effects, True
    if isinstance(value, str):
        return _split_tokens(value), True
    errors.append(f"{field} must be a list or string")
    return [], True


def _fs_entry_enabled(value: Any, field: str, errors: list[str]) -> tuple[bool, bool]:
    if value is None:
        return False, False
    if isinstance(value, bool):
        return True, value
    if isinstance(value, str):
        return True, bool(value.strip())
    if isinstance(value, list):
        for entry in value:
            if not isinstance(entry, str):
                errors.append(f"{field} entries must be strings")
        return True, bool(value)
    errors.append(f"{field} must be a list, string, or bool")
    return True, False


def _parse_fs_block(
    value: Any, field: str, errors: list[str]
) -> tuple[list[str], bool]:
    if value is None:
        return [], False
    if not isinstance(value, dict):
        errors.append(f"{field} must be a table")
        return [], True
    allow: list[str] = []
    for key, capability in (("read", "fs.read"), ("write", "fs.write")):
        present, enabled = _fs_entry_enabled(value.get(key), f"{field}.{key}", errors)
        if present and enabled:
            allow.append(capability)
    return allow, True


def _parse_package_grant(value: Any, field: str, errors: list[str]) -> CapabilityGrant:
    if value is None:
        return CapabilityGrant(allow=None, deny=[], effects=None)
    if isinstance(value, (list, str)):
        allow, _present = _coerce_token_list(value, f"{field}.allow", errors)
        return CapabilityGrant(
            allow=_dedupe_preserve_order(allow), deny=[], effects=None
        )
    if not isinstance(value, dict):
        errors.append(f"{field} must be a list, string, or table")
        return CapabilityGrant(allow=None, deny=[], effects=None)
    allow_tokens, allow_present = _coerce_token_list(
        value.get("allow"), f"{field}.allow", errors
    )
    caps_value = value.get("capabilities")
    caps_tokens: list[str] = []
    caps_present = False
    if isinstance(caps_value, dict):
        nested = _parse_package_grant(caps_value, f"{field}.capabilities", errors)
        allow_tokens = _dedupe_preserve_order(allow_tokens + (nested.allow or []))
        allow_present = True
        if nested.deny:
            errors.append(f"{field}.capabilities must not include deny entries")
        if nested.effects is not None:
            errors.append(f"{field}.capabilities must not include effects entries")
    else:
        caps_tokens, caps_present = _coerce_token_list(
            caps_value, f"{field}.capabilities", errors
        )
    deny_tokens, _deny_present = _coerce_token_list(
        value.get("deny"), f"{field}.deny", errors
    )
    effects_tokens, effects_present = _coerce_effects_list(
        value.get("effects"), f"{field}.effects", errors
    )
    fs_tokens, fs_present = _parse_fs_block(value.get("fs"), f"{field}.fs", errors)
    combined_allow: list[str] = []
    if allow_present:
        combined_allow.extend(allow_tokens)
    if caps_present:
        combined_allow.extend(caps_tokens)
    if fs_present:
        combined_allow.extend(fs_tokens)
    allow = (
        _dedupe_preserve_order(combined_allow)
        if allow_present or caps_present or fs_present
        else None
    )
    effects = _dedupe_preserve_order(effects_tokens) if effects_present else None
    return CapabilityGrant(
        allow=allow,
        deny=_dedupe_preserve_order(deny_tokens),
        effects=effects,
    )


def _parse_package_grants(
    value: Any, field: str, errors: list[str]
) -> dict[str, CapabilityGrant]:
    packages: dict[str, CapabilityGrant] = {}
    if value is None:
        return packages
    if isinstance(value, dict):
        for name, entry in value.items():
            if not isinstance(name, str) or not name:
                errors.append(f"{field} entries must be keyed by package name")
                continue
            grant = _parse_package_grant(entry, f"{field}.{name}", errors)
            if name in packages:
                packages[name] = packages[name].merged(grant)
            else:
                packages[name] = grant
        return packages
    if isinstance(value, list):
        for idx, entry in enumerate(value):
            if not isinstance(entry, dict):
                errors.append(f"{field}[{idx}] must be a table")
                continue
            name = entry.get("name") or entry.get("package")
            if not isinstance(name, str) or not name:
                errors.append(f"{field}[{idx}].name must be a non-empty string")
                continue
            grant = _parse_package_grant(entry, f"{field}.{name}", errors)
            if name in packages:
                packages[name] = packages[name].merged(grant)
            else:
                packages[name] = grant
        return packages
    errors.append(f"{field} must be a table or list")
    return packages


def _parse_capability_manifest_dict(
    data: Any, field: str, errors: list[str]
) -> CapabilityManifest | None:
    if not isinstance(data, dict):
        errors.append(f"{field} must be a table")
        return None
    allow: list[str] | None = None
    deny: list[str] = []
    effects: list[str] | None = None
    packages: dict[str, CapabilityGrant] = {}

    def apply_section(section: Any, ctx: str) -> None:
        nonlocal allow, deny, effects, packages
        if not isinstance(section, dict):
            errors.append(f"{ctx} must be a table")
            return
        caps_value = section.get("capabilities")
        if isinstance(caps_value, dict):
            apply_section(caps_value, f"{ctx}.capabilities")
            caps_value = None
        allow_tokens, allow_present = _coerce_token_list(
            section.get("allow"), f"{ctx}.allow", errors
        )
        caps_tokens: list[str] = []
        caps_present = False
        if caps_value is not None:
            caps_tokens, caps_present = _coerce_token_list(
                caps_value, f"{ctx}.capabilities", errors
            )
        fs_tokens, fs_present = _parse_fs_block(section.get("fs"), f"{ctx}.fs", errors)
        combined_allow: list[str] = []
        if allow_present:
            combined_allow.extend(allow_tokens)
        if caps_present:
            combined_allow.extend(caps_tokens)
        if fs_present:
            combined_allow.extend(fs_tokens)
        if allow_present or caps_present or fs_present:
            if allow is None:
                allow = _dedupe_preserve_order(combined_allow)
            else:
                allow = _dedupe_preserve_order([*allow, *combined_allow])
        deny_tokens, deny_present = _coerce_token_list(
            section.get("deny"), f"{ctx}.deny", errors
        )
        if deny_present:
            deny = _dedupe_preserve_order([*deny, *deny_tokens])
        effects_tokens, effects_present = _coerce_effects_list(
            section.get("effects"), f"{ctx}.effects", errors
        )
        if effects_present:
            if effects is None:
                effects = _dedupe_preserve_order(effects_tokens)
            else:
                effects = _dedupe_preserve_order([*effects, *effects_tokens])
        pkg_entries = _parse_package_grants(
            section.get("packages"), f"{ctx}.packages", errors
        )
        if pkg_entries:
            for name, grant in pkg_entries.items():
                if name in packages:
                    packages[name] = packages[name].merged(grant)
                else:
                    packages[name] = grant

    apply_section(data, field)
    molt_section = data.get("molt")
    if isinstance(molt_section, dict):
        apply_section(molt_section, f"{field}.molt")
    tool_section = data.get("tool")
    if isinstance(tool_section, dict):
        tool_molt = tool_section.get("molt")
        if isinstance(tool_molt, dict):
            apply_section(tool_molt, f"{field}.tool.molt")

    return CapabilityManifest(
        allow=allow,
        deny=deny,
        effects=effects,
        packages=packages,
    )


def _validate_capability_tokens(
    tokens: Iterable[str], field: str, errors: list[str]
) -> None:
    for cap in tokens:
        if not CAPABILITY_TOKEN_RE.match(cap):
            errors.append(f"invalid capability token in {field}: {cap}")


def _validate_effect_tokens(
    tokens: Iterable[str], field: str, errors: list[str]
) -> None:
    for effect in tokens:
        if not CAPABILITY_TOKEN_RE.match(effect):
            errors.append(f"invalid effect token in {field}: {effect}")


def _resolve_capability_manifest(
    manifest: CapabilityManifest,
) -> tuple[list[str], list[str], list[str]]:
    errors: list[str] = []
    allow_tokens = manifest.allow or []
    allow_expanded, allow_profiles = _expand_capabilities(allow_tokens)
    deny_expanded, deny_profiles = _expand_capabilities(manifest.deny)
    profiles = _dedupe_preserve_order([*allow_profiles, *deny_profiles])
    _validate_capability_tokens(allow_expanded, "allow", errors)
    _validate_capability_tokens(deny_expanded, "deny", errors)
    deny_set = set(deny_expanded)
    resolved = _dedupe_preserve_order(
        cap for cap in allow_expanded if cap not in deny_set
    )
    manifest_effects_set: set[str] | None = None
    if manifest.effects is not None:
        _validate_effect_tokens(manifest.effects, "effects", errors)
        manifest_effects_set = set(manifest.effects)
    global_allow = set(resolved)
    for name, grant in manifest.packages.items():
        pkg_allow_tokens = grant.allow or []
        pkg_allow_expanded, pkg_allow_profiles = _expand_capabilities(pkg_allow_tokens)
        pkg_deny_expanded, pkg_deny_profiles = _expand_capabilities(grant.deny)
        profiles = _dedupe_preserve_order(
            [*profiles, *pkg_allow_profiles, *pkg_deny_profiles]
        )
        _validate_capability_tokens(
            pkg_allow_expanded, f"packages.{name}.allow", errors
        )
        _validate_capability_tokens(pkg_deny_expanded, f"packages.{name}.deny", errors)
        if grant.allow is not None:
            extras = [
                cap
                for cap in _dedupe_preserve_order(pkg_allow_expanded)
                if cap not in global_allow
            ]
            if extras:
                errors.append(
                    "packages."
                    + name
                    + ".allow includes capabilities not in global allowlist: "
                    + ", ".join(extras)
                )
        if grant.effects is not None:
            _validate_effect_tokens(grant.effects, f"packages.{name}.effects", errors)
            if manifest_effects_set is not None:
                effect_extras = [
                    effect
                    for effect in _dedupe_preserve_order(grant.effects)
                    if effect not in manifest_effects_set
                ]
                if effect_extras:
                    errors.append(
                        "packages."
                        + name
                        + ".effects includes effects not in global effects allowlist: "
                        + ", ".join(effect_extras)
                    )
    return resolved, profiles, errors


def _parse_capabilities_spec(
    capabilities: CapabilityInput | None,
) -> CapabilitySpec:
    if capabilities is None:
        return CapabilitySpec(
            capabilities=None,
            profiles=[],
            source=None,
            errors=[],
            manifest=None,
        )
    errors: list[str] = []
    profiles: list[str] = []
    source: str | None = None
    manifest: CapabilityManifest | None = None
    if isinstance(capabilities, dict):
        source = "config"
        manifest = _parse_capability_manifest_dict(capabilities, "capabilities", errors)
    elif isinstance(capabilities, list):
        source = "config"
        tokens, _present = _coerce_token_list(capabilities, "capabilities", errors)
        manifest = CapabilityManifest(
            allow=_dedupe_preserve_order(tokens),
            deny=[],
            effects=None,
            packages={},
        )
    else:
        if isinstance(capabilities, str) and not capabilities.strip():
            source = "inline"
            manifest = CapabilityManifest(
                allow=[],
                deny=[],
                effects=None,
                packages={},
            )
            resolved, profiles, resolve_errors = _resolve_capability_manifest(manifest)
            if resolve_errors:
                return CapabilitySpec(
                    capabilities=None,
                    profiles=profiles,
                    source=None,
                    errors=resolve_errors,
                    manifest=manifest,
                )
            return CapabilitySpec(
                capabilities=resolved,
                profiles=profiles,
                source=source,
                errors=[],
                manifest=manifest,
            )
        path = Path(capabilities)
        if path.exists():
            source = str(path)
            try:
                if path.suffix == ".json":
                    data = json.loads(path.read_text())
                else:
                    data = tomllib.loads(path.read_text())
            except (OSError, json.JSONDecodeError, tomllib.TOMLDecodeError):
                return CapabilitySpec(
                    capabilities=None,
                    profiles=[],
                    source=source,
                    errors=["failed to load capabilities file"],
                    manifest=None,
                )
            manifest = _parse_capability_manifest_dict(data, "capabilities", errors)
        else:
            source = "inline"
            tokens = _split_tokens(capabilities)
            manifest = CapabilityManifest(
                allow=_dedupe_preserve_order(tokens),
                deny=[],
                effects=None,
                packages={},
            )
    if manifest is None:
        return CapabilitySpec(
            capabilities=None,
            profiles=profiles,
            source=source,
            errors=errors,
            manifest=None,
        )
    resolved, profiles, resolve_errors = _resolve_capability_manifest(manifest)
    errors.extend(resolve_errors)
    if errors:
        return CapabilitySpec(
            capabilities=None,
            profiles=profiles,
            source=source,
            errors=errors,
            manifest=manifest,
        )
    return CapabilitySpec(
        capabilities=resolved,
        profiles=profiles,
        source=source,
        errors=[],
        manifest=manifest,
    )


def _parse_capabilities(
    capabilities: CapabilityInput | None,
) -> tuple[list[str] | None, list[str], str | None, list[str]]:
    spec = _parse_capabilities_spec(capabilities)
    return spec.capabilities, spec.profiles, spec.source, spec.errors


def _format_capabilities_input(value: CapabilityInput | None) -> str:
    if value is None:
        return "none"
    if isinstance(value, list):
        return ", ".join(value) if value else "(empty)"
    if isinstance(value, str):
        return value if value else "(empty)"
    return json.dumps(value, sort_keys=True)


def _allowed_capabilities_for_package(
    global_allow: list[str],
    manifest: CapabilityManifest | None,
    package_name: str | None,
) -> set[str]:
    allowed = set(global_allow)
    if manifest is None or not package_name:
        return allowed
    grant = manifest.packages.get(package_name)
    if grant is None:
        return allowed
    if grant.allow is not None:
        grant_allow, _profiles = _expand_capabilities(grant.allow)
        allowed &= set(grant_allow)
    if grant.deny:
        grant_deny, _profiles = _expand_capabilities(grant.deny)
        allowed -= set(grant_deny)
    return allowed


def _allowed_effects_for_package(
    manifest: CapabilityManifest | None,
    package_name: str | None,
) -> set[str] | None:
    if manifest is None:
        return None
    allowed: set[str] | None = None
    if manifest.effects is not None:
        allowed = set(manifest.effects)
    grant = manifest.packages.get(package_name) if package_name else None
    if grant is None or grant.effects is None:
        return allowed
    grant_effects = set(grant.effects)
    if allowed is None:
        return grant_effects
    return allowed & grant_effects


def _materialize_capabilities_arg(
    capabilities: CapabilityInput,
) -> tuple[str, Path | None]:
    if isinstance(capabilities, list):
        return ",".join(capabilities), None
    if isinstance(capabilities, str):
        return capabilities, None
    handle = tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        suffix=".json",
        prefix="molt_capabilities_",
        delete=False,
    )
    try:
        json.dump(capabilities, handle, sort_keys=True, indent=2)
        handle.write("\n")
        path = Path(handle.name)
    finally:
        handle.close()
    return str(path), path


def _expand_capabilities(items: list[str]) -> tuple[list[str], list[str]]:
    expanded: list[str] = []
    profiles: list[str] = []
    for item in items:
        key = item.strip()
        if not key:
            continue
        profile = CAPABILITY_PROFILES.get(key)
        if profile is not None:
            profiles.append(key)
            expanded.extend(profile)
        else:
            expanded.append(key)
    # Preserve order while de-duplicating.
    seen: set[str] = set()
    deduped: list[str] = []
    for cap in expanded:
        if cap in seen:
            continue
        seen.add(cap)
        deduped.append(cap)
    return deduped, profiles


@functools.lru_cache(maxsize=128)
def _runtime_source_paths_cached(project_root_str: str) -> tuple[Path, ...]:
    project_root = Path(project_root_str)
    return (
        project_root / "runtime/molt-runtime/src",
        project_root / "runtime/molt-runtime/Cargo.toml",
        project_root / "runtime/molt-runtime/build.rs",
        project_root / "runtime/molt-obj-model/src",
        project_root / "runtime/molt-obj-model/Cargo.toml",
        project_root / "runtime/molt-obj-model/build.rs",
        project_root / "Cargo.toml",
        project_root / "Cargo.lock",
    )


def _runtime_source_paths(project_root: Path) -> list[Path]:
    return list(_runtime_source_paths_cached(os.fspath(project_root)))


@functools.lru_cache(maxsize=128)
def _backend_source_paths_cached(project_root_str: str) -> tuple[Path, ...]:
    project_root = Path(project_root_str)
    return (
        project_root / "runtime/molt-backend/src",
        project_root / "runtime/molt-backend/Cargo.toml",
        project_root / "runtime/molt-backend/build.rs",
        project_root / "Cargo.toml",
        project_root / "Cargo.lock",
    )


def _backend_source_paths(project_root: Path) -> list[Path]:
    return list(_backend_source_paths_cached(os.fspath(project_root)))


@functools.lru_cache(maxsize=256)
def _backend_bin_path_cached(
    project_root_str: str,
    cargo_profile: str,
    cargo_target_override: str | None,
    cwd_str: str,
    os_name: str,
) -> Path:
    profile_dir = _cargo_profile_dir(cargo_profile)
    target_root = _cargo_target_root_cached(
        project_root_str,
        cargo_target_override,
        cwd_str,
    )
    exe_suffix = ".exe" if os_name == "nt" else ""
    return target_root / profile_dir / f"molt-backend{exe_suffix}"


def _backend_bin_path(project_root: Path, cargo_profile: str) -> Path:
    return _backend_bin_path_cached(
        os.fspath(project_root),
        cargo_profile,
        os.environ.get("CARGO_TARGET_DIR"),
        os.fspath(Path.cwd()),
        os.name,
    )


@functools.lru_cache(maxsize=256)
def _runtime_lib_path_cached(
    project_root_str: str,
    cargo_profile: str,
    target_triple: str | None,
    cargo_target_override: str | None,
    cwd_str: str,
) -> Path:
    profile_dir = _cargo_profile_dir(cargo_profile)
    target_root = _cargo_target_root_cached(
        project_root_str,
        cargo_target_override,
        cwd_str,
    )
    if target_triple:
        return target_root / target_triple / profile_dir / "libmolt_runtime.a"
    return target_root / profile_dir / "libmolt_runtime.a"


def _runtime_lib_path(
    project_root: Path,
    cargo_profile: str,
    target_triple: str | None,
) -> Path:
    return _runtime_lib_path_cached(
        os.fspath(project_root),
        cargo_profile,
        target_triple,
        os.environ.get("CARGO_TARGET_DIR"),
        os.fspath(Path.cwd()),
    )


@functools.lru_cache(maxsize=256)
def _runtime_wasm_artifact_path_cached(
    project_root_str: str,
    artifact_name: str,
    wasm_runtime_dir_override: str | None,
    ext_root_override: str | None,
    cwd_str: str,
) -> Path:
    project_root = Path(project_root_str)
    if wasm_runtime_dir_override:
        base = Path(wasm_runtime_dir_override).expanduser()
    else:
        configured = ext_root_override
        external_root = Path(configured).expanduser() if configured else Path(cwd_str)
        if external_root.is_dir():
            base = external_root / "wasm"
        else:
            base = project_root / "wasm"
    return base / artifact_name


def _runtime_wasm_artifact_path(project_root: Path, artifact_name: str) -> Path:
    return _runtime_wasm_artifact_path_cached(
        os.fspath(project_root),
        artifact_name,
        os.environ.get("MOLT_WASM_RUNTIME_DIR"),
        os.environ.get("MOLT_EXT_ROOT"),
        os.fspath(Path.cwd()),
    )


@functools.lru_cache(maxsize=32)
def _resolve_backend_profile_cached(
    default_profile: BuildProfile,
    raw: str | None,
) -> tuple[BuildProfile, str | None]:
    if not raw:
        return default_profile, None
    value = raw.strip().lower()
    if value not in {"dev", "release"}:
        return default_profile, f"Invalid MOLT_BACKEND_PROFILE value: {raw}"
    return cast(BuildProfile, value), None


def _resolve_backend_profile(
    default_profile: BuildProfile,
) -> tuple[BuildProfile, str | None]:
    return _resolve_backend_profile_cached(
        default_profile,
        os.environ.get("MOLT_BACKEND_PROFILE"),
    )


@functools.lru_cache(maxsize=32)
def _resolve_cargo_profile_name_cached(
    build_profile: BuildProfile,
    raw: str,
) -> tuple[str, str | None]:
    env_var = (
        "MOLT_DEV_CARGO_PROFILE"
        if build_profile == "dev"
        else "MOLT_RELEASE_CARGO_PROFILE"
    )
    normalized_raw = raw.strip()
    default_profile = "dev-fast" if build_profile == "dev" else "release"
    profile_name = normalized_raw or default_profile
    if not _CARGO_PROFILE_NAME_RE.match(profile_name):
        return build_profile, f"Invalid {env_var} value: {raw}"
    return profile_name, None


def _resolve_cargo_profile_name(
    build_profile: BuildProfile,
) -> tuple[str, str | None]:
    env_var = (
        "MOLT_DEV_CARGO_PROFILE"
        if build_profile == "dev"
        else "MOLT_RELEASE_CARGO_PROFILE"
    )
    return _resolve_cargo_profile_name_cached(
        build_profile,
        os.environ.get(env_var, ""),
    )


@functools.lru_cache(maxsize=32)
def _resolve_wasm_cargo_profile_cached(
    cargo_profile: str,
    override: str,
) -> str:
    if override:
        return override
    if cargo_profile == "release":
        return "wasm-release"
    return cargo_profile


def _resolve_wasm_cargo_profile(cargo_profile: str) -> str:
    """Map cargo profile for WASM targets.

    Uses ``wasm-release`` (thin LTO, 4 codegen-units) instead of ``release``
    (full LTO, 1 codegen-unit) for dramatically faster WASM compilation with
    comparable runtime performance.  Override with ``MOLT_WASM_CARGO_PROFILE``.
    """
    return _resolve_wasm_cargo_profile_cached(
        cargo_profile,
        os.environ.get("MOLT_WASM_CARGO_PROFILE", "").strip(),
    )


@functools.lru_cache(maxsize=32)
def _native_arch_perf_requested_cached(
    profile_raw: str,
    native_arch_raw: str,
) -> bool:
    profile = profile_raw.strip().lower()
    if profile in {"native-arch", "native_arch", "native"}:
        return True
    raw = native_arch_raw.strip().lower()
    return raw in {"1", "true", "yes", "on"}


def _native_arch_perf_requested() -> bool:
    return _native_arch_perf_requested_cached(
        os.environ.get("MOLT_PERF_PROFILE", ""),
        os.environ.get("MOLT_NATIVE_ARCH_PERF", ""),
    )


def _enable_native_arch_rustflags() -> bool:
    flag = "-C target-cpu=native"
    existing = os.environ.get("RUSTFLAGS", "")
    if flag in existing:
        return False
    _append_rustflags(os.environ, flag)
    return True


@functools.lru_cache(maxsize=64)
def _backend_codegen_env_inputs_cached(
    is_wasm: bool,
    native_values: tuple[tuple[str, str], ...],
    wasm_values: tuple[tuple[str, str], ...],
) -> dict[str, str]:
    payload = {key: value for key, value in native_values}
    if is_wasm:
        payload.update({key: value for key, value in wasm_values})
    return {name: payload[name] for name in sorted(payload)}


def _backend_codegen_env_inputs(
    *,
    is_wasm: bool,
    env: Mapping[str, str] | None = None,
) -> dict[str, str]:
    source = env if env is not None else os.environ
    native_values = tuple(
        (key, value)
        for key in _NATIVE_CODEGEN_ENV_KNOBS
        if (value := (source.get(key) or "").strip())
    )
    wasm_values = tuple(
        (key, value)
        for key in _WASM_CODEGEN_ENV_KNOBS
        if (value := (source.get(key) or "").strip())
    )
    if env is None:
        return _backend_codegen_env_inputs_cached(
            is_wasm,
            native_values,
            wasm_values,
        )
    payload = {key: value for key, value in native_values}
    if is_wasm:
        payload.update({key: value for key, value in wasm_values})
    return {name: payload[name] for name in sorted(payload)}


def _backend_codegen_env_digest(
    *,
    is_wasm: bool,
    env: Mapping[str, str] | None = None,
) -> str:
    payload = {
        "schema": _BACKEND_CODEGEN_ENV_DIGEST_SCHEMA_VERSION,
        "target": "wasm" if is_wasm else "native",
        "inputs": _backend_codegen_env_inputs(is_wasm=is_wasm, env=env),
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _backend_daemon_config_digest(
    project_root: Path,
    cargo_profile: str,
    *,
    env: Mapping[str, str] | None = None,
) -> str:
    payload = {
        "schema": _DAEMON_CONFIG_DIGEST_SCHEMA_VERSION,
        "project_root": str(project_root.resolve()),
        "cargo_profile": cargo_profile,
        "codegen": _backend_codegen_env_inputs(is_wasm=False, env=env),
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


@functools.lru_cache(maxsize=32)
def _backend_daemon_enabled_cached(os_name: str, raw: str) -> bool:
    if os_name != "posix":
        return False
    return raw.strip().lower() not in {"0", "false", "no", "off"}


def _backend_daemon_enabled() -> bool:
    return _backend_daemon_enabled_cached(
        os.name,
        os.environ.get("MOLT_BACKEND_DAEMON", "1"),
    )


def _backend_daemon_start_timeout() -> None:
    return None


def _backend_daemon_socket_dir(project_root: Path) -> Path:
    # Unix sockets can fail on some external/shared volumes (e.g. exFAT).
    # Keep sockets on a local socket-capable path by default.
    default_dir = Path(tempfile.gettempdir()) / "molt-backend-daemon"
    socket_dir = _resolve_env_path("MOLT_BACKEND_DAEMON_SOCKET_DIR", default_dir)
    socket_dir.mkdir(parents=True, exist_ok=True)
    return socket_dir


@functools.lru_cache(maxsize=256)
def _backend_daemon_paths_cached(
    project_root_str: str,
    cargo_profile: str,
    config_digest: str | None,
    explicit_socket: str,
    socket_dir_override: str | None,
    build_state_root_str: str,
    tempdir_str: str,
) -> tuple[Path, Path, Path]:
    project_root = Path(project_root_str)
    build_state_root = Path(build_state_root_str)
    if explicit_socket:
        socket_path = Path(explicit_socket).expanduser()
        if not socket_path.is_absolute():
            socket_path = (project_root / socket_path).absolute()
    else:
        default_dir = Path(tempdir_str) / "molt-backend-daemon"
        if socket_dir_override:
            socket_dir = Path(socket_dir_override).expanduser()
            if not socket_dir.is_absolute():
                socket_dir = (Path.cwd() / socket_dir).absolute()
        else:
            socket_dir = default_dir
        daemon_digest = config_digest or _backend_daemon_config_digest(
            project_root, cargo_profile
        )
        key = f"{project_root.resolve()}|{build_state_root}|{cargo_profile}|{daemon_digest}"
        suffix = hashlib.sha256(key.encode("utf-8")).hexdigest()[:16]
        socket_path = socket_dir / f"moltbd.{suffix}.sock"
    daemon_root = build_state_root / "backend_daemon"
    return (
        socket_path,
        daemon_root / f"molt-backend.{cargo_profile}.log",
        daemon_root / f"molt-backend.{cargo_profile}.pid",
    )


def _backend_daemon_socket_path(
    project_root: Path,
    cargo_profile: str,
    *,
    config_digest: str | None = None,
) -> Path:
    socket_path, _log_path, _pid_path = _backend_daemon_paths_cached(
        os.fspath(project_root),
        cargo_profile,
        config_digest,
        os.environ.get("MOLT_BACKEND_DAEMON_SOCKET", "").strip(),
        os.environ.get("MOLT_BACKEND_DAEMON_SOCKET_DIR"),
        os.fspath(_build_state_root(project_root)),
        tempfile.gettempdir(),
    )
    socket_path.parent.mkdir(parents=True, exist_ok=True)
    return socket_path


def _backend_daemon_log_path(project_root: Path, cargo_profile: str) -> Path:
    _socket_path, log_path, _pid_path = _backend_daemon_paths_cached(
        os.fspath(project_root),
        cargo_profile,
        None,
        os.environ.get("MOLT_BACKEND_DAEMON_SOCKET", "").strip(),
        os.environ.get("MOLT_BACKEND_DAEMON_SOCKET_DIR"),
        os.fspath(_build_state_root(project_root)),
        tempfile.gettempdir(),
    )
    log_path.parent.mkdir(parents=True, exist_ok=True)
    return log_path


def _backend_daemon_pid_path(project_root: Path, cargo_profile: str) -> Path:
    _socket_path, _log_path, pid_path = _backend_daemon_paths_cached(
        os.fspath(project_root),
        cargo_profile,
        None,
        os.environ.get("MOLT_BACKEND_DAEMON_SOCKET", "").strip(),
        os.environ.get("MOLT_BACKEND_DAEMON_SOCKET_DIR"),
        os.fspath(_build_state_root(project_root)),
        tempfile.gettempdir(),
    )
    pid_path.parent.mkdir(parents=True, exist_ok=True)
    return pid_path


def _read_backend_daemon_pid(pid_path: Path) -> int | None:
    try:
        raw = pid_path.read_text().strip()
    except OSError:
        return None
    if not raw.isdigit():
        return None
    pid = int(raw)
    return pid if pid > 0 else None


def _write_backend_daemon_pid(pid_path: Path, pid: int) -> None:
    pid_path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = pid_path.with_name(f".{pid_path.name}.{os.getpid()}.tmp")
    try:
        tmp_path.write_text(f"{pid}\n")
        tmp_path.replace(pid_path)
    finally:
        try:
            if tmp_path.exists():
                tmp_path.unlink()
        except OSError:
            pass


def _remove_backend_daemon_pid(pid_path: Path) -> None:
    try:
        pid_path.unlink()
    except OSError:
        pass


def _backend_daemon_binary_is_newer(backend_bin: Path, pid_path: Path) -> bool:
    try:
        return backend_bin.stat().st_mtime > (pid_path.stat().st_mtime + 1e-6)
    except OSError:
        return False


def _pid_alive(pid: int) -> bool:
    if pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def _terminate_backend_daemon_pid(pid: int, *, grace: float = 1.0) -> None:
    if pid <= 0:
        return
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    except OSError:
        return
    deadline = time.monotonic() + max(0.05, grace)
    while time.monotonic() < deadline:
        if not _pid_alive(pid):
            return
        time.sleep(0.05)
    try:
        os.kill(pid, signal.SIGKILL)
    except OSError:
        return


def _atomic_copy_file(src: Path, dst: Path) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = dst.with_name(f".{dst.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    try:
        shutil.copyfile(src, tmp_path)
        tmp_path.replace(dst)
    finally:
        try:
            if tmp_path.exists():
                tmp_path.unlink()
        except OSError:
            pass


def _atomic_link_or_copy_file(src: Path, dst: Path) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = dst.with_name(f".{dst.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    try:
        try:
            os.link(src, tmp_path)
            try:
                tmp_path.replace(dst)
                return
            except OSError as exc:
                if exc.errno != errno.ENOENT:
                    raise
        except OSError as exc:
            if exc.errno not in {
                errno.EXDEV,
                errno.EPERM,
                errno.EACCES,
                errno.ENOTSUP,
                errno.ENOENT,
            }:
                raise
        shutil.copyfile(src, tmp_path)
        tmp_path.replace(dst)
    finally:
        try:
            if tmp_path.exists():
                tmp_path.unlink()
        except OSError:
            pass


def _materialize_cached_backend_artifact(
    project_root: Path,
    candidate: Path,
    output_artifact: Path,
    *,
    tier: str,
    source_key: str,
    cache_path: Path | None,
    warnings: list[str],
    state_path: Path | None = None,
    state: dict[str, Any] | None = None,
    output_stat: os.stat_result | None = None,
) -> bool:
    if state_path is None:
        state_path = _artifact_sync_state_path(project_root, output_artifact)
        state = _read_artifact_sync_state(state_path)
    if output_stat is None:
        with contextlib.suppress(OSError):
            output_stat = output_artifact.stat()
    if output_stat is not None and _artifact_sync_state_matches_stat(
        state,
        source_key=source_key,
        tier=tier,
        stat=output_stat,
    ):
        return True
    try:
        _atomic_link_or_copy_file(candidate, output_artifact)
        if (
            tier == "function"
            and cache_path is not None
            and candidate != cache_path
            and not cache_path.exists()
        ):
            with contextlib.suppress(OSError):
                _atomic_link_or_copy_file(candidate, cache_path)
        try:
            state_path.parent.mkdir(parents=True, exist_ok=True)
            _write_artifact_sync_state(
                state_path,
                source_key=source_key,
                tier=tier,
                artifact=output_artifact,
            )
        except OSError:
            pass
        return True
    except OSError as exc:
        warnings.append(f"Cache copy failed: {exc}")
        return False


def _try_cached_backend_candidates(
    *,
    project_root: Path,
    cache_candidates: Sequence[tuple[str, Path]],
    output_artifact: Path,
    is_wasm: bool,
    cache_key: str | None,
    function_cache_key: str | None,
    cache_path: Path | None,
    warnings: list[str],
) -> tuple[bool, str | None]:
    state_path = _artifact_sync_state_path(project_root, output_artifact)
    state = _read_artifact_sync_state(state_path)
    try:
        output_stat: os.stat_result | None = output_artifact.stat()
    except OSError:
        output_stat = None
    for tier, candidate in cache_candidates:
        if not candidate.exists():
            continue
        if not _is_valid_cached_backend_artifact(candidate, is_wasm=is_wasm):
            warnings.append(f"Ignoring invalid cache artifact: {candidate}")
            with contextlib.suppress(OSError):
                candidate.unlink()
            continue
        if _materialize_cached_backend_artifact(
            project_root,
            candidate,
            output_artifact,
            tier=tier,
            source_key=cache_key if tier == "module" else (function_cache_key or cache_key or ""),
            cache_path=cache_path,
            warnings=warnings,
            state_path=state_path,
            state=state,
            output_stat=output_stat,
        ):
            return True, tier
    return False, None


def _backend_daemon_skip_output_sync_flags(
    project_root: Path,
    output_artifact: Path,
    *,
    cache_key: str | None,
    function_cache_key: str | None,
    state_path: Path | None = None,
    state: dict[str, Any] | None = None,
    output_stat: os.stat_result | None = None,
) -> tuple[bool, bool]:
    if state_path is None:
        state_path = _artifact_sync_state_path(project_root, output_artifact)
        state = _read_artifact_sync_state(state_path)
    if output_stat is None:
        try:
            output_stat = output_artifact.stat()
        except OSError:
            return False, False
    skip_module_output = bool(cache_key) and _artifact_sync_state_matches_stat(
        state,
        source_key=cache_key or "",
        tier="module",
        stat=output_stat,
    )
    skip_function_output = bool(
        function_cache_key
    ) and _artifact_sync_state_matches_stat(
        state,
        source_key=function_cache_key or "",
        tier="function",
        stat=output_stat,
    )
    return skip_module_output, skip_function_output


@contextmanager
def _temporary_backend_output_path(
    artifacts_root: Path,
    *,
    is_wasm: bool,
) -> Iterator[Path]:
    suffix = ".wasm" if is_wasm else ".o"
    artifacts_root.mkdir(parents=True, exist_ok=True)
    path = artifacts_root / f"backend_{os.getpid()}_{uuid.uuid4().hex}{suffix}"
    try:
        yield path
    finally:
        with contextlib.suppress(OSError):
            path.unlink()


def _stage_backend_output_and_caches(
    project_root: Path,
    backend_output: Path,
    output_artifact: Path,
    *,
    cache_path: Path | None,
    cache_key: str | None,
    function_cache_path: Path | None,
    warnings: list[str],
    output_already_synced: bool | None = None,
    state_path: Path | None = None,
    state: dict[str, Any] | None = None,
    output_stat: os.stat_result | None = None,
) -> str | None:
    try:
        if output_artifact.parent != Path("."):
            output_artifact.parent.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        return f"Failed to move backend output: {exc}"

    staged_source = backend_output
    if cache_path is not None:
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        if backend_output != cache_path:
            try:
                backend_output.replace(cache_path)
                staged_source = cache_path
            except OSError as exc:
                if exc.errno != errno.EXDEV:
                    return f"Failed to move backend output: {exc}"
                try:
                    _atomic_copy_file(backend_output, cache_path)
                    backend_output.unlink()
                    staged_source = cache_path
                except OSError as copy_exc:
                    return f"Failed to move backend output: {copy_exc}"
        else:
            staged_source = cache_path

    if state_path is None:
        state_path = _artifact_sync_state_path(project_root, output_artifact)
    if output_already_synced is None:
        state = _read_artifact_sync_state(state_path)
        if output_stat is None:
            try:
                output_stat = output_artifact.stat()
            except OSError:
                output_stat = None
        output_already_synced = (
            bool(cache_key)
            and output_stat is not None
            and (
                _artifact_sync_state_matches_stat(
                    state,
                    source_key=cache_key or "",
                    tier="module",
                    stat=output_stat,
                )
            )
        )

    try:
        if output_already_synced:
            pass
        elif staged_source == backend_output and cache_path is None:
            backend_output.replace(output_artifact)
        else:
            _atomic_link_or_copy_file(staged_source, output_artifact)
    except OSError as exc:
        return f"Failed to move backend output: {exc}"

    if cache_path is None:
        return None

    if function_cache_path is not None and function_cache_path != cache_path:
        try:
            _atomic_link_or_copy_file(cache_path, function_cache_path)
        except OSError as exc:
            warnings.append(f"Function cache write failed: {exc}")
    if cache_key and not output_already_synced:
        try:
            state_path.parent.mkdir(parents=True, exist_ok=True)
            _write_artifact_sync_state(
                state_path,
                source_key=cache_key,
                tier="module",
                artifact=output_artifact,
            )
        except OSError:
            pass
    return None


def _backend_daemon_request_bytes(
    socket_path: Path,
    data: bytes,
    *,
    timeout: float | None,
) -> tuple[dict[str, Any] | None, str | None]:
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            if timeout is not None:
                sock.settimeout(timeout)
            sock.connect(str(socket_path))
            return _backend_daemon_request_on_socket(sock, data, shutdown_write=True)
    except OSError as exc:
        return None, f"backend daemon connection failed: {exc}"


def _backend_daemon_request_on_socket(
    sock: socket.socket,
    data: bytes,
    *,
    shutdown_write: bool,
) -> tuple[dict[str, Any] | None, str | None]:
    try:
        sock.sendall(data)
        if shutdown_write:
            sock.shutdown(socket.SHUT_WR)
        raw = bytearray()
        recv_buffer = bytearray(65536)
        recv_view = memoryview(recv_buffer)
        while True:
            received = sock.recv_into(recv_view)
            if received == 0:
                break
            raw.extend(recv_view[:received])
            if b"\n" in raw:
                raw = raw.partition(b"\n")[0]
                break
    except OSError as exc:
        return None, f"backend daemon connection failed: {exc}"
    if not raw or all(byte in b" \t\r\n" for byte in raw):
        return None, "backend daemon returned empty response"
    try:
        response = json.loads(raw)
    except json.JSONDecodeError as exc:
        return None, f"backend daemon returned invalid JSON: {exc}"
    if not isinstance(response, dict):
        return None, "backend daemon returned non-object response"
    return response, None


def _backend_daemon_request(
    socket_path: Path,
    payload: dict[str, Any],
    *,
    timeout: float | None,
) -> tuple[dict[str, Any] | None, str | None]:
    data, encode_err = _backend_daemon_request_payload_bytes(payload)
    if encode_err is not None:
        return None, encode_err
    assert data is not None
    return _backend_daemon_request_bytes(socket_path, data, timeout=timeout)


def _backend_daemon_request_payload_bytes(
    payload: dict[str, Any],
) -> tuple[bytes | None, str | None]:
    try:
        encoded = json.dumps(
            payload,
            default=_json_ir_default,
            separators=(",", ":"),
        ).encode("utf-8")
    except (TypeError, ValueError) as exc:
        return None, f"backend daemon request encode failed: {exc}"
    return encoded + b"\n", None


def _backend_daemon_compile_request_bytes(
    *,
    ir: dict[str, Any] | None,
    backend_output: Path,
    is_wasm: bool,
    target_triple: str | None,
    cache_key: str | None,
    function_cache_key: str | None,
    config_digest: str | None,
    skip_module_output_if_synced: bool,
    skip_function_output_if_synced: bool,
    probe_cache_only: bool = False,
    include_health: bool = False,
) -> tuple[bytes | None, str | None]:
    job: dict[str, Any] = {
        "id": "job0",
        "is_wasm": is_wasm,
        "target_triple": target_triple,
        "output": str(backend_output),
        "cache_key": cache_key or "",
        "function_cache_key": function_cache_key or "",
        "skip_module_output_if_synced": skip_module_output_if_synced,
        "skip_function_output_if_synced": skip_function_output_if_synced,
    }
    if probe_cache_only:
        job["probe_cache_only"] = True
    elif ir is not None:
        job["ir"] = ir
    jobs: list[dict[str, Any]] = [job]
    payload: dict[str, Any] = {
        "version": _BACKEND_DAEMON_PROTOCOL_VERSION,
        "jobs": jobs,
    }
    if config_digest:
        payload["config_digest"] = config_digest
    if include_health:
        payload["include_health"] = True
    return _backend_daemon_request_payload_bytes(payload)


def _backend_daemon_health_from_response(
    response: dict[str, Any],
) -> dict[str, Any] | None:
    raw = response.get("health")
    if not isinstance(raw, dict):
        return None
    health: dict[str, Any] = {}
    int_fields = {
        "protocol_version",
        "pid",
        "uptime_ms",
        "cache_entries",
        "cache_bytes",
        "cache_max_bytes",
        "request_limit_bytes",
        "max_jobs",
        "requests_total",
        "jobs_total",
        "cache_hits",
        "cache_misses",
    }
    for field in int_fields:
        value = raw.get(field)
        if isinstance(value, int):
            health[field] = value
    return health or None


def _backend_daemon_ping_health(
    socket_path: Path, *, timeout: float | None
) -> tuple[bool, dict[str, Any] | None]:
    payload = {"version": _BACKEND_DAEMON_PROTOCOL_VERSION, "ping": True}
    response, err = _backend_daemon_request(socket_path, payload, timeout=timeout)
    if err is not None or response is None:
        return False, None
    health = _backend_daemon_health_from_response(response)
    return bool(response.get("ok")) and bool(response.get("pong")), health


def _backend_daemon_ping(socket_path: Path, *, timeout: float | None) -> bool:
    ready, _ = _backend_daemon_ping_health(socket_path, timeout=timeout)
    return ready


def _backend_daemon_wait_until_ready(
    socket_path: Path,
    *,
    ready_timeout: float | None,
    probe_timeout: float | None = None,
) -> tuple[bool, dict[str, Any] | None]:
    deadline = (
        time.monotonic() + max(0.05, ready_timeout)
        if ready_timeout is not None
        else None
    )
    while deadline is None or time.monotonic() < deadline:
        ready, health = _backend_daemon_ping_health(socket_path, timeout=probe_timeout)
        if ready:
            return True, health
        time.sleep(0.05)
    return False, None


def _backend_daemon_retryable_error(error: str | None) -> bool:
    if not error:
        return False
    lowered = error.lower()
    return (
        "connection failed" in lowered
        or "empty response" in lowered
        or "invalid json" in lowered
        or "unsupported protocol version" in lowered
        or "missing job results" in lowered
        or "output is missing" in lowered
    )


def _start_backend_daemon(
    backend_bin: Path,
    socket_path: Path,
    *,
    cargo_profile: str,
    project_root: Path,
    startup_timeout: float | None,
    json_output: bool,
) -> bool:
    startup_wait = startup_timeout if startup_timeout is not None else None
    pid_path = _backend_daemon_pid_path(project_root, cargo_profile)
    existing_pid = _read_backend_daemon_pid(pid_path)
    if existing_pid is not None:
        if _pid_alive(existing_pid):
            if _backend_daemon_binary_is_newer(backend_bin, pid_path):
                _terminate_backend_daemon_pid(existing_pid, grace=1.0)
                _remove_backend_daemon_pid(pid_path)
                try:
                    if socket_path.exists():
                        socket_path.unlink()
                except OSError:
                    pass
                existing_pid = None
            else:
                if socket_path.exists():
                    ready, _ = _backend_daemon_wait_until_ready(
                        socket_path,
                        ready_timeout=startup_wait,
                        probe_timeout=None,
                    )
                    if ready:
                        return True
                    if not json_output:
                        print(
                            "Backend daemon is running but did not become ready "
                            "within the startup probe window; skipping restart.",
                            file=sys.stderr,
                        )
                    return False
                _terminate_backend_daemon_pid(existing_pid, grace=1.0)
        if existing_pid is not None:
            _remove_backend_daemon_pid(pid_path)
    try:
        if socket_path.exists():
            ready, _ = _backend_daemon_wait_until_ready(
                socket_path,
                ready_timeout=startup_wait,
                probe_timeout=None,
            )
            if ready:
                return True
            socket_path.unlink()
    except OSError:
        pass
    log_path = _backend_daemon_log_path(project_root, cargo_profile)
    daemon_pid: int | None = None
    try:
        log_path.parent.mkdir(parents=True, exist_ok=True)
        with log_path.open("ab") as log_file:
            daemon = subprocess.Popen(
                [str(backend_bin), "--daemon", "--socket", str(socket_path)],
                cwd=project_root,
                stdout=log_file,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            daemon_pid = daemon.pid
            _write_backend_daemon_pid(pid_path, daemon_pid)
    except OSError as exc:
        if daemon_pid is not None:
            _remove_backend_daemon_pid(pid_path)
        if not json_output:
            print(f"Failed to start backend daemon: {exc}", file=sys.stderr)
        return False
    ready, _ = _backend_daemon_wait_until_ready(
        socket_path,
        ready_timeout=startup_wait,
        probe_timeout=None,
    )
    if ready:
        return True
    if daemon_pid is not None:
        _terminate_backend_daemon_pid(daemon_pid, grace=1.0)
    _remove_backend_daemon_pid(pid_path)
    if not json_output:
        print("Backend daemon failed to become ready in time.", file=sys.stderr)
    return False


def _compile_with_backend_daemon(
    socket_path: Path,
    *,
    ir: dict[str, Any],
    backend_output: Path,
    is_wasm: bool,
    target_triple: str | None,
    cache_key: str | None,
    function_cache_key: str | None,
    config_digest: str | None,
    skip_module_output_if_synced: bool = False,
    skip_function_output_if_synced: bool = False,
    timeout: float | None,
    request_bytes: bytes | None = None,
) -> _BackendDaemonCompileResult:
    full_request_bytes = request_bytes
    probe_request_bytes: bytes | None = None
    probe_followup_socket: socket.socket | None = None
    if request_bytes is None and (cache_key or function_cache_key):
        probe_request_bytes, probe_encode_err = _backend_daemon_compile_request_bytes(
            ir=None,
            backend_output=backend_output,
            is_wasm=is_wasm,
            target_triple=target_triple,
            cache_key=cache_key,
            function_cache_key=function_cache_key,
            config_digest=config_digest,
            skip_module_output_if_synced=skip_module_output_if_synced,
            skip_function_output_if_synced=skip_function_output_if_synced,
            probe_cache_only=True,
            include_health=False,
        )
        if probe_encode_err is not None:
            return _BackendDaemonCompileResult(
                False, probe_encode_err, None, None, None, True, False
            )
    elif full_request_bytes is None:
        full_request_bytes, encode_err = _backend_daemon_compile_request_bytes(
            ir=ir,
            backend_output=backend_output,
            is_wasm=is_wasm,
            target_triple=target_triple,
            cache_key=cache_key,
            function_cache_key=function_cache_key,
            config_digest=config_digest,
            skip_module_output_if_synced=skip_module_output_if_synced,
            skip_function_output_if_synced=skip_function_output_if_synced,
            include_health=False,
        )
        if encode_err is not None:
            return _BackendDaemonCompileResult(
                False, encode_err, None, None, None, True, False
            )
        assert full_request_bytes is not None
    if probe_request_bytes is not None:
        try:
            probe_followup_socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            if timeout is not None:
                probe_followup_socket.settimeout(timeout)
            probe_followup_socket.connect(str(socket_path))
            response, err = _backend_daemon_request_on_socket(
                probe_followup_socket,
                probe_request_bytes,
                shutdown_write=False,
            )
        except OSError as exc:
            if probe_followup_socket is not None:
                with contextlib.suppress(OSError):
                    probe_followup_socket.close()
            return _BackendDaemonCompileResult(
                False,
                f"backend daemon connection failed: {exc}",
                None,
                None,
                None,
                True,
                False,
            )
    else:
        response, err = _backend_daemon_request_bytes(
            socket_path, full_request_bytes, timeout=timeout
        )
    if err is not None:
        if probe_followup_socket is not None:
            with contextlib.suppress(OSError):
                probe_followup_socket.close()
        return _BackendDaemonCompileResult(False, err, None, None, None, True, False)
    if response is None:
        if probe_followup_socket is not None:
            with contextlib.suppress(OSError):
                probe_followup_socket.close()
        return _BackendDaemonCompileResult(
            False,
            "backend daemon returned no response",
            None,
            None,
            None,
            True,
            False,
        )
    health = _backend_daemon_health_from_response(response)
    if not bool(response.get("ok")):
        error = response.get("error")
        if isinstance(error, str) and error:
            return _BackendDaemonCompileResult(
                False, error, health, None, None, True, False
            )
        return _BackendDaemonCompileResult(
            False,
            "backend daemon compile request failed",
            health,
            None,
            None,
            True,
            False,
        )
    response_jobs = response.get("jobs")
    if not isinstance(response_jobs, list) or not response_jobs:
        return _BackendDaemonCompileResult(
            False,
            "backend daemon response missing job results",
            health,
            None,
            None,
            True,
            False,
        )
    first = response_jobs[0]
    if not isinstance(first, dict):
        return _BackendDaemonCompileResult(
            False,
            "backend daemon response had malformed job payload",
            health,
            None,
            None,
            True,
            False,
        )
    cached: bool | None = (
        first.get("cached") if isinstance(first.get("cached"), bool) else None
    )
    raw_tier = first.get("cache_tier")
    cache_tier = (
        raw_tier.strip() if isinstance(raw_tier, str) and raw_tier.strip() else None
    )
    output_written = (
        first.get("output_written")
        if isinstance(first.get("output_written"), bool)
        else True
    )
    needs_ir = bool(first.get("needs_ir"))
    output_exists = not output_written
    if needs_ir and probe_request_bytes is not None:
        if full_request_bytes is None:
            full_request_bytes, encode_err = _backend_daemon_compile_request_bytes(
                ir=ir,
                backend_output=backend_output,
                is_wasm=is_wasm,
                target_triple=target_triple,
                cache_key=cache_key,
                function_cache_key=function_cache_key,
                config_digest=config_digest,
                skip_module_output_if_synced=skip_module_output_if_synced,
                skip_function_output_if_synced=skip_function_output_if_synced,
                include_health=False,
            )
            if encode_err is not None:
                return _BackendDaemonCompileResult(
                    False, encode_err, health, None, None, True, False
                )
            assert full_request_bytes is not None
        assert probe_followup_socket is not None
        response, err = _backend_daemon_request_on_socket(
            probe_followup_socket,
            full_request_bytes,
            shutdown_write=True,
        )
        if err is not None:
            with contextlib.suppress(OSError):
                probe_followup_socket.close()
            return _BackendDaemonCompileResult(
                False, err, health, None, None, True, False
            )
        if response is None:
            with contextlib.suppress(OSError):
                probe_followup_socket.close()
            return _BackendDaemonCompileResult(
                False,
                "backend daemon returned no response",
                health,
                None,
                None,
                True,
                False,
            )
        health = _backend_daemon_health_from_response(response)
        if not bool(response.get("ok")):
            error = response.get("error")
            if isinstance(error, str) and error:
                return _BackendDaemonCompileResult(
                    False, error, health, None, None, True, False
                )
            return _BackendDaemonCompileResult(
                False,
                "backend daemon compile request failed",
                health,
                None,
                None,
                True,
                False,
            )
        response_jobs = response.get("jobs")
        if not isinstance(response_jobs, list) or not response_jobs:
            return _BackendDaemonCompileResult(
                False,
                "backend daemon response missing job results",
                health,
                None,
                None,
                True,
                False,
            )
        first = response_jobs[0]
        if not isinstance(first, dict):
            return _BackendDaemonCompileResult(
                False,
                "backend daemon response had malformed job payload",
                health,
                None,
                None,
                True,
                False,
            )
        cached = first.get("cached") if isinstance(first.get("cached"), bool) else None
        raw_tier = first.get("cache_tier")
        cache_tier = (
            raw_tier.strip() if isinstance(raw_tier, str) and raw_tier.strip() else None
        )
        output_written = (
            first.get("output_written")
            if isinstance(first.get("output_written"), bool)
            else True
        )
        output_exists = not output_written
    if probe_followup_socket is not None:
        with contextlib.suppress(OSError):
            probe_followup_socket.close()
    if not bool(first.get("ok")):
        message = first.get("message")
        if isinstance(message, str) and message:
            return _BackendDaemonCompileResult(
                False, message, health, cached, cache_tier, output_written, False
            )
        return _BackendDaemonCompileResult(
            False,
            "backend daemon failed to compile job",
            health,
            cached,
            cache_tier,
            output_written,
            False,
        )
    if output_written and not backend_output.exists():
        return _BackendDaemonCompileResult(
            False,
            "backend daemon reported success but output is missing",
            health,
            cached,
            cache_tier,
            output_written,
            False,
        )
    output_exists = True
    return _BackendDaemonCompileResult(
        True,
        None,
        health,
        cached,
        cache_tier,
        output_written,
        output_exists,
    )


def _backend_fingerprint_path(
    project_root: Path,
    artifact: Path,
    cargo_profile: str,
) -> Path:
    return _artifact_state_path(
        project_root,
        artifact,
        subdir="backend_fingerprints",
        stem_suffix=f"{cargo_profile}",
        extension="fingerprint",
    )


def _link_fingerprint_path(
    project_root: Path,
    artifact: Path,
    profile: BuildProfile,
    target_triple: str | None,
) -> Path:
    target = (target_triple or "native").replace(os.sep, "_").replace(":", "_")
    return _artifact_state_path(
        project_root,
        artifact,
        subdir="link_fingerprints",
        stem_suffix=f"{profile}.{target}",
        extension="fingerprint",
    )


def _artifact_sync_state_path(project_root: Path, artifact: Path) -> Path:
    return _artifact_state_path(
        project_root,
        artifact,
        subdir="artifact_sync",
        stem_suffix="",
        extension="json",
    )


def _read_artifact_sync_state(path: Path) -> dict[str, Any] | None:
    try:
        stat = path.stat()
    except OSError:
        _ARTIFACT_SYNC_STATE_CACHE.pop(path, None)
        return None
    cached = _ARTIFACT_SYNC_STATE_CACHE.get(path)
    if cached is not None:
        cached_size, cached_mtime_ns, cached_payload = cached
        if cached_size == stat.st_size and cached_mtime_ns == stat.st_mtime_ns:
            return cached_payload
    try:
        text = path.read_text().strip()
    except OSError:
        _ARTIFACT_SYNC_STATE_CACHE.pop(path, None)
        return None
    if not text:
        _ARTIFACT_SYNC_STATE_CACHE[path] = (stat.st_size, stat.st_mtime_ns, None)
        return None
    try:
        data = json.loads(text)
    except json.JSONDecodeError:
        _ARTIFACT_SYNC_STATE_CACHE[path] = (stat.st_size, stat.st_mtime_ns, None)
        return None
    payload = data if isinstance(data, dict) else None
    _ARTIFACT_SYNC_STATE_CACHE[path] = (stat.st_size, stat.st_mtime_ns, payload)
    return payload


def _read_cached_json_object(path: Path) -> dict[str, Any] | None:
    try:
        stat = path.stat()
    except OSError:
        _PERSISTED_JSON_OBJECT_CACHE.pop(path, None)
        return None
    cached = _PERSISTED_JSON_OBJECT_CACHE.get(path)
    if cached is not None:
        cached_size, cached_mtime_ns, cached_payload = cached
        if cached_size == stat.st_size and cached_mtime_ns == stat.st_mtime_ns:
            return cached_payload
    try:
        data = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        _PERSISTED_JSON_OBJECT_CACHE[path] = (stat.st_size, stat.st_mtime_ns, None)
        return None
    payload = data if isinstance(data, dict) else None
    _PERSISTED_JSON_OBJECT_CACHE[path] = (
        stat.st_size,
        stat.st_mtime_ns,
        payload,
    )
    return payload


def _write_artifact_sync_state(
    path: Path,
    *,
    source_key: str,
    tier: str,
    artifact: Path,
) -> None:
    stat = artifact.stat()
    payload = {
        "version": 1,
        "source_key": source_key,
        "tier": tier,
        "size": stat.st_size,
        "mtime_ns": stat.st_mtime_ns,
    }
    path.write_text(json.dumps(payload, indent=2) + "\n")
    try:
        written_stat = path.stat()
    except OSError:
        _ARTIFACT_SYNC_STATE_CACHE.pop(path, None)
    else:
        _ARTIFACT_SYNC_STATE_CACHE[path] = (
            written_stat.st_size,
            written_stat.st_mtime_ns,
            dict(payload),
        )


def _write_cached_json_object(
    path: Path,
    payload: dict[str, Any],
    *,
    default: Any | None = None,
) -> None:
    text = json.dumps(payload, indent=2, default=default) + "\n"
    path.write_text(text)
    try:
        written_stat = path.stat()
    except OSError:
        _PERSISTED_JSON_OBJECT_CACHE.pop(path, None)
    else:
        _PERSISTED_JSON_OBJECT_CACHE[path] = (
            written_stat.st_size,
            written_stat.st_mtime_ns,
            copy.deepcopy(payload),
        )


def _write_artifact_sync_payload(
    path: Path,
    payload: dict[str, Any],
    *,
    default: Any | None = None,
) -> None:
    text = json.dumps(payload, indent=2, default=default) + "\n"
    path.write_text(text)
    try:
        written_stat = path.stat()
    except OSError:
        _ARTIFACT_SYNC_STATE_CACHE.pop(path, None)
    else:
        _ARTIFACT_SYNC_STATE_CACHE[path] = (
            written_stat.st_size,
            written_stat.st_mtime_ns,
            dict(payload),
        )


def _artifact_sync_state_matches(
    state: dict[str, Any] | None,
    *,
    source_key: str,
    tier: str,
    artifact: Path,
) -> bool:
    try:
        stat = artifact.stat()
    except OSError:
        return False
    return _artifact_sync_state_matches_stat(
        state,
        source_key=source_key,
        tier=tier,
        stat=stat,
    )


def _artifact_sync_state_matches_stat(
    state: dict[str, Any] | None,
    *,
    source_key: str,
    tier: str,
    stat: os.stat_result,
) -> bool:
    if state is None:
        return False
    if state.get("source_key") != source_key or state.get("tier") != tier:
        return False
    return (
        state.get("size") == stat.st_size and state.get("mtime_ns") == stat.st_mtime_ns
    )


def _import_scan_cache_path(
    project_root: Path,
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    include_nested: bool,
) -> Path:
    root = _build_state_subdir_cached(
        os.fspath(_build_state_root(project_root)),
        "import_scan_cache",
    )
    cache_key = _resolved_module_cache_key(
        os.fspath(path),
        module_name,
        "pkg" if is_package else "mod",
        "nested" if include_nested else "top",
    )
    return root / f"{path.stem}.{cache_key}.json"


def _module_analysis_cache_path(
    project_root: Path,
    path: Path,
    *,
    kind: str = "module_analysis_cache",
    module_name: str,
    is_package: bool | None = None,
) -> Path:
    root = _build_state_subdir_cached(
        os.fspath(_build_state_root(project_root)),
        kind,
    )
    package_kind = "pkg" if is_package else "mod" if is_package is not None else "-"
    cache_key = _resolved_module_cache_key(
        os.fspath(path),
        module_name,
        package_kind,
        kind,
    )
    return root / f"{path.stem}.{cache_key}.json"


def _module_lowering_cache_path(
    project_root: Path,
    path: Path,
    *,
    module_name: str,
    is_package: bool,
) -> Path:
    root = _build_state_subdir_cached(
        os.fspath(_build_state_root(project_root)),
        "module_lowering_cache",
    )
    cache_key = _resolved_module_cache_key(
        os.fspath(path),
        module_name,
        "pkg" if is_package else "mod",
    )
    return root / f"{path.stem}.{cache_key}.json"


def _module_graph_cache_path(
    project_root: Path,
    entry_path: Path,
    *,
    roots: list[Path],
    module_roots: list[Path],
    stdlib_root: Path,
    skip_modules: set[str],
    stub_parents: set[str],
    nested_stdlib_scan_modules: set[str],
) -> Path:
    root = _build_state_subdir_cached(
        os.fspath(_build_state_root(project_root)),
        "module_graph_cache",
    )
    cache_key = _module_graph_cache_key(
        os.fspath(entry_path),
        tuple(os.fspath(path) for path in roots),
        tuple(os.fspath(path) for path in module_roots),
        os.fspath(stdlib_root),
        tuple(sorted(skip_modules)),
        tuple(sorted(stub_parents)),
        tuple(sorted(nested_stdlib_scan_modules)),
    )
    return root / f"{entry_path.stem}.{cache_key}.json"


def _read_persisted_module_graph(
    project_root: Path,
    entry_path: Path,
    *,
    roots: list[Path],
    module_roots: list[Path],
    stdlib_root: Path,
    skip_modules: set[str],
    stub_parents: set[str],
    nested_stdlib_scan_modules: set[str],
    resolution_cache: _ModuleResolutionCache | None = None,
) -> _PersistedModuleGraphState | None:
    cache_path = _module_graph_cache_path(
        project_root,
        entry_path,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        skip_modules=skip_modules,
        stub_parents=stub_parents,
        nested_stdlib_scan_modules=nested_stdlib_scan_modules,
    )
    payload = _read_cached_json_object(cache_path)
    if payload is None:
        return None
    if not isinstance(payload, dict) or payload.get("version") != 1:
        return None
    raw_modules = payload.get("modules")
    if not isinstance(raw_modules, list):
        return None
    graph: dict[str, Path] = {}
    dirty_modules: set[str] = set()
    for item in raw_modules:
        if not isinstance(item, dict):
            return None
        module_name = item.get("module")
        path_text = item.get("path")
        size = item.get("size")
        mtime_ns = item.get("mtime_ns")
        if (
            not isinstance(module_name, str)
            or not isinstance(path_text, str)
            or not isinstance(size, int)
            or not isinstance(mtime_ns, int)
        ):
            return None
        path = Path(path_text)
        try:
            stat = (
                resolution_cache.path_stat(path)
                if resolution_cache is not None
                else path.stat()
            )
        except OSError:
            dirty_modules.add(module_name)
            graph[module_name] = path
            continue
        if stat.st_size != size or stat.st_mtime_ns != mtime_ns:
            dirty_modules.add(module_name)
        graph[module_name] = path
    raw_explicit_imports = payload.get("explicit_imports", [])
    if not isinstance(raw_explicit_imports, list) or not all(
        isinstance(name, str) for name in raw_explicit_imports
    ):
        return None
    return _PersistedModuleGraphState(
        graph=graph,
        explicit_imports=set(cast(list[str], raw_explicit_imports)),
        dirty_modules=dirty_modules,
    )


def _write_persisted_module_graph(
    project_root: Path,
    entry_path: Path,
    *,
    roots: list[Path],
    module_roots: list[Path],
    stdlib_root: Path,
    skip_modules: set[str],
    stub_parents: set[str],
    nested_stdlib_scan_modules: set[str],
    graph: dict[str, Path],
    explicit_imports: set[str],
) -> None:
    modules: list[dict[str, Any]] = []
    for module_name, path in sorted(graph.items()):
        stat = path.stat()
        modules.append(
            {
                "module": module_name,
                "path": str(path),
                "size": stat.st_size,
                "mtime_ns": stat.st_mtime_ns,
            }
        )
    payload = {
        "version": 1,
        "modules": modules,
        "explicit_imports": sorted(explicit_imports),
    }
    cache_path = _module_graph_cache_path(
        project_root,
        entry_path,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        skip_modules=skip_modules,
        stub_parents=stub_parents,
        nested_stdlib_scan_modules=nested_stdlib_scan_modules,
    )
    cache_path.parent.mkdir(parents=True, exist_ok=True)
    _write_cached_json_object(cache_path, payload)


def _read_persisted_import_scan(
    project_root: Path,
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    include_nested: bool,
    path_stat: os.stat_result | None = None,
) -> tuple[str, ...] | None:
    cache_path = _import_scan_cache_path(
        project_root,
        path,
        module_name=module_name,
        is_package=is_package,
        include_nested=include_nested,
    )
    payload = _read_artifact_sync_state(cache_path)
    if payload is None:
        return None
    if path_stat is None:
        try:
            path_stat = path.stat()
        except OSError:
            return None
    imports = payload.get("imports")
    if not isinstance(imports, list) or not all(
        isinstance(item, str) for item in imports
    ):
        return None
    if (
        payload.get("size") != path_stat.st_size
        or payload.get("mtime_ns") != path_stat.st_mtime_ns
    ):
        return None
    return tuple(imports)


def _write_persisted_import_scan(
    project_root: Path,
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    include_nested: bool,
    imports: Iterable[str],
) -> None:
    cache_path = _import_scan_cache_path(
        project_root,
        path,
        module_name=module_name,
        is_package=is_package,
        include_nested=include_nested,
    )
    stat = path.stat()
    payload = {
        "version": 1,
        "module_name": module_name,
        "is_package": is_package,
        "include_nested": include_nested,
        "size": stat.st_size,
        "mtime_ns": stat.st_mtime_ns,
        "imports": list(imports),
    }
    cache_path.parent.mkdir(parents=True, exist_ok=True)
    _write_artifact_sync_payload(cache_path, payload)


def _read_persisted_module_analysis(
    project_root: Path,
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    path_stat: os.stat_result | None = None,
    validate_stat: bool = True,
) -> tuple[dict[str, dict[str, Any]], tuple[str, ...] | None] | None:
    cache_path = _module_analysis_cache_path(
        project_root,
        path,
        module_name=module_name,
        is_package=is_package,
    )
    payload = _read_artifact_sync_state(cache_path)
    if payload is None:
        return None
    raw_defaults = payload.get("func_defaults")
    if not isinstance(raw_defaults, dict):
        return None
    if validate_stat:
        if path_stat is None:
            try:
                path_stat = path.stat()
            except OSError:
                return None
        if (
            payload.get("size") != path_stat.st_size
            or payload.get("mtime_ns") != path_stat.st_mtime_ns
        ):
            return None
    cached_imports: tuple[str, ...] | None = None
    raw_imports = payload.get("imports")
    if raw_imports is not None:
        if not isinstance(raw_imports, list) or not all(
            isinstance(item, str) for item in raw_imports
        ):
            return None
        cached_imports = tuple(raw_imports)

    normalized: dict[str, dict[str, Any]] = {}
    for func_name, func_payload in raw_defaults.items():
        if not isinstance(func_name, str) or not isinstance(func_payload, dict):
            return None
        normalized[func_name] = cast(
            dict[str, Any], _decode_cached_json_value(func_payload)
        )
    return normalized, cached_imports


def _write_persisted_module_analysis(
    project_root: Path,
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    func_defaults: dict[str, dict[str, Any]],
    imports: Iterable[str] | None = None,
) -> None:
    cache_path = _module_analysis_cache_path(
        project_root,
        path,
        module_name=module_name,
        is_package=is_package,
    )
    stat = path.stat()
    payload = {
        "version": 1,
        "module_name": module_name,
        "is_package": is_package,
        "size": stat.st_size,
        "mtime_ns": stat.st_mtime_ns,
        "func_defaults": func_defaults,
    }
    if imports is not None:
        payload["imports"] = list(imports)
    cache_path.parent.mkdir(parents=True, exist_ok=True)
    _write_artifact_sync_payload(cache_path, payload, default=_json_ir_default)


def _decode_cached_json_value(value: Any) -> Any:
    if isinstance(value, list):
        return [_decode_cached_json_value(item) for item in value]
    if isinstance(value, dict):
        if value.get("__ellipsis__") is True and len(value) == 1:
            return Ellipsis
        if "__complex__" in value and isinstance(value["__complex__"], list):
            real_imag = value["__complex__"]
            if len(real_imag) == 2:
                return complex(real_imag[0], real_imag[1])
        if "__tuple__" in value and isinstance(value["__tuple__"], list):
            return tuple(_decode_cached_json_value(item) for item in value["__tuple__"])
        if "__ast__" in value and isinstance(value["__ast__"], str):
            return value["__ast__"]
        if "__set__" in value and isinstance(value["__set__"], list):
            return set(_decode_cached_json_value(item) for item in value["__set__"])
        if "__molt_value__" in value and isinstance(value["__molt_value__"], dict):
            payload = value["__molt_value__"]
            name = payload.get("name")
            type_hint = payload.get("type_hint", "Unknown")
            if isinstance(name, str) and isinstance(type_hint, str):
                return MoltValue(name=name, type_hint=type_hint)
        return {
            str(key): _decode_cached_json_value(item) for key, item in value.items()
        }
    return value


def _load_module_imports(
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    include_nested: bool,
    tree: ast.AST,
    resolution_cache: _ModuleResolutionCache,
    project_root: Path | None,
) -> tuple[str, ...]:
    if project_root is not None:
        persisted_imports = _read_persisted_import_scan(
            project_root,
            path,
            module_name=module_name,
            is_package=is_package,
            include_nested=include_nested,
        )
        if persisted_imports is not None:
            return persisted_imports
    imports = resolution_cache.collect_imports(
        path,
        tree,
        module_name=module_name,
        is_package=is_package,
        include_nested=include_nested,
    )
    if project_root is not None:
        with contextlib.suppress(OSError):
            _write_persisted_import_scan(
                project_root,
                path,
                module_name=module_name,
                is_package=is_package,
                include_nested=include_nested,
                imports=imports,
            )
    return imports


def _load_module_analysis(
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    include_nested: bool,
    source: str | None,
    logical_source_path: str,
    resolution_cache: _ModuleResolutionCache,
    project_root: Path | None,
    path_stat: os.stat_result | None = None,
) -> tuple[
    ast.AST | None,
    tuple[str, ...],
    dict[str, dict[str, Any]],
    str | None,
    bool,
    bool,
    os.stat_result | None,
]:
    if path_stat is None and project_root is not None:
        with contextlib.suppress(OSError):
            path_stat = resolution_cache.path_stat(path)
    persisted_analysis = (
        _read_persisted_module_analysis(
            project_root,
            path,
            module_name=module_name,
            is_package=is_package,
            path_stat=path_stat,
        )
        if project_root is not None
        else None
    )
    stale_analysis = (
        _read_persisted_module_analysis(
            project_root,
            path,
            module_name=module_name,
            is_package=is_package,
            validate_stat=False,
        )
        if project_root is not None
        else None
    )
    persisted_defaults = (
        persisted_analysis[0] if persisted_analysis is not None else None
    )
    persisted_imports_from_analysis = (
        persisted_analysis[1] if persisted_analysis is not None else None
    )
    persisted_imports = persisted_imports_from_analysis
    if persisted_imports is None and project_root is not None:
        persisted_imports = _read_persisted_import_scan(
            project_root,
            path,
            module_name=module_name,
            is_package=is_package,
            include_nested=include_nested,
            path_stat=path_stat,
        )
    if persisted_imports is not None and persisted_defaults is not None:
        return None, persisted_imports, persisted_defaults, None, True, False, path_stat

    if source is None:
        source = resolution_cache.read_module_source(path)

    tree = resolution_cache.parse_module_ast(path, source, filename=logical_source_path)
    imports = persisted_imports
    if imports is None:
        imports = _load_module_imports(
            path,
            module_name=module_name,
            is_package=is_package,
            include_nested=include_nested,
            tree=tree,
            resolution_cache=resolution_cache,
            project_root=project_root,
        )
    func_defaults = persisted_defaults
    if func_defaults is None:
        func_defaults = _collect_func_defaults(tree)
        if project_root is not None:
            with contextlib.suppress(OSError):
                _write_persisted_module_analysis(
                    project_root,
                    path,
                    module_name=module_name,
                    is_package=is_package,
                    func_defaults=func_defaults,
                    imports=imports,
                )
    interface_changed = True
    if stale_analysis is not None:
        stale_defaults, stale_imports = stale_analysis
        if (
            stale_imports is not None
            and stale_imports == imports
            and stale_defaults == func_defaults
        ):
            interface_changed = False
    return tree, imports, func_defaults, source, False, interface_changed, path_stat


def _module_frontend_payload(
    gen: SimpleTIRGenerator,
    ir: dict[str, Any],
    *,
    visit_s: float,
    lower_s: float,
    total_s: float,
) -> dict[str, Any]:
    return {
        "functions": ir["functions"],
        "func_code_ids": dict(gen.func_code_ids),
        "local_class_names": sorted(gen.local_class_names),
        "local_classes": {
            class_name: gen.classes[class_name]
            for class_name in sorted(gen.local_class_names)
        },
        "midend_policy_outcomes_by_function": dict(
            gen.midend_policy_outcomes_by_function
        ),
        "midend_pass_stats_by_function": dict(gen.midend_pass_stats_by_function),
        "timings": {
            "visit_s": visit_s,
            "lower_s": lower_s,
            "total_s": total_s,
        },
    }


def _module_frontend_generator(
    *,
    module_name: str,
    logical_source_path: str,
    entry_override: str | None,
    module_is_namespace: bool,
    parse_codec: ParseCodec,
    type_hint_policy: TypeHintPolicy,
    fallback_policy: FallbackPolicy,
    enable_phi: bool,
    stdlib_allowlist: Collection[str],
    module_chunking: bool,
    module_chunk_max_ops: int,
    optimization_profile: str,
    scoped_inputs: _ScopedLoweringInputView,
    scoped_known_classes: dict[str, Any],
) -> SimpleTIRGenerator:
    return SimpleTIRGenerator(
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
        source_path=logical_source_path,
        type_facts=scoped_inputs.type_facts,
        module_name=module_name,
        module_is_namespace=module_is_namespace,
        entry_module=entry_override,
        enable_phi=enable_phi,
        known_modules=scoped_inputs.known_modules_set,
        known_classes=scoped_known_classes,
        stdlib_allowlist=stdlib_allowlist,
        known_func_defaults=scoped_inputs.known_func_defaults,
        module_chunking=module_chunking,
        module_chunk_max_ops=module_chunk_max_ops,
        optimization_profile=optimization_profile,
        pgo_hot_functions=scoped_inputs.pgo_hot_function_names_set,
    )


def _known_classes_snapshot_copy(known_classes: Mapping[str, Any]) -> dict[str, Any]:
    if not known_classes:
        return {}
    return dict(known_classes)


def _summarize_worker_timing_items(
    items: Sequence[Mapping[str, Any]],
) -> _WorkerTimingSummary:
    queue_samples = [float(item.get("queue_ms", 0.0)) for item in items]
    wait_samples = [float(item.get("wait_ms", 0.0)) for item in items]
    exec_samples = [float(item.get("exec_ms", 0.0)) for item in items]
    roundtrip_samples = [float(item.get("roundtrip_ms", 0.0)) for item in items]
    return _WorkerTimingSummary(
        count=len(items),
        queue_ms_total=round(sum(queue_samples), 6),
        queue_ms_max=round(max(queue_samples, default=0.0), 6),
        wait_ms_total=round(sum(wait_samples), 6),
        wait_ms_max=round(max(wait_samples, default=0.0), 6),
        exec_ms_total=round(sum(exec_samples), 6),
        exec_ms_max=round(max(exec_samples, default=0.0), 6),
        roundtrip_ms_total=round(sum(roundtrip_samples), 6),
    )


def _frontend_parallel_layer_detail(
    *,
    layer_index: int,
    mode: str,
    policy_reason: str,
    module_count: int,
    candidate_count: int,
    workers: int,
    cache_hits: int,
    predicted_cost_total: float,
    effective_min_predicted_cost: float,
    stdlib_candidates: int,
    target_cost_per_worker: float,
    timing_summary: _WorkerTimingSummary,
    started_ns: int,
    finished_ns: int,
    fallback_reason: str | None = None,
) -> dict[str, Any]:
    detail: dict[str, Any] = {
        "index": layer_index,
        "mode": mode,
        "policy_reason": policy_reason,
        "module_count": module_count,
        "candidate_count": candidate_count,
        "workers": workers,
        "cache_hits": cache_hits,
        "predicted_cost_total": round(predicted_cost_total, 3),
        "effective_min_predicted_cost": round(effective_min_predicted_cost, 3),
        "stdlib_candidates": stdlib_candidates,
        "target_cost_per_worker": round(target_cost_per_worker, 3),
        "queue_ms_total": timing_summary.queue_ms_total,
        "queue_ms_max": timing_summary.queue_ms_max,
        "wait_ms_total": timing_summary.wait_ms_total,
        "wait_ms_max": timing_summary.wait_ms_max,
        "exec_ms_total": timing_summary.exec_ms_total,
        "exec_ms_max": timing_summary.exec_ms_max,
        "roundtrip_ms_total": timing_summary.roundtrip_ms_total,
        "elapsed_ms": _duration_ms_from_ns(started_ns, finished_ns),
    }
    if fallback_reason:
        detail["fallback_reason"] = fallback_reason
    return detail


def _frontend_result_timings(result: Mapping[str, Any]) -> _FrontendModuleResultTimings:
    timings = cast(Mapping[str, Any], result.get("timings", {}))
    return _FrontendModuleResultTimings(
        visit_s=float(timings.get("visit_s", 0.0)),
        lower_s=float(timings.get("lower_s", 0.0)),
        total_s=float(timings.get("total_s", 0.0)),
    )


def _frontend_layer_policy_summary(
    layer_policy: Mapping[str, Any],
    *,
    default_min_predicted_cost: float,
) -> _FrontendLayerPolicySummary:
    return _FrontendLayerPolicySummary(
        enabled=bool(layer_policy.get("enabled")),
        workers=int(layer_policy.get("workers", 1)),
        reason=str(layer_policy.get("reason", "serial")),
        predicted_cost_total=float(layer_policy.get("predicted_cost_total", 0.0)),
        effective_min_predicted_cost=float(
            layer_policy.get(
                "effective_min_predicted_cost",
                default_min_predicted_cost,
            )
        ),
        stdlib_candidates=int(layer_policy.get("stdlib_candidates", 0)),
    )


def _record_parallel_cached_module_result(
    layer_state: _FrontendParallelLayerState,
    module_name: str,
    cached_result: Mapping[str, Any],
) -> None:
    timings = cast(Mapping[str, Any], cached_result.get("timings", {}))
    total_ms = float(timings.get("total_s", 0.0)) * 1000.0
    layer_state.results[module_name] = {"ok": True, **cached_result}
    layer_state.worker_timings_by_module[module_name] = {
        "mode": "parallel_cache_hit",
        "queue_ms": 0.0,
        "wait_ms": 0.0,
        "exec_ms": round(max(0.0, total_ms), 6),
        "roundtrip_ms": round(max(0.0, total_ms), 6),
        "worker_pid": None,
    }


def _record_parallel_worker_result(
    layer_state: _FrontendParallelLayerState,
    *,
    module_name: str,
    result: Mapping[str, Any],
    submitted_ns: int,
    received_ns: int,
) -> None:
    timings = cast(Mapping[str, Any], result.get("timings", {}))
    worker_meta = cast(Mapping[str, Any], result.get("worker", {}))
    worker_started_ns = worker_meta.get("started_ns")
    worker_finished_ns = worker_meta.get("finished_ns")
    exec_ms = float(timings.get("total_s", 0.0)) * 1000.0
    exec_from_ns = _duration_ms_from_ns(worker_started_ns, worker_finished_ns)
    if exec_from_ns > 0.0:
        exec_ms = exec_from_ns
    layer_state.results[module_name] = dict(result)
    layer_state.worker_timings_by_module[module_name] = {
        "mode": "parallel",
        "queue_ms": _duration_ms_from_ns(submitted_ns, worker_started_ns),
        "wait_ms": _duration_ms_from_ns(worker_finished_ns, received_ns),
        "exec_ms": round(max(0.0, exec_ms), 6),
        "roundtrip_ms": _duration_ms_from_ns(submitted_ns, received_ns),
        "worker_pid": worker_meta.get("pid"),
    }


def _resolve_frontend_parallel_config(
    *,
    module_count: int,
    has_back_edges: bool,
    frontend_phase_timeout: float | None,
) -> _FrontendParallelConfig:
    workers = _resolve_frontend_parallel_module_workers()
    min_modules = _resolve_frontend_parallel_min_modules()
    min_predicted_cost = _resolve_frontend_parallel_min_predicted_cost()
    target_cost_per_worker = _resolve_frontend_parallel_target_cost_per_worker()
    stdlib_min_cost_scale = _resolve_frontend_parallel_stdlib_min_cost_scale()
    enabled = False
    reason = "disabled"
    if workers < 2:
        reason = "workers<2"
    elif module_count < 2:
        reason = "module_count<2"
    elif has_back_edges:
        reason = "dependency_back_edge"
    elif frontend_phase_timeout is not None:
        reason = "phase_timeout_configured"
    else:
        enabled = True
        reason = "enabled"
    return _FrontendParallelConfig(
        workers=workers,
        min_modules=min_modules,
        min_predicted_cost=min_predicted_cost,
        target_cost_per_worker=target_cost_per_worker,
        stdlib_min_cost_scale=stdlib_min_cost_scale,
        enabled=enabled,
        reason=reason,
    )


def _frontend_parallel_policy_payload(
    config: _FrontendParallelConfig,
) -> dict[str, Any]:
    return {
        "min_modules": config.min_modules,
        "min_predicted_cost": round(config.min_predicted_cost, 3),
        "target_cost_per_worker": round(config.target_cost_per_worker, 3),
        "stdlib_min_cost_scale": round(config.stdlib_min_cost_scale, 3),
    }


def _frontend_layer_plan(
    layer: Sequence[str],
    *,
    syntax_error_modules: Mapping[str, Any],
    module_sources: dict[str, str],
    module_deps: dict[str, set[str]],
    frontend_module_costs: Mapping[str, float],
    stdlib_like_by_module: Mapping[str, bool],
    frontend_parallel_config: _FrontendParallelConfig,
    parallel_pool_usable: bool,
) -> _FrontendLayerPlan:
    candidates = tuple(name for name in layer if name not in syntax_error_modules)
    policy = _choose_frontend_parallel_layer_workers(
        candidates=list(candidates),
        module_sources=module_sources,
        module_deps=module_deps,
        module_costs=frontend_module_costs,
        stdlib_like_by_module=stdlib_like_by_module,
        max_workers=frontend_parallel_config.workers,
        min_modules=frontend_parallel_config.min_modules,
        min_predicted_cost=frontend_parallel_config.min_predicted_cost,
        target_cost_per_worker=frontend_parallel_config.target_cost_per_worker,
    )
    policy_summary = _frontend_layer_policy_summary(
        policy,
        default_min_predicted_cost=frontend_parallel_config.min_predicted_cost,
    )
    mode = "serial"
    policy_reason = policy_summary.reason
    workers = policy_summary.workers
    if parallel_pool_usable and policy_summary.enabled and len(candidates) > 1:
        mode = "parallel"
        workers = min(workers, len(candidates))
    elif len(candidates) > 1 and not parallel_pool_usable:
        mode = "serial_layer_policy"
        policy_reason = "pool_unavailable_after_error"
    return _FrontendLayerPlan(
        candidates=candidates,
        predicted_cost_total=policy_summary.predicted_cost_total,
        effective_min_predicted_cost=policy_summary.effective_min_predicted_cost,
        stdlib_candidates=policy_summary.stdlib_candidates,
        workers=workers,
        policy_reason=policy_reason,
        mode=mode,
    )


def _worker_timing_summary_payload(summary: _WorkerTimingSummary) -> dict[str, Any]:
    return {
        "count": summary.count,
        "queue_ms_total": summary.queue_ms_total,
        "queue_ms_max": summary.queue_ms_max,
        "wait_ms_total": summary.wait_ms_total,
        "wait_ms_max": summary.wait_ms_max,
        "exec_ms_total": summary.exec_ms_total,
        "exec_ms_max": summary.exec_ms_max,
    }


def _layer_cache_hit_count(items: Sequence[Mapping[str, Any]]) -> int:
    return sum(1 for item in items if item.get("mode") == "parallel_cache_hit")


def _frontend_layer_static_metrics(
    module_names: Sequence[str],
    *,
    frontend_module_costs: Mapping[str, float],
    stdlib_like_by_module: Mapping[str, bool],
) -> _FrontendLayerStaticMetrics:
    return _FrontendLayerStaticMetrics(
        predicted_cost_total=sum(frontend_module_costs.get(name, 0.0) for name in module_names),
        stdlib_candidates=sum(
            1 for name in module_names if stdlib_like_by_module.get(name, False)
        ),
    )


def _record_serial_frontend_worker_timing(
    *,
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]],
    recorded_worker_timings: list[dict[str, Any]],
    layer_index: int,
    module_name: str,
    module_path: Path,
    mode: str,
    total_s: float,
) -> None:
    total_ms = total_s * 1000.0
    recorded_worker_timings.append(
        record_frontend_parallel_worker_timing(
            layer_index=layer_index,
            module_name=module_name,
            module_path=module_path,
            mode=mode,
            queue_ms=0.0,
            wait_ms=0.0,
            exec_ms=total_ms,
            roundtrip_ms=total_ms,
            worker_pid=None,
        )
    )


def _append_frontend_parallel_layer_detail(
    frontend_parallel_layers: list[dict[str, Any]],
    *,
    layer_index: int,
    layer_mode: str,
    layer_policy_reason: str,
    module_names: Sequence[str],
    candidate_count: int,
    workers: int,
    timing_items: Sequence[Mapping[str, Any]],
    predicted_cost_total: float,
    effective_min_predicted_cost: float,
    stdlib_candidates: int,
    target_cost_per_worker: float,
    started_ns: int,
    finished_ns: int,
    fallback_reason: str | None = None,
) -> None:
    timing_summary = _summarize_worker_timing_items(timing_items)
    frontend_parallel_layers.append(
        _frontend_parallel_layer_detail(
            layer_index=layer_index,
            mode=layer_mode,
            policy_reason=layer_policy_reason,
            module_count=len(module_names),
            candidate_count=candidate_count,
            workers=workers,
            cache_hits=_layer_cache_hit_count(timing_items),
            predicted_cost_total=predicted_cost_total,
            effective_min_predicted_cost=effective_min_predicted_cost,
            stdlib_candidates=stdlib_candidates,
            target_cost_per_worker=target_cost_per_worker,
            timing_summary=timing_summary,
            started_ns=started_ns,
            finished_ns=finished_ns,
            fallback_reason=fallback_reason,
        )
    )


def _initialize_frontend_parallel_details(
    frontend_parallel_details: MutableMapping[str, Any],
    *,
    frontend_parallel_config: _FrontendParallelConfig,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    frontend_parallel_details["enabled"] = frontend_parallel_config.enabled
    frontend_parallel_details["workers"] = frontend_parallel_config.workers
    frontend_parallel_details["mode"] = (
        "process_pool_reused" if frontend_parallel_config.enabled else "serial"
    )
    frontend_parallel_details["reason"] = frontend_parallel_config.reason
    frontend_parallel_details["policy"] = _frontend_parallel_policy_payload(
        frontend_parallel_config
    )
    frontend_parallel_details["layers"] = []
    frontend_parallel_details["worker_timings"] = []
    return (
        cast(list[dict[str, Any]], frontend_parallel_details["layers"]),
        cast(list[dict[str, Any]], frontend_parallel_details["worker_timings"]),
    )


def _summarize_frontend_parallel_worker_timings(
    frontend_parallel_details: MutableMapping[str, Any],
    worker_timings: Sequence[Mapping[str, Any]],
) -> None:
    summary = _summarize_worker_timing_items(worker_timings)
    frontend_parallel_details["worker_summary"] = _worker_timing_summary_payload(summary)


def _append_frontend_serial_disabled_layer_detail(
    frontend_parallel_layers: list[dict[str, Any]],
    *,
    module_order: Sequence[str],
    serial_layer_state: _FrontendParallelLayerState,
    frontend_module_costs: Mapping[str, float],
    stdlib_like_by_module: Mapping[str, bool],
    frontend_parallel_config: _FrontendParallelConfig,
    serial_layer_started_ns: int,
) -> None:
    serial_static_metrics = _frontend_layer_static_metrics(
        module_order,
        frontend_module_costs=frontend_module_costs,
        stdlib_like_by_module=stdlib_like_by_module,
    )
    _append_frontend_parallel_layer_detail(
        frontend_parallel_layers,
        layer_index=0,
        layer_mode="serial_disabled",
        layer_policy_reason=frontend_parallel_config.reason,
        module_names=module_order,
        candidate_count=len(module_order),
        workers=1,
        timing_items=serial_layer_state.recorded_worker_timings,
        predicted_cost_total=serial_static_metrics.predicted_cost_total,
        effective_min_predicted_cost=frontend_parallel_config.min_predicted_cost,
        stdlib_candidates=serial_static_metrics.stdlib_candidates,
        target_cost_per_worker=frontend_parallel_config.target_cost_per_worker,
        started_ns=serial_layer_started_ns,
        finished_ns=time.time_ns(),
    )


def _resolve_tree_for_serial_frontend_module(
    module_name: str,
    module_path: Path,
    *,
    lowering_context: _SerialFrontendLoweringContext,
) -> ast.AST:
    if module_name in lowering_context.syntax_error_modules:
        return _syntax_error_stub_ast(lowering_context.syntax_error_modules[module_name])
    tree = lowering_context.module_trees.get(module_name)
    if tree is not None:
        return tree
    source = lowering_context.module_sources.get(module_name)
    if source is None:
        try:
            source = lowering_context.module_resolution_cache.read_module_source(module_path)
        except (SyntaxError, UnicodeDecodeError) as exc:
            raise _ModuleLowerError(f"Syntax error in {module_path}: {exc}") from exc
        except OSError as exc:
            raise _ModuleLowerError(f"Failed to read module {module_path}: {exc}") from exc
    logical_source_path = lowering_context.generated_module_source_paths.get(
        module_name, str(module_path)
    )
    try:
        return lowering_context.module_resolution_cache.parse_module_ast(
            module_path, source, filename=logical_source_path
        )
    except SyntaxError as exc:
        raise _ModuleLowerError(f"Syntax error in {module_path}: {exc}") from exc


def _lower_module_serial_with_context(
    module_name: str,
    module_path: Path,
    *,
    lowering_context: _SerialFrontendLoweringContext,
) -> tuple[dict[str, Any], float, float, float]:
    execution_view = _module_lowering_execution_view(
        module_name,
        module_path=module_path,
        module_graph_metadata=lowering_context.module_graph_metadata,
        module_deps=lowering_context.module_deps,
        known_modules=lowering_context.known_modules,
        known_func_defaults=lowering_context.known_func_defaults,
        pgo_hot_function_names=lowering_context.pgo_hot_function_names,
        type_facts=cast(TypeFacts | None, lowering_context.type_facts),
        known_classes_snapshot=lowering_context.known_classes,
        module_dep_closures=lowering_context.module_dep_closures,
        path_stat_by_module=lowering_context.module_path_stats,
        scoped_lowering_inputs=lowering_context.scoped_lowering_inputs,
        known_modules_sorted=lowering_context.known_modules_sorted,
        pgo_hot_function_names_sorted=lowering_context.pgo_hot_function_names_sorted,
    )
    metadata_view = execution_view.metadata
    scoped_inputs = execution_view.scoped_inputs
    logical_source_path = metadata_view.logical_source_path
    entry_override = metadata_view.entry_override
    is_package = metadata_view.is_package
    module_is_namespace = metadata_view.module_is_namespace
    path_stat = metadata_view.path_stat
    if path_stat is None:
        with contextlib.suppress(OSError):
            path_stat = lowering_context.module_resolution_cache.path_stat(module_path)
    scoped_known_classes = execution_view.scoped_known_classes
    context_digest: str | None = None
    if lowering_context.project_root is not None:
        context_digest = _module_lowering_context_digest_for_module(
            module_name,
            module_path,
            logical_source_path=logical_source_path,
            entry_override=entry_override,
            known_classes_snapshot=lowering_context.known_classes,
            parse_codec=lowering_context.parse_codec,
            type_hint_policy=lowering_context.type_hint_policy,
            fallback_policy=lowering_context.fallback_policy,
            type_facts=lowering_context.type_facts,
            enable_phi=lowering_context.enable_phi,
            known_modules=lowering_context.known_modules,
            stdlib_allowlist=lowering_context.stdlib_allowlist,
            known_func_defaults=lowering_context.known_func_defaults,
            module_deps=lowering_context.module_deps,
            module_is_namespace=module_is_namespace,
            module_chunking=lowering_context.module_chunking,
            module_chunk_max_ops=lowering_context.module_chunk_max_ops,
            optimization_profile=lowering_context.optimization_profile,
            pgo_hot_function_names=lowering_context.pgo_hot_function_names,
            known_modules_sorted=lowering_context.known_modules_sorted,
            stdlib_allowlist_sorted=lowering_context.stdlib_allowlist_sorted,
            pgo_hot_function_names_sorted=lowering_context.pgo_hot_function_names_sorted,
            module_dep_closures=lowering_context.module_dep_closures,
            scoped_lowering_inputs=lowering_context.scoped_lowering_inputs,
            scoped_inputs=scoped_inputs,
            scoped_known_classes=scoped_known_classes,
            is_package=is_package,
            path_stat=path_stat,
        )
        if (
            context_digest is not None
            and module_name not in lowering_context.dirty_lowering_modules
        ):
            cached_payload = _read_persisted_module_lowering(
                lowering_context.project_root,
                module_path,
                module_name=module_name,
                is_package=is_package,
                context_digest=context_digest,
                path_stat=path_stat,
            )
            if cached_payload is not None:
                return cached_payload, 0.0, 0.0, 0.0

    tree = _resolve_tree_for_serial_frontend_module(
        module_name,
        module_path,
        lowering_context=lowering_context,
    )
    gen = _module_frontend_generator(
        module_name=module_name,
        logical_source_path=logical_source_path,
        entry_override=entry_override,
        module_is_namespace=module_is_namespace,
        parse_codec=lowering_context.parse_codec,
        type_hint_policy=lowering_context.type_hint_policy,
        fallback_policy=lowering_context.fallback_policy,
        enable_phi=lowering_context.enable_phi,
        stdlib_allowlist=lowering_context.stdlib_allowlist,
        module_chunking=lowering_context.module_chunking,
        module_chunk_max_ops=lowering_context.module_chunk_max_ops,
        optimization_profile=lowering_context.optimization_profile,
        scoped_inputs=scoped_inputs,
        scoped_known_classes=scoped_known_classes,
    )
    module_frontend_start = time.perf_counter()
    visit_s = 0.0
    lower_s = 0.0
    try:
        visit_start = time.perf_counter()
        with _phase_timeout(
            lowering_context.frontend_phase_timeout,
            phase_name=f"frontend visit ({module_name})",
        ):
            gen.visit(tree)
        visit_s = time.perf_counter() - visit_start
        lower_start = time.perf_counter()
        with _phase_timeout(
            lowering_context.frontend_phase_timeout,
            phase_name=f"frontend IR lowering ({module_name})",
        ):
            ir = gen.to_json()
        lower_s = time.perf_counter() - lower_start
    except TimeoutError as exc:
        raise _ModuleLowerError(str(exc), timed_out=True) from exc
    except CompatibilityError as exc:
        raise _ModuleLowerError(str(exc)) from exc
    total_s = time.perf_counter() - module_frontend_start
    payload = _module_frontend_payload(
        gen,
        ir,
        visit_s=visit_s,
        lower_s=lower_s,
        total_s=total_s,
    )
    if lowering_context.project_root is not None and context_digest is not None:
        with contextlib.suppress(OSError):
            _write_persisted_module_lowering(
                lowering_context.project_root,
                module_path,
                module_name=module_name,
                is_package=is_package,
                context_digest=context_digest,
                result=payload,
            )
    return payload, visit_s, lower_s, total_s


def _run_serial_frontend_lower_with_context(
    module_name: str,
    module_path: Path,
    *,
    lowering_context: _SerialFrontendLoweringContext,
    lowering_hooks: _SerialFrontendLoweringHooks,
) -> tuple[dict[str, Any] | None, _FrontendModuleResultTimings | None, dict[str, Any] | None]:
    try:
        result, visit_s, lower_s, total_s = _lower_module_serial_with_context(
            module_name,
            module_path,
            lowering_context=lowering_context,
        )
    except _ModuleLowerError as exc:
        lowering_hooks.record_frontend_timing(
            module_name=module_name,
            module_path=module_path,
            visit_s=0.0,
            lower_s=0.0,
            total_s=0.0,
            timed_out=exc.timed_out,
            detail=str(exc),
        )
        return None, None, lowering_hooks.fail(
            str(exc), lowering_hooks.json_output, command="build"
        )
    result_timings = _FrontendModuleResultTimings(
        visit_s=visit_s,
        lower_s=lower_s,
        total_s=total_s,
    )
    lowering_hooks.record_frontend_timing(
        module_name=module_name,
        module_path=module_path,
        visit_s=result_timings.visit_s,
        lower_s=result_timings.lower_s,
        total_s=result_timings.total_s,
    )
    return result, result_timings, None


def _register_global_code_id_with_state(
    integration_state: _FrontendIntegrationState,
    symbol: str,
) -> int:
    code_id = integration_state.global_code_ids.get(symbol)
    if code_id is None:
        code_id = integration_state.global_code_id_counter
        integration_state.global_code_ids[symbol] = code_id
        integration_state.global_code_id_counter += 1
    return code_id


def _remap_module_code_ops_with_state(
    integration_state: _FrontendIntegrationState,
    module_name: str,
    funcs: list[dict[str, Any]],
    local_id_to_symbol: dict[int, str],
) -> None:
    for func in funcs:
        ops = func.get("ops", [])
        remapped_ops: list[dict[str, Any]] = []
        for op in ops:
            kind = op.get("kind")
            if kind == "code_slots_init":
                continue
            if kind in {"call", "call_internal"}:
                symbol = op.get("s_value")
                if symbol:
                    op["value"] = _register_global_code_id_with_state(
                        integration_state, symbol
                    )
            elif kind == "code_slot_set":
                local_id = op.get("value")
                symbol = local_id_to_symbol.get(local_id)
                if symbol is None:
                    raise ValueError(
                        "Missing code symbol for id "
                        f"{local_id} in module {module_name}"
                    )
                op["value"] = _register_global_code_id_with_state(
                    integration_state, symbol
                )
            elif kind == "trace_enter_slot":
                local_id = op.get("value")
                symbol = local_id_to_symbol.get(local_id)
                if symbol is None:
                    raise ValueError(
                        "Missing code symbol for id "
                        f"{local_id} in module {module_name}"
                    )
                op["value"] = _register_global_code_id_with_state(
                    integration_state, symbol
                )
            remapped_ops.append(op)
        func["ops"] = remapped_ops


def _accumulate_midend_diagnostics_with_state(
    diagnostics_state: _MidendDiagnosticsState,
    module_name: str,
    *,
    policy_outcomes_by_func: dict[str, dict[str, Any]],
    pass_stats_by_func: dict[str, dict[str, dict[str, Any]]],
) -> None:
    def normalize_function_name(function_name: str) -> str:
        if function_name == "molt_main":
            return SimpleTIRGenerator.module_init_symbol(module_name)
        return function_name

    for function_name in sorted(policy_outcomes_by_func):
        normalized_name = normalize_function_name(function_name)
        combined_name = f"{module_name}::{normalized_name}"
        outcome = policy_outcomes_by_func[function_name]
        copied_events: list[dict[str, Any]] = []
        for event in outcome.get("degrade_events", []):
            if isinstance(event, dict):
                copied_events.append(dict(event))
        copied_outcome = dict(outcome)
        copied_outcome["degrade_events"] = copied_events
        diagnostics_state.policy_outcomes_by_function[combined_name] = copied_outcome
    for function_name in sorted(pass_stats_by_func):
        normalized_name = normalize_function_name(function_name)
        combined_name = f"{module_name}::{normalized_name}"
        per_pass = pass_stats_by_func[function_name]
        copied_per_pass: dict[str, dict[str, Any]] = {}
        for pass_name in sorted(per_pass):
            copied_stats = dict(per_pass[pass_name])
            samples = copied_stats.get("samples_ms")
            if isinstance(samples, list):
                copied_stats["samples_ms"] = list(samples)
            copied_per_pass[pass_name] = copied_stats
        diagnostics_state.pass_stats_by_function[combined_name] = copied_per_pass


def _integrate_module_frontend_result_with_state(
    integration_state: _FrontendIntegrationState,
    module_name: str,
    *,
    ir_functions: list[dict[str, Any]],
    func_code_ids: dict[str, int],
    local_class_names: list[str],
    local_classes: dict[str, Any],
) -> str | None:
    init_symbol = SimpleTIRGenerator.module_init_symbol(module_name)
    local_code_ids = dict(func_code_ids)
    if "molt_main" in local_code_ids:
        local_code_ids[init_symbol] = local_code_ids.pop("molt_main")
    local_id_to_symbol = {
        code_id: symbol for symbol, code_id in local_code_ids.items()
    }
    try:
        _remap_module_code_ops_with_state(
            integration_state,
            module_name,
            ir_functions,
            local_id_to_symbol,
        )
    except ValueError as exc:
        return str(exc)
    for func in ir_functions:
        if func["name"] == "molt_main":
            func["name"] = init_symbol
    integration_state.functions.extend(ir_functions)
    for class_name in local_class_names:
        class_info = local_classes.get(class_name)
        if class_info is not None:
            integration_state.known_classes[class_name] = class_info
    return None


def _lower_entry_module_as_main(
    *,
    lowering_context: _EntryFrontendLoweringContext,
    integration_state: _FrontendIntegrationState,
    diagnostics_state: _MidendDiagnosticsState,
    record_frontend_timing: Callable[..., None],
    fail: Callable[[str, bool, str], dict[str, Any] | None],
    json_output: bool,
) -> dict[str, Any] | None:
    try:
        source = _read_module_source(lowering_context.entry_path)
    except (SyntaxError, UnicodeDecodeError) as exc:
        return fail(
            f"Syntax error in {lowering_context.entry_path}: {exc}",
            json_output,
            command="build",
        )
    except OSError as exc:
        return fail(
            f"Failed to read module {lowering_context.entry_path}: {exc}",
            json_output,
            command="build",
        )
    try:
        tree = ast.parse(source, filename=str(lowering_context.entry_path))
    except SyntaxError as exc:
        return fail(
            f"Syntax error in {lowering_context.entry_path}: {exc}",
            json_output,
            command="build",
        )

    main_gen = SimpleTIRGenerator(
        parse_codec=lowering_context.parse_codec,
        type_hint_policy=lowering_context.type_hint_policy,
        fallback_policy=lowering_context.fallback_policy,
        source_path=str(lowering_context.entry_path),
        type_facts=lowering_context.type_facts,
        type_facts_module=lowering_context.entry_module,
        module_name="__main__",
        module_spec_name=lowering_context.entry_module,
        entry_module=None,
        enable_phi=lowering_context.enable_phi,
        known_modules=lowering_context.known_modules,
        known_classes=lowering_context.known_classes,
        stdlib_allowlist=lowering_context.stdlib_allowlist,
        known_func_defaults=lowering_context.known_func_defaults,
        module_chunking=lowering_context.module_chunking,
        module_chunk_max_ops=lowering_context.module_chunk_max_ops,
        optimization_profile=lowering_context.optimization_profile,
        pgo_hot_functions=lowering_context.pgo_hot_function_names,
    )
    main_frontend_start = time.perf_counter()
    main_visit_s = 0.0
    main_lower_s = 0.0
    try:
        main_visit_start = time.perf_counter()
        with _phase_timeout(
            lowering_context.frontend_phase_timeout,
            phase_name="frontend visit (__main__)",
        ):
            main_gen.visit(tree)
        main_visit_s = time.perf_counter() - main_visit_start
        main_lower_start = time.perf_counter()
        with _phase_timeout(
            lowering_context.frontend_phase_timeout,
            phase_name="frontend IR lowering (__main__)",
        ):
            main_ir = main_gen.to_json()
        main_lower_s = time.perf_counter() - main_lower_start
    except TimeoutError as exc:
        record_frontend_timing(
            module_name="__main__",
            module_path=lowering_context.entry_path,
            visit_s=main_visit_s,
            lower_s=main_lower_s,
            total_s=time.perf_counter() - main_frontend_start,
            timed_out=True,
            detail=str(exc),
        )
        return fail(str(exc), json_output, command="build")
    except CompatibilityError as exc:
        return fail(str(exc), json_output, command="build")

    record_frontend_timing(
        module_name="__main__",
        module_path=lowering_context.entry_path,
        visit_s=main_visit_s,
        lower_s=main_lower_s,
        total_s=time.perf_counter() - main_frontend_start,
    )
    main_init = SimpleTIRGenerator.module_init_symbol("__main__")
    local_code_ids = dict(main_gen.func_code_ids)
    if "molt_main" in local_code_ids:
        local_code_ids[main_init] = local_code_ids.pop("molt_main")
    local_id_to_symbol = {
        code_id: symbol for symbol, code_id in local_code_ids.items()
    }
    try:
        _remap_module_code_ops_with_state(
            integration_state,
            "__main__",
            main_ir["functions"],
            local_id_to_symbol,
        )
    except ValueError as exc:
        return fail(str(exc), json_output, command="build")
    for func in main_ir["functions"]:
        if func["name"] == "molt_main":
            func["name"] = main_init
    integration_state.functions.extend(main_ir["functions"])
    _accumulate_midend_diagnostics_with_state(
        diagnostics_state,
        "__main__",
        policy_outcomes_by_func=dict(main_gen.midend_policy_outcomes_by_function),
        pass_stats_by_func=dict(main_gen.midend_pass_stats_by_function),
    )
    return None


def _append_module_code_slot_ops(
    ops: list[dict[str, Any]],
    *,
    logical_source_path: str,
    code_id: int,
    next_var: int,
) -> int:
    file_var = f"v{next_var}"
    next_var += 1
    name_var = f"v{next_var}"
    next_var += 1
    line_var = f"v{next_var}"
    next_var += 1
    linetable_var = f"v{next_var}"
    next_var += 1
    varnames_var = f"v{next_var}"
    next_var += 1
    argcount_var = f"v{next_var}"
    next_var += 1
    posonly_var = f"v{next_var}"
    next_var += 1
    kwonly_var = f"v{next_var}"
    next_var += 1
    code_var = f"v{next_var}"
    next_var += 1
    ops.extend(
        [
            {
                "kind": "const_str",
                "s_value": logical_source_path,
                "out": file_var,
            },
            {"kind": "const_str", "s_value": "<module>", "out": name_var},
            {"kind": "const", "value": 1, "out": line_var},
            {"kind": "const_none", "out": linetable_var},
            {"kind": "tuple_new", "args": [], "out": varnames_var},
            {"kind": "const", "value": 0, "out": argcount_var},
            {"kind": "const", "value": 0, "out": posonly_var},
            {"kind": "const", "value": 0, "out": kwonly_var},
            {
                "kind": "code_new",
                "args": [
                    file_var,
                    name_var,
                    line_var,
                    linetable_var,
                    varnames_var,
                    argcount_var,
                    posonly_var,
                    kwonly_var,
                ],
                "out": code_var,
            },
            {
                "kind": "code_slot_set",
                "value": code_id,
                "args": [code_var],
            },
        ]
    )
    return next_var


def _python_version_display() -> tuple[str, str, int]:
    py_version = sys.version_info
    version_release = py_version.releaselevel
    version_serial = py_version.serial
    version_suffix = ""
    if version_release == "alpha":
        version_suffix = f"a{version_serial}"
    elif version_release == "beta":
        version_suffix = f"b{version_serial}"
    elif version_release == "candidate":
        version_suffix = f"rc{version_serial}"
    elif version_release != "final":
        version_suffix = f"{version_release}{version_serial}"
    version_str = (
        f"{py_version.major}.{py_version.minor}.{py_version.micro}"
        f"{version_suffix} (molt)"
    )
    return version_release, version_str, version_serial


def _build_version_info_ops(
    *,
    register_global_code_id: Callable[[str], int],
) -> list[dict[str, Any]]:
    py_version = sys.version_info
    version_release, version_str, version_serial = _python_version_display()
    return [
        {"kind": "const", "value": py_version.major, "out": "v3"},
        {"kind": "const", "value": py_version.minor, "out": "v4"},
        {"kind": "const", "value": py_version.micro, "out": "v5"},
        {"kind": "const_str", "s_value": version_release, "out": "v6"},
        {"kind": "const", "value": version_serial, "out": "v7"},
        {"kind": "const_str", "s_value": version_str, "out": "v8"},
        {
            "kind": "call",
            "s_value": "molt_sys_set_version_info",
            "args": ["v3", "v4", "v5", "v6", "v7", "v8"],
            "out": "v9",
            "value": register_global_code_id("molt_sys_set_version_info"),
        },
    ]


def _build_entry_main_ops(
    *,
    entry_init: str,
    version_ops: Sequence[dict[str, Any]],
    register_global_code_id: Callable[[str], int],
) -> list[dict[str, Any]]:
    return [
        {
            "kind": "call",
            "s_value": "molt_runtime_init",
            "args": [],
            "out": "v0",
            "value": register_global_code_id("molt_runtime_init"),
        },
        *version_ops,
        {
            "kind": "call",
            "s_value": entry_init,
            "args": [],
            "out": "v1",
            "value": register_global_code_id(entry_init),
        },
        {
            "kind": "call",
            "s_value": "molt_runtime_shutdown",
            "args": [],
            "out": "v2",
            "value": register_global_code_id("molt_runtime_shutdown"),
        },
        {"kind": "ret_void"},
    ]


def _entry_call_index(entry_ops: Sequence[dict[str, Any]], entry_init: str) -> int:
    return next(
        idx
        for idx, op in enumerate(entry_ops)
        if op.get("kind") == "call" and op.get("s_value") == entry_init
    )


def _next_tir_var_index(ops: Sequence[dict[str, Any]]) -> int:
    used_vars: set[int] = set()
    for op in ops:
        out = op.get("out")
        if isinstance(out, str) and out.startswith("v"):
            try:
                used_vars.add(int(out[1:]))
            except ValueError:
                continue
    return max(used_vars, default=-1) + 1


def _append_entry_sys_init_op(
    entry_ops: list[dict[str, Any]],
    *,
    entry_init: str,
    register_global_code_id: Callable[[str], int],
    next_var: int,
) -> int:
    sys_init = SimpleTIRGenerator.module_init_symbol("sys")
    sys_out_var = f"v{next_var}"
    next_var += 1
    entry_call_idx = _entry_call_index(entry_ops, entry_init)
    entry_ops[entry_call_idx:entry_call_idx] = [
        {
            "kind": "call",
            "s_value": sys_init,
            "args": [],
            "out": sys_out_var,
            "value": register_global_code_id(sys_init),
        }
    ]
    return next_var


def _build_module_code_ops(
    *,
    module_order: Sequence[str],
    module_graph: Mapping[str, Path],
    generated_module_source_paths: Mapping[str, str],
    entry_module: str,
    entry_path: Path | None,
    register_global_code_id: Callable[[str], int],
    next_var: int,
) -> tuple[list[dict[str, Any]], int]:
    module_code_ops: list[dict[str, Any]] = []
    for module_name in module_order:
        module_path = module_graph[module_name]
        logical_source_path = generated_module_source_paths.get(
            module_name, module_path.as_posix()
        )
        init_symbol = SimpleTIRGenerator.module_init_symbol(module_name)
        code_id = register_global_code_id(init_symbol)
        next_var = _append_module_code_slot_ops(
            module_code_ops,
            logical_source_path=logical_source_path,
            code_id=code_id,
            next_var=next_var,
        )
    if entry_module != "__main__" and entry_path is not None:
        init_symbol = SimpleTIRGenerator.module_init_symbol("__main__")
        code_id = register_global_code_id(init_symbol)
        next_var = _append_module_code_slot_ops(
            module_code_ops,
            logical_source_path=entry_path.as_posix(),
            code_id=code_id,
            next_var=next_var,
        )
    return module_code_ops, next_var


def _replace_entry_call_with_spawn_override(
    entry_ops: list[dict[str, Any]],
    *,
    entry_init: str,
    register_global_code_id: Callable[[str], int],
    next_var: int,
) -> int:
    spawn_init = SimpleTIRGenerator.module_init_symbol(ENTRY_OVERRIDE_SPAWN)
    spawn_code_id = register_global_code_id(spawn_init)
    entry_call_idx = _entry_call_index(entry_ops, entry_init)
    entry_code_id = register_global_code_id(entry_init)
    env_key_var = f"v{next_var}"
    next_var += 1
    env_default_var = f"v{next_var}"
    next_var += 1
    env_value_var = f"v{next_var}"
    next_var += 1
    spawn_name_var = f"v{next_var}"
    next_var += 1
    spawn_eq_var = f"v{next_var}"
    next_var += 1
    spawn_out_var = f"v{next_var}"
    next_var += 1
    entry_out_var = f"v{next_var}"
    next_var += 1
    entry_ops[entry_call_idx : entry_call_idx + 1] = [
        {"kind": "const_str", "s_value": ENTRY_OVERRIDE_ENV, "out": env_key_var},
        {"kind": "const_str", "s_value": "", "out": env_default_var},
        {
            "kind": "env_get",
            "args": [env_key_var, env_default_var],
            "out": env_value_var,
        },
        {
            "kind": "const_str",
            "s_value": ENTRY_OVERRIDE_SPAWN,
            "out": spawn_name_var,
        },
        {
            "kind": "string_eq",
            "args": [env_value_var, spawn_name_var],
            "out": spawn_eq_var,
        },
        {"kind": "if", "args": [spawn_eq_var]},
        {
            "kind": "call",
            "s_value": spawn_init,
            "args": [],
            "out": spawn_out_var,
            "value": spawn_code_id,
        },
        {"kind": "else"},
        {
            "kind": "call",
            "s_value": entry_init,
            "args": [],
            "out": entry_out_var,
            "value": entry_code_id,
        },
        {"kind": "end_if"},
    ]
    return next_var


def _build_isolate_bootstrap_ops(
    *,
    code_slot_count: int,
    version_ops: Sequence[dict[str, Any]],
    module_code_ops: Sequence[dict[str, Any]],
) -> list[dict[str, Any]]:
    return [
        {"kind": "code_slots_init", "value": code_slot_count},
        *version_ops,
        *module_code_ops,
        {"kind": "ret_void"},
    ]


def _build_isolate_import_ops(
    *,
    module_order: Sequence[str],
    register_global_code_id: Callable[[str], int],
) -> list[dict[str, Any]]:
    import_ops: list[dict[str, Any]] = []
    import_var_idx = 0

    def import_var() -> str:
        nonlocal import_var_idx
        name = f"v{import_var_idx}"
        import_var_idx += 1
        return name

    name_var = "p0"
    module_var = import_var()
    import_ops.append({"kind": "module_cache_get", "args": [name_var], "out": module_var})
    none_var = import_var()
    import_ops.append({"kind": "const_none", "out": none_var})
    is_none_var = import_var()
    import_ops.append({"kind": "is", "args": [module_var, none_var], "out": is_none_var})
    import_ops.append({"kind": "if", "args": [is_none_var]})
    if module_order:
        for idx, module_name in enumerate(module_order):
            match_name_var = import_var()
            import_ops.append(
                {"kind": "const_str", "s_value": module_name, "out": match_name_var}
            )
            match_var = import_var()
            import_ops.append(
                {"kind": "string_eq", "args": [name_var, match_name_var], "out": match_var}
            )
            import_ops.append({"kind": "if", "args": [match_var]})
            init_symbol = SimpleTIRGenerator.module_init_symbol(module_name)
            init_out = import_var()
            import_ops.append(
                {
                    "kind": "call",
                    "s_value": init_symbol,
                    "args": [],
                    "out": init_out,
                    "value": register_global_code_id(init_symbol),
                }
            )
            if idx < len(module_order) - 1:
                import_ops.append({"kind": "else"})
        import_ops.extend({"kind": "end_if"} for _ in module_order)
    import_ops.append({"kind": "end_if"})
    loaded_var = import_var()
    import_ops.append({"kind": "module_cache_get", "args": [name_var], "out": loaded_var})
    import_ops.append({"kind": "ret", "args": [loaded_var]})
    return import_ops


def _finalize_backend_ir(
    *,
    functions: Sequence[dict[str, Any]],
    pgo_profile_summary: Any | None,
    runtime_feedback_summary: Any | None,
) -> dict[str, Any]:
    ir: dict[str, Any] = {"functions": list(functions)}
    if pgo_profile_summary is not None:
        ir["profile"] = {
            "version": pgo_profile_summary.version,
            "hash": pgo_profile_summary.hash,
            "hot_functions": pgo_profile_summary.hot_functions,
        }
    if runtime_feedback_summary is not None:
        ir["runtime_feedback"] = {
            "schema_version": runtime_feedback_summary.schema_version,
            "hash": runtime_feedback_summary.hash,
            "hot_functions": runtime_feedback_summary.hot_functions,
        }
    return ir


def _write_emitted_ir(emit_ir_path: Path | None, ir: Mapping[str, Any]) -> str | None:
    if emit_ir_path is None:
        return None
    try:
        emit_ir_path.write_text(
            json.dumps(ir, indent=2, default=_json_ir_default) + "\n"
        )
    except OSError as exc:
        return f"Failed to write IR: {exc}"
    return None


def _build_cache_info(
    *,
    enabled: bool,
    hit: bool,
    cache_key: str | None,
    function_cache_key: str | None,
    cache_path: Path | None,
    function_cache_path: Path | None,
    cache_hit_tier: str | None,
    backend_daemon_cached: bool | None,
    backend_daemon_cache_tier: str | None,
    backend_daemon_config_digest: str | None,
) -> dict[str, Any]:
    cache_info: dict[str, Any] = {"enabled": enabled, "hit": hit}
    if cache_key:
        cache_info["key"] = cache_key
    if function_cache_key:
        cache_info["function_key"] = function_cache_key
    if cache_path is not None:
        cache_info["path"] = str(cache_path)
    if function_cache_path is not None:
        cache_info["function_path"] = str(function_cache_path)
    if cache_hit_tier:
        cache_info["hit_tier"] = cache_hit_tier
    if (
        backend_daemon_cached is None
        and backend_daemon_cache_tier is None
        and backend_daemon_config_digest is None
    ):
        return cache_info
    daemon_info: dict[str, Any] = {}
    if backend_daemon_cached is not None:
        daemon_info["cached"] = backend_daemon_cached
    if backend_daemon_cache_tier is not None:
        daemon_info["cache_tier"] = backend_daemon_cache_tier
    if backend_daemon_config_digest is not None:
        daemon_info["config_digest"] = backend_daemon_config_digest
    cache_info["daemon"] = daemon_info
    return cache_info


def _attach_build_metadata(
    data: MutableMapping[str, Any],
    *,
    diagnostics_payload: Any | None,
    pgo_profile_payload: Any | None,
    runtime_feedback_payload: Any | None,
    emit_ir_path: Path | None,
) -> MutableMapping[str, Any]:
    if diagnostics_payload is not None:
        data["compile_diagnostics"] = diagnostics_payload
    if pgo_profile_payload is not None:
        data["pgo_profile"] = pgo_profile_payload
    if runtime_feedback_payload is not None:
        data["runtime_feedback"] = runtime_feedback_payload
    if emit_ir_path is not None:
        data["emit_ir"] = str(emit_ir_path)
    return data


def _build_common_build_json_data(
    *,
    target: str,
    target_triple: str | None,
    source_path: Path,
    output: Path,
    deterministic: bool,
    trusted: bool,
    capabilities_list: list[str] | None,
    capability_profiles: list[str] | None,
    capabilities_source: str | None,
    sysroot_path: Path | None,
    cache_info: Mapping[str, Any],
    emit_mode: str,
    profile: str,
    native_arch_perf_enabled: bool,
) -> dict[str, Any]:
    return {
        "target": target,
        "target_triple": target_triple,
        "entry": str(source_path),
        "output": str(output),
        "deterministic": deterministic,
        "trusted": trusted,
        "capabilities": capabilities_list,
        "capability_profiles": capability_profiles,
        "capabilities_source": capabilities_source,
        "sysroot": str(sysroot_path) if sysroot_path is not None else None,
        "cache": dict(cache_info),
        "emit": emit_mode,
        "profile": profile,
        "native_arch_perf": native_arch_perf_enabled,
        "cpu_baseline": _cpu_baseline(target_triple),
        "cranelift_flags": "default",
    }


def _attach_process_output(
    data: MutableMapping[str, Any],
    process: subprocess.CompletedProcess[str],
) -> MutableMapping[str, Any]:
    if process.stdout:
        data["stdout"] = process.stdout
    if process.stderr:
        data["stderr"] = process.stderr
    return data


def _emit_build_success_json(
    *,
    data: Mapping[str, Any],
    warnings: Sequence[str],
    json_output: bool,
) -> None:
    payload = _json_payload(
        "build",
        "ok",
        data=dict(data),
        warnings=list(warnings),
    )
    _emit_json(payload, json_output)


def _emit_build_diagnostics_if_present(
    *,
    diagnostics_payload: dict[str, Any] | None,
    diagnostics_path: Path | None,
    json_output: bool,
    verbosity: str,
) -> None:
    _emit_build_diagnostics(
        diagnostics=diagnostics_payload,
        diagnostics_path=diagnostics_path,
        json_output=json_output,
        verbosity=verbosity,
    )


def _build_native_link_success_data(
    *,
    target: str,
    target_triple: str | None,
    source_path: Path,
    output_binary: Path,
    deterministic: bool,
    trusted: bool,
    capabilities_list: list[str] | None,
    capability_profiles: list[str] | None,
    capabilities_source: str | None,
    sysroot_path: Path | None,
    cache_info: Mapping[str, Any],
    emit_mode: str,
    profile: str,
    native_arch_perf_enabled: bool,
    output_obj: Path,
    stub_path: Path,
    runtime_lib: Path,
    link_skipped: bool,
) -> dict[str, Any]:
    data = _build_common_build_json_data(
        target=target,
        target_triple=target_triple,
        source_path=source_path,
        output=output_binary,
        deterministic=deterministic,
        trusted=trusted,
        capabilities_list=capabilities_list,
        capability_profiles=capability_profiles,
        capabilities_source=capabilities_source,
        sysroot_path=sysroot_path,
        cache_info=cache_info,
        emit_mode=emit_mode,
        profile=profile,
        native_arch_perf_enabled=native_arch_perf_enabled,
    )
    data["artifacts"] = {
        "object": str(output_obj),
        "stub": str(stub_path),
        "runtime": str(runtime_lib),
    }
    data["link"] = {"skipped": link_skipped}
    return data


def _build_native_link_error_data(
    *,
    target: str,
    source_path: Path,
    returncode: int,
    emit_mode: str,
    profile: str,
    native_arch_perf_enabled: bool,
    trusted: bool,
    cache_info: Mapping[str, Any],
) -> dict[str, Any]:
    return {
        "target": target,
        "entry": str(source_path),
        "returncode": returncode,
        "emit": emit_mode,
        "profile": profile,
        "native_arch_perf": native_arch_perf_enabled,
        "trusted": trusted,
        "cache": dict(cache_info),
    }


def _native_main_stub_snippets(
    *,
    trusted: bool,
    capabilities_list: Sequence[str] | None,
) -> tuple[str, str, str, str]:
    trusted_snippet = ""
    trusted_call = ""
    if trusted:
        trusted_snippet = """
static void molt_set_trusted() {
#ifdef _WIN32
    _putenv_s("MOLT_TRUSTED", "1");
#else
    setenv("MOLT_TRUSTED", "1", 1);
#endif
}
"""
        trusted_call = "    molt_set_trusted();\n"
    capabilities_snippet = ""
    capabilities_call = ""
    if capabilities_list is not None:
        caps_literal = json.dumps(",".join(capabilities_list))
        capabilities_snippet = f"""
static void molt_set_capabilities() {{
#ifdef _WIN32
    _putenv_s("MOLT_CAPABILITIES", {caps_literal});
#else
    setenv("MOLT_CAPABILITIES", {caps_literal}, 1);
#endif
}}
"""
        capabilities_call = "    molt_set_capabilities();\n"
    return trusted_snippet, trusted_call, capabilities_snippet, capabilities_call


def _render_native_main_stub(
    *,
    trusted: bool,
    capabilities_list: Sequence[str] | None,
) -> str:
    trusted_snippet, trusted_call, capabilities_snippet, capabilities_call = (
        _native_main_stub_snippets(
            trusted=trusted,
            capabilities_list=capabilities_list,
        )
    )
    main_c_content = """
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifdef _WIN32
#include <wchar.h>
#endif
extern unsigned long long molt_runtime_init();
extern void molt_runtime_ensure_gil();
extern unsigned long long molt_runtime_shutdown();
extern void molt_set_argv(int argc, const char** argv);
#ifdef _WIN32
extern void molt_set_argv_utf16(int argc, const wchar_t** argv);
#endif
extern void molt_main();
extern unsigned long long molt_exception_pending();
extern unsigned long long molt_exception_last();
extern unsigned long long molt_raise(unsigned long long exc_bits);
extern void molt_dec_ref(unsigned long long bits);
extern int molt_json_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_msgpack_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_cbor_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern long molt_get_attr_generic(void* obj, const char* attr, long len);
extern unsigned long long molt_alloc(long size);
extern long molt_block_on(void* task);
extern long molt_async_sleep(void* obj);
extern void molt_spawn(void* task);
extern void* molt_chan_new(unsigned long long capacity);
extern long molt_chan_send(void* chan, long val);
extern long molt_chan_recv(void* chan);
extern long molt_chan_try_send(void* chan, long val);
extern long molt_chan_try_recv(void* chan);
extern long molt_chan_send_blocking(void* chan, long val);
extern long molt_chan_recv_blocking(void* chan);
extern void molt_print_obj(unsigned long long val);
extern void molt_profile_dump();
/* MOLT_TRUSTED_SNIPPET */
/* MOLT_CAPABILITIES_SNIPPET */

static int molt_finish() {
    unsigned long long pending = molt_exception_pending();
    const char* debug_exc = getenv("MOLT_DEBUG_MAIN_EXCEPTION");
    if (debug_exc != NULL && debug_exc[0] != '\\0' && strcmp(debug_exc, "0") != 0) {
        fprintf(stderr, "molt main finish pending=%d\\n", pending != 0);
    }
    if (pending != 0) {
        unsigned long long exc = molt_exception_last();
        molt_raise(exc);
        molt_dec_ref(exc);
        molt_runtime_shutdown();
        return 1;
    }
    const char* profile = getenv("MOLT_PROFILE");
    if (profile != NULL && profile[0] != '\\0' && strcmp(profile, "0") != 0) {
        molt_profile_dump();
    }
    molt_runtime_shutdown();
    return 0;
}

#ifdef _WIN32
int wmain(int argc, wchar_t** argv) {
    /* MOLT_TRUSTED_CALL */
    /* MOLT_CAPABILITIES_CALL */
    molt_runtime_init();
    molt_runtime_ensure_gil();
    molt_set_argv_utf16(argc, (const wchar_t**)argv);
    molt_main();
    return molt_finish();
}
#else
int main(int argc, char** argv) {
    /* MOLT_TRUSTED_CALL */
    /* MOLT_CAPABILITIES_CALL */
    molt_runtime_init();
    molt_runtime_ensure_gil();
    molt_set_argv(argc, (const char**)argv);
    molt_main();
    return molt_finish();
}
#endif
"""
    main_c_content = main_c_content.replace(
        "/* MOLT_TRUSTED_SNIPPET */", trusted_snippet
    )
    main_c_content = main_c_content.replace(
        "/* MOLT_CAPABILITIES_SNIPPET */", capabilities_snippet
    )
    main_c_content = main_c_content.replace("/* MOLT_TRUSTED_CALL */", trusted_call)
    main_c_content = main_c_content.replace(
        "/* MOLT_CAPABILITIES_CALL */", capabilities_call
    )
    return main_c_content


def _build_native_link_command(
    *,
    output_obj: Path,
    stub_path: Path,
    runtime_lib: Path,
    output_binary: Path,
    target_triple: str | None,
    sysroot_path: Path | None,
    profile: str,
) -> tuple[list[str], str | None, str | None]:
    cc = os.environ.get("CC", "clang")
    link_cmd = shlex.split(cc)
    normalized_target: str | None = target_triple
    if target_triple:
        cross_cc = os.environ.get("MOLT_CROSS_CC")
        target_arg = target_triple
        if cross_cc:
            link_cmd = shlex.split(cross_cc)
        elif shutil.which("zig"):
            link_cmd = ["zig", "cc"]
            target_arg = _zig_target_query(target_triple)
            normalized_target = target_arg
        else:
            raise RuntimeError(
                f"Cross-target build requires zig or MOLT_CROSS_CC (missing for {target_triple})."
            )
        link_cmd.extend(["-target", target_arg])
    if sysroot_path is not None:
        sysroot_flag = "--sysroot"
        if link_cmd and Path(link_cmd[0]).name.startswith("zig"):
            sysroot_flag = "--sysroot"
        elif (
            target_triple and ("apple" in target_triple or "darwin" in target_triple)
        ) or (not target_triple and sys.platform == "darwin"):
            sysroot_flag = "-isysroot"
        link_cmd.extend([sysroot_flag, str(sysroot_path)])
    cflags = os.environ.get("CFLAGS", "")
    if cflags:
        link_cmd.extend(shlex.split(cflags))
    linker_hint: str | None = None
    if profile == "dev":
        linker_hint = _resolve_dev_linker()
        if linker_hint and not any(arg.startswith("-fuse-ld=") for arg in link_cmd):
            link_cmd.append(f"-fuse-ld={linker_hint}")
    if sys.platform == "darwin" and not target_triple:
        link_cmd = _strip_arch_flags(link_cmd)
        arch = (
            os.environ.get("MOLT_ARCH")
            or _detect_macos_arch(output_obj)
            or platform.machine()
        )
        link_cmd.extend(["-arch", arch])
        deployment_target = _detect_macos_deployment_target()
        if deployment_target:
            link_cmd.append(f"-mmacosx-version-min={deployment_target}")
    link_cmd.extend(
        [str(stub_path), str(output_obj), str(runtime_lib), "-o", str(output_binary)]
    )
    if target_triple:
        if "apple" in target_triple or "darwin" in target_triple:
            link_cmd.append("-Wl,-dead_strip")
            link_cmd.append("-lc++")
        elif "linux" in target_triple:
            link_cmd.extend(["-fdata-sections", "-ffunction-sections"])
            link_cmd.append("-Wl,--gc-sections")
            link_cmd.append("-lstdc++")
            link_cmd.append("-lm")
    else:
        if sys.platform == "darwin":
            link_cmd.append("-Wl,-dead_strip")
            link_cmd.append("-lc++")
        elif sys.platform.startswith("linux"):
            link_cmd.extend(["-fdata-sections", "-ffunction-sections"])
            link_cmd.append("-Wl,--gc-sections")
            link_cmd.append("-lstdc++")
            link_cmd.append("-lm")
    return link_cmd, linker_hint, normalized_target


def _run_native_link_command(
    *,
    link_cmd: Sequence[str],
    json_output: bool,
    link_timeout: float | None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(link_cmd),
        capture_output=json_output,
        text=True,
        timeout=link_timeout,
    )


def _retry_native_link_without_hint(
    *,
    link_cmd: Sequence[str],
    linker_hint: str | None,
    json_output: bool,
    link_timeout: float | None,
) -> tuple[subprocess.CompletedProcess[str] | None, list[str]]:
    if linker_hint is None:
        return None, list(link_cmd)
    retry_cmd = [arg for arg in link_cmd if arg != f"-fuse-ld={linker_hint}"]
    if retry_cmd == list(link_cmd):
        return None, retry_cmd
    retry_process = _run_native_link_command(
        link_cmd=retry_cmd,
        json_output=json_output,
        link_timeout=link_timeout,
    )
    return retry_process, retry_cmd


def _darwin_link_validation_failure(
    *,
    output_binary: Path,
    kind: str,
) -> str | None:
    if kind == "magic":
        detail = _darwin_binary_magic_error(output_binary)
        if detail is None:
            return None
        return "Generated binary failed Mach-O header validation.\n" + detail + "\n"
    detail = _darwin_binary_imports_validation_error(output_binary)
    if detail is None:
        return None
    return "Generated binary failed dyld import validation.\n" + detail + "\n"


def _validate_darwin_link_output(
    *,
    link_process: subprocess.CompletedProcess[str],
    link_cmd: Sequence[str],
    linker_hint: str | None,
    output_binary: Path,
    validation_kind: str,
    json_output: bool,
    link_timeout: float | None,
    warnings: list[str],
) -> subprocess.CompletedProcess[str]:
    validation_error = _darwin_link_validation_failure(
        output_binary=output_binary,
        kind=validation_kind,
    )
    if (
        validation_error is not None
        and linker_hint is not None
        and any(arg == f"-fuse-ld={linker_hint}" for arg in link_cmd)
    ):
        retry_process, _ = _retry_native_link_without_hint(
            link_cmd=link_cmd,
            linker_hint=linker_hint,
            json_output=json_output,
            link_timeout=link_timeout,
        )
        if retry_process is not None:
            if retry_process.returncode == 0:
                retry_validation_error = _darwin_link_validation_failure(
                    output_binary=output_binary,
                    kind=validation_kind,
                )
                if retry_validation_error is None:
                    label = (
                        "invalid output"
                        if validation_kind == "magic"
                        else "invalid dyld imports"
                    )
                    warnings.append(
                        "Linker fallback: "
                        f"-fuse-ld={linker_hint} produced {label}; "
                        "retried default linker."
                    )
                    return retry_process
                link_process = retry_process
                validation_error = retry_validation_error
            else:
                return retry_process
    if validation_error is None:
        return link_process
    failure_stderr = (link_process.stderr or "") + "\n" + validation_error
    return subprocess.CompletedProcess(
        args=list(link_cmd),
        returncode=1,
        stdout=link_process.stdout,
        stderr=failure_stderr,
    )


def _write_link_fingerprint_if_needed(
    *,
    link_skipped: bool,
    link_fingerprint: dict[str, Any] | None,
    link_fingerprint_path: Path,
    json_output: bool,
) -> None:
    if link_skipped or link_fingerprint is None:
        return
    try:
        link_fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
        _write_runtime_fingerprint(link_fingerprint_path, link_fingerprint)
    except OSError:
        if not json_output:
            print(
                "Warning: failed to write link fingerprint metadata.",
                file=sys.stderr,
            )


def _emit_native_link_result(
    *,
    link_process: subprocess.CompletedProcess[str],
    link_skipped: bool,
    link_fingerprint: dict[str, Any] | None,
    link_fingerprint_path: Path,
    cache: bool,
    cache_hit: bool,
    cache_key: str | None,
    function_cache_key: str | None,
    cache_path: Path | None,
    function_cache_path: Path | None,
    cache_hit_tier: str | None,
    backend_daemon_cached: bool | None,
    backend_daemon_cache_tier: str | None,
    backend_daemon_config_digest: str | None,
    target: str,
    target_triple: str | None,
    source_path: Path,
    output_binary: Path,
    deterministic: bool,
    trusted: bool,
    capabilities_list: list[str] | None,
    capability_profiles: list[str] | None,
    capabilities_source: str | None,
    sysroot_path: Path | None,
    emit_mode: str,
    profile: str,
    native_arch_perf_enabled: bool,
    output_obj: Path,
    stub_path: Path,
    runtime_lib: Path,
    diagnostics_payload: dict[str, Any] | None,
    diagnostics_path: Path | None,
    pgo_profile_payload: Any | None,
    runtime_feedback_payload: Any | None,
    emit_ir_path: Path | None,
    warnings: list[str],
    json_output: bool,
    resolved_diagnostics_verbosity: str,
) -> int:
    if link_process.returncode == 0:
        _write_link_fingerprint_if_needed(
            link_skipped=link_skipped,
            link_fingerprint=link_fingerprint,
            link_fingerprint_path=link_fingerprint_path,
            json_output=json_output,
        )
        if json_output:
            cache_info = _build_cache_info(
                enabled=cache,
                hit=cache_hit,
                cache_key=cache_key,
                function_cache_key=function_cache_key,
                cache_path=cache_path,
                function_cache_path=function_cache_path,
                cache_hit_tier=cache_hit_tier,
                backend_daemon_cached=backend_daemon_cached,
                backend_daemon_cache_tier=backend_daemon_cache_tier,
                backend_daemon_config_digest=backend_daemon_config_digest,
            )
            data = _build_native_link_success_data(
                target=target,
                source_path=source_path,
                target_triple=target_triple,
                output_binary=output_binary,
                deterministic=deterministic,
                trusted=trusted,
                capabilities_list=capabilities_list,
                capability_profiles=capability_profiles,
                capabilities_source=capabilities_source,
                sysroot_path=sysroot_path,
                cache_info=cache_info,
                emit_mode=emit_mode,
                profile=profile,
                native_arch_perf_enabled=native_arch_perf_enabled,
                output_obj=output_obj,
                stub_path=stub_path,
                runtime_lib=runtime_lib,
                link_skipped=link_skipped,
            )
            _attach_build_metadata(
                data,
                diagnostics_payload=diagnostics_payload,
                pgo_profile_payload=pgo_profile_payload,
                runtime_feedback_payload=runtime_feedback_payload,
                emit_ir_path=emit_ir_path,
            )
            _attach_process_output(data, link_process)
            _emit_build_success_json(
                data=data,
                warnings=warnings,
                json_output=json_output,
            )
        else:
            print(f"Successfully built {output_binary}")
    else:
        if json_output:
            cache_info = _build_cache_info(
                enabled=cache,
                hit=cache_hit,
                cache_key=cache_key,
                function_cache_key=None,
                cache_path=cache_path,
                function_cache_path=None,
                cache_hit_tier=cache_hit_tier,
                backend_daemon_cached=backend_daemon_cached,
                backend_daemon_cache_tier=backend_daemon_cache_tier,
                backend_daemon_config_digest=backend_daemon_config_digest,
            )
            data = _build_native_link_error_data(
                target=target,
                source_path=source_path,
                returncode=link_process.returncode,
                emit_mode=emit_mode,
                profile=profile,
                native_arch_perf_enabled=native_arch_perf_enabled,
                trusted=trusted,
                cache_info=cache_info,
            )
            _attach_build_metadata(
                data,
                diagnostics_payload=diagnostics_payload,
                pgo_profile_payload=pgo_profile_payload,
                runtime_feedback_payload=runtime_feedback_payload,
                emit_ir_path=None,
            )
            _attach_process_output(data, link_process)
            payload = _json_payload(
                "build",
                "error",
                data=data,
                errors=["Linking failed"],
            )
            _emit_json(payload, json_output)
        else:
            print("Linking failed", file=sys.stderr)
    _emit_build_diagnostics_if_present(
        diagnostics_payload=diagnostics_payload,
        diagnostics_path=diagnostics_path,
        json_output=json_output,
        verbosity=resolved_diagnostics_verbosity,
    )
    return link_process.returncode


def _initialize_runtime_artifact_state(
    *,
    is_rust_transpile: bool,
    is_wasm: bool,
    emit_mode: str,
    molt_root: Path,
    runtime_cargo_profile: str,
    target_triple: str | None,
) -> _RuntimeArtifactState:
    state = _RuntimeArtifactState()
    if is_rust_transpile:
        return state
    if is_wasm:
        state.runtime_wasm = _runtime_wasm_artifact_path(molt_root, "molt_runtime.wasm")
        state.runtime_reloc_wasm = _runtime_wasm_artifact_path(
            molt_root, "molt_runtime_reloc.wasm"
        )
        return state
    if emit_mode == "bin":
        state.runtime_lib = _runtime_lib_path(
            molt_root,
            runtime_cargo_profile,
            target_triple,
        )
    return state


def _ensure_runtime_lib_ready(
    runtime_state: _RuntimeArtifactState,
    *,
    target_triple: str | None,
    json_output: bool,
    runtime_cargo_profile: str,
    molt_root: Path,
    cargo_timeout: float | None,
) -> bool:
    runtime_lib = runtime_state.runtime_lib
    if runtime_lib is None:
        return True
    return _ensure_runtime_lib(
        runtime_lib,
        target_triple,
        json_output,
        runtime_cargo_profile,
        molt_root,
        cargo_timeout,
    )


def _ensure_runtime_wasm_artifact(
    runtime_state: _RuntimeArtifactState,
    *,
    reloc: bool,
    json_output: bool,
    cargo_profile: str,
    cargo_timeout: float | None,
    project_root: Path,
) -> bool:
    runtime_path = (
        runtime_state.runtime_reloc_wasm if reloc else runtime_state.runtime_wasm
    )
    ready = (
        runtime_state.runtime_reloc_wasm_ready if reloc else runtime_state.runtime_wasm_ready
    )
    if runtime_path is None or ready:
        return True
    if not _ensure_runtime_wasm(
        runtime_path,
        reloc=reloc,
        json_output=json_output,
        cargo_profile=cargo_profile,
        cargo_timeout=cargo_timeout,
        project_root=project_root,
    ):
        return False
    if reloc:
        runtime_state.runtime_reloc_wasm_ready = True
    else:
        runtime_state.runtime_wasm_ready = True
    return True


def _run_frontend_parallel_enabled_layers(
    module_layers: Sequence[Sequence[str]],
    *,
    execution_context: _FrontendLayerExecutionContext,
    runtime_hooks: _FrontendLayerRuntimeHooks,
    frontend_parallel_config: _FrontendParallelConfig,
    frontend_parallel_layers: list[dict[str, Any]],
) -> dict[str, Any] | None:
    parallel_pool_usable = True
    with ProcessPoolExecutor(max_workers=frontend_parallel_config.workers) as executor:
        for layer_index, layer in enumerate(module_layers):
            layer_started_ns = time.time_ns()
            layer_run_result, layer_error = _run_frontend_layer(
                layer,
                layer_index=layer_index,
                executor=executor,
                execution_context=execution_context,
                runtime_hooks=runtime_hooks,
                frontend_parallel_config=frontend_parallel_config,
                parallel_pool_usable=parallel_pool_usable,
            )
            if layer_error is not None:
                return layer_error
            assert layer_run_result is not None
            layer_state = layer_run_result.layer_state
            layer_plan = layer_run_result.layer_plan
            parallel_pool_usable = layer_run_result.parallel_pool_usable
            _append_frontend_parallel_layer_detail(
                frontend_parallel_layers,
                layer_index=layer_index,
                layer_mode=layer_plan.mode,
                layer_policy_reason=layer_plan.policy_reason,
                module_names=layer,
                candidate_count=len(layer_plan.candidates),
                workers=layer_plan.workers,
                timing_items=layer_state.recorded_worker_timings,
                predicted_cost_total=layer_plan.predicted_cost_total,
                effective_min_predicted_cost=layer_plan.effective_min_predicted_cost,
                stdlib_candidates=layer_plan.stdlib_candidates,
                target_cost_per_worker=frontend_parallel_config.target_cost_per_worker,
                started_ns=layer_started_ns,
                finished_ns=time.time_ns(),
                fallback_reason=layer_state.fallback_reason,
            )
    return None


def _run_frontend_serial_disabled_layers(
    module_order: Sequence[str],
    *,
    execution_context: _FrontendLayerExecutionContext,
    runtime_hooks: _FrontendLayerRuntimeHooks,
    frontend_parallel_layers: list[dict[str, Any]],
    frontend_parallel_config: _FrontendParallelConfig,
) -> dict[str, Any] | None:
    serial_layer_started_ns = time.time_ns()
    serial_layer_state = _fresh_frontend_parallel_layer_state()
    serial_error = _run_frontend_serial_layer_modules(
        module_order,
        module_graph=execution_context.module_graph,
        run_serial_frontend_lower=runtime_hooks.run_serial_frontend_lower,
        record_frontend_parallel_worker_timing=runtime_hooks.record_frontend_parallel_worker_timing,
        integrate_module_frontend_result=runtime_hooks.integrate_module_frontend_result,
        accumulate_midend_diagnostics=runtime_hooks.accumulate_midend_diagnostics,
        fail=runtime_hooks.fail,
        json_output=runtime_hooks.json_output,
        layer_state=serial_layer_state,
        layer_index=0,
        serial_mode="serial_disabled",
    )
    if serial_error is not None:
        return serial_error
    _append_frontend_serial_disabled_layer_detail(
        frontend_parallel_layers,
        module_order=module_order,
        serial_layer_state=serial_layer_state,
        frontend_module_costs=execution_context.frontend_module_costs,
        stdlib_like_by_module=execution_context.stdlib_like_by_module,
        frontend_parallel_config=frontend_parallel_config,
        serial_layer_started_ns=serial_layer_started_ns,
    )
    return None


def _run_frontend_parallel_layer_batches(
    candidates: Sequence[str],
    *,
    layer_workers: int,
    executor: Any,
    known_classes_snapshot_source: Mapping[str, Any],
    module_graph: dict[str, Path],
    module_sources: dict[str, str],
    project_root: Path | None,
    module_resolution_cache: _ModuleResolutionCache,
    parse_codec: ParseCodec,
    type_hint_policy: TypeHintPolicy,
    fallback_policy: FallbackPolicy,
    type_facts: dict[str, Any] | None,
    enable_phi: bool,
    known_modules: Collection[str],
    stdlib_allowlist: Collection[str],
    known_func_defaults: dict[str, dict[str, Any]],
    module_deps: dict[str, set[str]],
    module_chunk_max_ops: int,
    optimization_profile: str,
    pgo_hot_function_names: Collection[str],
    known_modules_sorted: tuple[str, ...],
    stdlib_allowlist_sorted: tuple[str, ...],
    pgo_hot_function_names_sorted: tuple[str, ...],
    module_dep_closures: dict[str, frozenset[str]],
    module_graph_metadata: _ModuleGraphMetadata,
    path_stat_by_module: Mapping[str, os.stat_result | None] | None,
    module_chunking: bool,
    scoped_lowering_inputs: _ScopedLoweringInputs | None,
    dirty_lowering_modules: Collection[str],
) -> tuple[_FrontendParallelLayerState, str | None, str | None]:
    layer_state = _fresh_frontend_parallel_layer_state()
    known_classes_snapshot = _known_classes_snapshot_copy(known_classes_snapshot_source)
    scoped_known_classes_by_module = _build_scoped_known_classes_snapshot(
        candidates,
        module_deps=module_deps,
        module_dep_closures=module_dep_closures,
        known_classes_snapshot=known_classes_snapshot,
    )
    for batch_start in range(0, len(candidates), layer_workers):
        batch = list(candidates[batch_start : batch_start + layer_workers])
        worker_submissions: list[_ParallelWorkerSubmission] = []
        (
            cached_results,
            worker_payloads,
            context_digest_by_module,
            batch_error,
        ) = _prepare_frontend_parallel_batch(
            batch,
            module_graph=module_graph,
            module_sources=module_sources,
            project_root=project_root,
            known_classes_snapshot=known_classes_snapshot,
            module_resolution_cache=module_resolution_cache,
            parse_codec=parse_codec,
            type_hint_policy=type_hint_policy,
            fallback_policy=fallback_policy,
            type_facts=type_facts,
            enable_phi=enable_phi,
            known_modules=known_modules,
            stdlib_allowlist=stdlib_allowlist,
            known_func_defaults=known_func_defaults,
            module_deps=module_deps,
            module_chunk_max_ops=module_chunk_max_ops,
            optimization_profile=optimization_profile,
            pgo_hot_function_names=pgo_hot_function_names,
            known_modules_sorted=known_modules_sorted,
            stdlib_allowlist_sorted=stdlib_allowlist_sorted,
            pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
            module_dep_closures=module_dep_closures,
            module_graph_metadata=module_graph_metadata,
            path_stat_by_module=path_stat_by_module,
            module_chunking=module_chunking,
            scoped_lowering_inputs=scoped_lowering_inputs,
            scoped_known_classes_by_module=scoped_known_classes_by_module,
            dirty_lowering_modules=dirty_lowering_modules,
        )
        if batch_error is not None:
            return layer_state, batch_error, None
        layer_state.context_digests.update(context_digest_by_module)
        for module_name, cached_result in cached_results.items():
            _record_parallel_cached_module_result(
                layer_state,
                module_name,
                cached_result,
            )
        for module_name, payload in worker_payloads:
            worker_submissions.append(
                _ParallelWorkerSubmission(
                    module_name=module_name,
                    submitted_ns=time.time_ns(),
                    future=executor.submit(_frontend_lower_module_worker, payload),
                )
            )
        for submission in worker_submissions:
            module_name = submission.module_name
            future = submission.future
            try:
                result = future.result()
                received_ns = time.time_ns()
                _record_parallel_worker_result(
                    layer_state,
                    module_name=module_name,
                    result=result,
                    submitted_ns=submission.submitted_ns,
                    received_ns=received_ns,
                )
            except Exception as exc:
                return layer_state, None, f"{module_graph[module_name]}: {exc}"
    return layer_state, None, None


def _fallback_frontend_parallel_layer_to_serial(
    *,
    frontend_parallel_details: MutableMapping[str, Any],
    warnings: list[str],
    failure_detail: str,
) -> _FrontendParallelLayerState:
    frontend_parallel_details["reason"] = "worker_error_fallback_serial"
    warnings.append(
        "Frontend parallel lowering fallback to serial for layer: "
        f"{failure_detail}"
    )
    fallback_state = _fresh_frontend_parallel_layer_state()
    fallback_state.fallback_reason = failure_detail
    return fallback_state


def _frontend_parallel_result_error(
    module_name: str,
    result: Mapping[str, Any],
) -> str | None:
    if bool(result.get("ok")):
        return None
    return str(result.get("error", f"Failed to lower module {module_name}"))


def _write_parallel_persisted_module_lowering(
    *,
    project_root: Path | None,
    module_path: Path,
    module_name: str,
    worker_mode: str,
    context_digest: str | None,
    result: Mapping[str, Any],
) -> None:
    if (
        project_root is None
        or worker_mode == "parallel_cache_hit"
        or context_digest is None
    ):
        return
    with contextlib.suppress(OSError):
        _write_persisted_module_lowering(
            project_root,
            module_path,
            module_name=module_name,
            is_package=module_path.name == "__init__.py",
            context_digest=context_digest,
            result={key: value for key, value in result.items() if key != "ok"},
        )


def _frontend_parallel_worker_timing_inputs(
    result_timings: _FrontendModuleResultTimings,
    worker_timing: Mapping[str, Any] | None,
) -> tuple[float, float, float, float, str, int | None]:
    total_ms = result_timings.total_s * 1000.0
    queue_ms = float((worker_timing or {}).get("queue_ms", 0.0))
    wait_ms = float((worker_timing or {}).get("wait_ms", 0.0))
    exec_ms = float((worker_timing or {}).get("exec_ms", total_ms))
    roundtrip_ms = float(
        (worker_timing or {}).get("roundtrip_ms", max(queue_ms + wait_ms, exec_ms))
    )
    worker_mode = str((worker_timing or {}).get("mode", "parallel"))
    worker_pid_raw = (worker_timing or {}).get("worker_pid")
    worker_pid = worker_pid_raw if isinstance(worker_pid_raw, int) else None
    return queue_ms, wait_ms, exec_ms, roundtrip_ms, worker_mode, worker_pid


def _record_parallel_layer_module_timing(
    *,
    layer_state: _FrontendParallelLayerState,
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]],
    layer_index: int,
    module_name: str,
    module_path: Path,
    result_timings: _FrontendModuleResultTimings,
    worker_timing: Mapping[str, Any] | None,
) -> str:
    (
        queue_ms,
        wait_ms,
        exec_ms,
        roundtrip_ms,
        worker_mode,
        worker_pid,
    ) = _frontend_parallel_worker_timing_inputs(result_timings, worker_timing)
    layer_state.recorded_worker_timings.append(
        record_frontend_parallel_worker_timing(
            layer_index=layer_index,
            module_name=module_name,
            module_path=module_path,
            mode=worker_mode,
            queue_ms=queue_ms,
            wait_ms=wait_ms,
            exec_ms=exec_ms,
            roundtrip_ms=roundtrip_ms,
            worker_pid=worker_pid,
        )
    )
    return worker_mode


def _consume_frontend_module_result(
    module_name: str,
    module_path: Path,
    result: Mapping[str, Any],
    *,
    result_timings: _FrontendModuleResultTimings | None = None,
    record_frontend_timing: Callable[..., None] | None,
    integrate_module_frontend_result: Callable[..., str | None],
    accumulate_midend_diagnostics: Callable[..., None],
    fail: Callable[[str, bool, str], dict[str, Any] | None],
    json_output: bool,
) -> dict[str, Any] | None:
    timings = result_timings or _frontend_result_timings(result)
    if record_frontend_timing is not None:
        record_frontend_timing(
            module_name=module_name,
            module_path=module_path,
            visit_s=timings.visit_s,
            lower_s=timings.lower_s,
            total_s=timings.total_s,
        )
    integration_error = integrate_module_frontend_result(
        module_name,
        ir_functions=cast(list[dict[str, Any]], result["functions"]),
        func_code_ids=cast(dict[str, int], result["func_code_ids"]),
        local_class_names=cast(list[str], result["local_class_names"]),
        local_classes=cast(dict[str, Any], result["local_classes"]),
    )
    if integration_error is not None:
        return fail(integration_error, json_output, command="build")
    accumulate_midend_diagnostics(
        module_name,
        policy_outcomes_by_func=cast(
            dict[str, dict[str, Any]],
            result.get("midend_policy_outcomes_by_function", {}),
        ),
        pass_stats_by_func=cast(
            dict[str, dict[str, dict[str, Any]]],
            result.get("midend_pass_stats_by_function", {}),
        ),
    )
    return None


def _consume_frontend_parallel_layer_result(
    *,
    layer_state: _FrontendParallelLayerState,
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]],
    record_frontend_timing: Callable[..., None],
    integrate_module_frontend_result: Callable[..., str | None],
    accumulate_midend_diagnostics: Callable[..., None],
    fail: Callable[[str, bool, str], dict[str, Any] | None],
    json_output: bool,
    project_root: Path | None,
    layer_index: int,
    module_name: str,
    module_path: Path,
    result: Mapping[str, Any],
) -> dict[str, Any] | None:
    result_error = _frontend_parallel_result_error(module_name, result)
    if result_error is not None:
        return fail(result_error, json_output, command="build")
    result_timings = _frontend_result_timings(result)
    worker_mode = _record_parallel_layer_module_timing(
        layer_state=layer_state,
        record_frontend_parallel_worker_timing=record_frontend_parallel_worker_timing,
        layer_index=layer_index,
        module_name=module_name,
        module_path=module_path,
        result_timings=result_timings,
        worker_timing=layer_state.worker_timings_by_module.get(module_name),
    )
    _write_parallel_persisted_module_lowering(
        project_root=project_root,
        module_path=module_path,
        module_name=module_name,
        worker_mode=worker_mode,
        context_digest=layer_state.context_digests.get(module_name),
        result=result,
    )
    return _consume_frontend_module_result(
        module_name=module_name,
        module_path=module_path,
        result=result,
        result_timings=result_timings,
        record_frontend_timing=record_frontend_timing,
        integrate_module_frontend_result=integrate_module_frontend_result,
        accumulate_midend_diagnostics=accumulate_midend_diagnostics,
        fail=fail,
        json_output=json_output,
    )


def _consume_frontend_serial_layer_result(
    *,
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]],
    integrate_module_frontend_result: Callable[..., str | None],
    accumulate_midend_diagnostics: Callable[..., None],
    fail: Callable[[str, bool, str], dict[str, Any] | None],
    json_output: bool,
    layer_state: _FrontendParallelLayerState,
    layer_index: int,
    module_name: str,
    module_path: Path,
    result: Mapping[str, Any],
    result_timings: _FrontendModuleResultTimings,
    serial_mode: str,
) -> dict[str, Any] | None:
    _record_serial_frontend_worker_timing(
        record_frontend_parallel_worker_timing=record_frontend_parallel_worker_timing,
        recorded_worker_timings=layer_state.recorded_worker_timings,
        layer_index=layer_index,
        module_name=module_name,
        module_path=module_path,
        mode=serial_mode,
        total_s=result_timings.total_s,
    )
    return _consume_frontend_module_result(
        module_name=module_name,
        module_path=module_path,
        result=result,
        result_timings=result_timings,
        record_frontend_timing=None,
        integrate_module_frontend_result=integrate_module_frontend_result,
        accumulate_midend_diagnostics=accumulate_midend_diagnostics,
        fail=fail,
        json_output=json_output,
    )


def _run_frontend_serial_layer_modules(
    module_names: Sequence[str],
    *,
    module_graph: Mapping[str, Path],
    run_serial_frontend_lower: Callable[
        [str, Path],
        tuple[dict[str, Any] | None, _FrontendModuleResultTimings | None, dict[str, Any] | None],
    ],
    record_frontend_parallel_worker_timing: Callable[..., dict[str, Any]],
    integrate_module_frontend_result: Callable[..., str | None],
    accumulate_midend_diagnostics: Callable[..., None],
    fail: Callable[[str, bool, str], dict[str, Any] | None],
    json_output: bool,
    layer_state: _FrontendParallelLayerState,
    layer_index: int,
    serial_mode: str,
) -> dict[str, Any] | None:
    for module_name in module_names:
        module_path = module_graph[module_name]
        result, result_timings, lower_error = run_serial_frontend_lower(
            module_name,
            module_path,
        )
        if lower_error is not None:
            return lower_error
        assert result is not None
        assert result_timings is not None
        consume_error = _consume_frontend_serial_layer_result(
            record_frontend_parallel_worker_timing=record_frontend_parallel_worker_timing,
            integrate_module_frontend_result=integrate_module_frontend_result,
            accumulate_midend_diagnostics=accumulate_midend_diagnostics,
            fail=fail,
            json_output=json_output,
            layer_state=layer_state,
            layer_index=layer_index,
            module_name=module_name,
            module_path=module_path,
            result=result,
            result_timings=result_timings,
            serial_mode=serial_mode,
        )
        if consume_error is not None:
            return consume_error
    return None


def _run_frontend_layer(
    layer: Sequence[str],
    *,
    layer_index: int,
    executor: Any | None,
    execution_context: _FrontendLayerExecutionContext,
    runtime_hooks: _FrontendLayerRuntimeHooks,
    frontend_parallel_config: _FrontendParallelConfig,
    parallel_pool_usable: bool,
) -> tuple[_FrontendLayerRunResult | None, dict[str, Any] | None]:
    layer_state = _fresh_frontend_parallel_layer_state()
    layer_plan = _frontend_layer_plan(
        layer,
        syntax_error_modules=execution_context.syntax_error_modules,
        module_sources=execution_context.module_sources,
        module_deps=execution_context.module_deps,
        frontend_module_costs=execution_context.frontend_module_costs,
        stdlib_like_by_module=execution_context.stdlib_like_by_module,
        frontend_parallel_config=frontend_parallel_config,
        parallel_pool_usable=parallel_pool_usable,
    )
    if layer_plan.mode == "parallel":
        assert executor is not None
        layer_state, batch_error, layer_failure_detail = (
            _run_frontend_parallel_layer_batches(
                layer_plan.candidates,
                layer_workers=layer_plan.workers,
                executor=executor,
                known_classes_snapshot_source=execution_context.known_classes,
                module_graph=execution_context.module_graph,
                module_sources=execution_context.module_sources,
                project_root=execution_context.project_root,
                module_resolution_cache=execution_context.module_resolution_cache,
                parse_codec=execution_context.parse_codec,
                type_hint_policy=execution_context.type_hint_policy,
                fallback_policy=execution_context.fallback_policy,
                type_facts=execution_context.type_facts,
                enable_phi=execution_context.enable_phi,
                known_modules=execution_context.known_modules,
                stdlib_allowlist=execution_context.stdlib_allowlist,
                known_func_defaults=execution_context.known_func_defaults,
                module_deps=execution_context.module_deps,
                module_chunk_max_ops=execution_context.module_chunk_max_ops,
                optimization_profile=execution_context.optimization_profile,
                pgo_hot_function_names=execution_context.pgo_hot_function_names,
                known_modules_sorted=execution_context.known_modules_sorted,
                stdlib_allowlist_sorted=execution_context.stdlib_allowlist_sorted,
                pgo_hot_function_names_sorted=execution_context.pgo_hot_function_names_sorted,
                module_dep_closures=execution_context.module_dep_closures,
                module_graph_metadata=execution_context.module_graph_metadata,
                path_stat_by_module=execution_context.path_stat_by_module,
                module_chunking=execution_context.module_chunking,
                scoped_lowering_inputs=execution_context.scoped_lowering_inputs,
                dirty_lowering_modules=execution_context.dirty_lowering_modules,
            )
        )
        if batch_error is not None:
            return None, runtime_hooks.fail(
                batch_error, runtime_hooks.json_output, command="build"
            )
        if layer_failure_detail is not None:
            layer_state = _fallback_frontend_parallel_layer_to_serial(
                frontend_parallel_details=runtime_hooks.frontend_parallel_details,
                warnings=runtime_hooks.warnings,
                failure_detail=layer_failure_detail,
            )
            layer_plan = _FrontendLayerPlan(
                candidates=layer_plan.candidates,
                predicted_cost_total=layer_plan.predicted_cost_total,
                effective_min_predicted_cost=layer_plan.effective_min_predicted_cost,
                stdlib_candidates=layer_plan.stdlib_candidates,
                workers=1,
                policy_reason="worker_error_fallback_serial",
                mode="serial_fallback",
            )
            parallel_pool_usable = False

    for module_name in layer:
        module_path = execution_context.module_graph[module_name]
        result = layer_state.results.get(module_name)
        if result is not None:
            consume_error = _consume_frontend_parallel_layer_result(
                layer_state=layer_state,
                record_frontend_parallel_worker_timing=runtime_hooks.record_frontend_parallel_worker_timing,
                record_frontend_timing=runtime_hooks.record_frontend_timing,
                integrate_module_frontend_result=runtime_hooks.integrate_module_frontend_result,
                accumulate_midend_diagnostics=runtime_hooks.accumulate_midend_diagnostics,
                fail=runtime_hooks.fail,
                json_output=runtime_hooks.json_output,
                project_root=execution_context.project_root,
                layer_index=layer_index,
                module_name=module_name,
                module_path=module_path,
                result=result,
            )
            if consume_error is not None:
                return None, consume_error
            continue
        serial_error = _run_frontend_serial_layer_modules(
            [module_name],
            module_graph=execution_context.module_graph,
            run_serial_frontend_lower=runtime_hooks.run_serial_frontend_lower,
            record_frontend_parallel_worker_timing=runtime_hooks.record_frontend_parallel_worker_timing,
            integrate_module_frontend_result=runtime_hooks.integrate_module_frontend_result,
            accumulate_midend_diagnostics=runtime_hooks.accumulate_midend_diagnostics,
            fail=runtime_hooks.fail,
            json_output=runtime_hooks.json_output,
            layer_state=layer_state,
            layer_index=layer_index,
            serial_mode=_frontend_serial_worker_mode(layer_plan.mode),
        )
        if serial_error is not None:
            return None, serial_error
    return (
        _FrontendLayerRunResult(
            layer_state=layer_state,
            layer_plan=layer_plan,
            parallel_pool_usable=parallel_pool_usable,
        ),
        None,
    )


def _frontend_serial_worker_mode(layer_mode: str) -> str:
    if layer_mode == "serial_fallback":
        return "serial_fallback"
    if layer_mode == "serial_layer_policy":
        return "serial_layer_policy"
    return "serial"


def _module_lowering_context_payload(
    module_name: str,
    module_path: Path,
    *,
    logical_source_path: str,
    entry_override: str | None,
    known_classes_snapshot: dict[str, Any],
    parse_codec: ParseCodec,
    type_hint_policy: TypeHintPolicy,
    fallback_policy: FallbackPolicy,
    type_facts: dict[str, Any] | None,
    enable_phi: bool,
    known_modules: Collection[str],
    stdlib_allowlist: Collection[str],
    known_func_defaults: dict[str, dict[str, Any]],
    module_deps: dict[str, set[str]],
    module_is_namespace: bool,
    module_chunking: bool,
    module_chunk_max_ops: int,
    optimization_profile: str,
    pgo_hot_function_names: Collection[str],
    known_modules_sorted: tuple[str, ...] | None = None,
    stdlib_allowlist_sorted: tuple[str, ...] | None = None,
    pgo_hot_function_names_sorted: tuple[str, ...] | None = None,
    module_dep_closures: dict[str, frozenset[str]] | None = None,
    scoped_lowering_inputs: _ScopedLoweringInputs | None = None,
    scoped_inputs: _ScopedLoweringInputView | None = None,
    scoped_known_classes_by_module: Mapping[str, dict[str, Any]] | None = None,
    scoped_known_classes: dict[str, Any] | None = None,
    is_package: bool | None = None,
    path_stat: os.stat_result | None = None,
) -> dict[str, Any] | None:
    if path_stat is None:
        try:
            path_stat = module_path.stat()
        except OSError:
            return None
    if scoped_inputs is None:
        scoped_inputs = _scoped_lowering_input_view(
            module_name,
            module_deps=module_deps,
            known_modules=known_modules,
            known_func_defaults=known_func_defaults,
            pgo_hot_function_names=pgo_hot_function_names,
            type_facts=cast(TypeFacts | None, type_facts),
            module_dep_closures=module_dep_closures,
            scoped_lowering_inputs=scoped_lowering_inputs,
            known_modules_sorted=known_modules_sorted,
            pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
        )
    known_modules_sorted = scoped_inputs.known_modules
    if stdlib_allowlist_sorted is None:
        stdlib_allowlist_sorted = tuple(sorted(stdlib_allowlist))
    pgo_hot_function_names_sorted = scoped_inputs.pgo_hot_function_names
    scoped_known_func_defaults = scoped_inputs.known_func_defaults
    if scoped_known_classes is None:
        scoped_known_classes = _scoped_known_classes_view(
            module_name,
            module_deps=module_deps,
            known_classes_snapshot=known_classes_snapshot,
            module_dep_closures=module_dep_closures,
            scoped_known_classes_by_module=scoped_known_classes_by_module,
        )
    scoped_type_facts = scoped_inputs.type_facts
    if is_package is None:
        is_package = module_path.name == "__init__.py"
    return {
        "version": 1,
        "module_name": module_name,
        "logical_source_path": logical_source_path,
        "is_package": is_package,
        "module_is_namespace": module_is_namespace,
        "entry_module": entry_override,
        "size": path_stat.st_size,
        "mtime_ns": path_stat.st_mtime_ns,
        "parse_codec": parse_codec,
        "type_hint_policy": type_hint_policy,
        "fallback_policy": fallback_policy,
        "type_facts": scoped_type_facts,
        "enable_phi": enable_phi,
        "known_modules": known_modules_sorted,
        "known_classes": scoped_known_classes,
        "stdlib_allowlist": stdlib_allowlist_sorted,
        "known_func_defaults": scoped_known_func_defaults,
        "module_chunking": module_chunking,
        "module_chunk_max_ops": module_chunk_max_ops,
        "optimization_profile": optimization_profile,
        "pgo_hot_functions": pgo_hot_function_names_sorted,
    }


def _module_lowering_context_digest(payload: dict[str, Any]) -> str | None:
    try:
        encoded = json.dumps(
            payload,
            sort_keys=True,
            separators=(",", ":"),
            default=_json_ir_default,
        ).encode("utf-8")
    except (TypeError, ValueError):
        return None
    return hashlib.sha256(encoded).hexdigest()


def _module_lowering_context_digest_for_module(
    module_name: str,
    module_path: Path,
    *,
    logical_source_path: str,
    entry_override: str | None,
    known_classes_snapshot: dict[str, Any],
    parse_codec: ParseCodec,
    type_hint_policy: TypeHintPolicy,
    fallback_policy: FallbackPolicy,
    type_facts: dict[str, Any] | None,
    enable_phi: bool,
    known_modules: Collection[str],
    stdlib_allowlist: Collection[str],
    known_func_defaults: dict[str, dict[str, Any]],
    module_deps: dict[str, set[str]],
    module_is_namespace: bool,
    module_chunking: bool,
    module_chunk_max_ops: int,
    optimization_profile: str,
    pgo_hot_function_names: Collection[str],
    known_modules_sorted: tuple[str, ...] | None = None,
    stdlib_allowlist_sorted: tuple[str, ...] | None = None,
    pgo_hot_function_names_sorted: tuple[str, ...] | None = None,
    module_dep_closures: dict[str, frozenset[str]] | None = None,
    scoped_lowering_inputs: _ScopedLoweringInputs | None = None,
    scoped_inputs: _ScopedLoweringInputView | None = None,
    scoped_known_classes_by_module: Mapping[str, dict[str, Any]] | None = None,
    scoped_known_classes: dict[str, Any] | None = None,
    is_package: bool | None = None,
    path_stat: os.stat_result | None = None,
) -> str | None:
    context_payload = _module_lowering_context_payload(
        module_name,
        module_path,
        logical_source_path=logical_source_path,
        entry_override=entry_override,
        known_classes_snapshot=known_classes_snapshot,
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
        type_facts=type_facts,
        enable_phi=enable_phi,
        known_modules=known_modules,
        stdlib_allowlist=stdlib_allowlist,
        known_func_defaults=known_func_defaults,
        module_deps=module_deps,
        module_is_namespace=module_is_namespace,
        module_chunking=module_chunking,
        module_chunk_max_ops=module_chunk_max_ops,
        optimization_profile=optimization_profile,
        pgo_hot_function_names=pgo_hot_function_names,
        known_modules_sorted=known_modules_sorted,
        stdlib_allowlist_sorted=stdlib_allowlist_sorted,
        pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
        module_dep_closures=module_dep_closures,
        scoped_lowering_inputs=scoped_lowering_inputs,
        scoped_inputs=scoped_inputs,
        scoped_known_classes_by_module=scoped_known_classes_by_module,
        scoped_known_classes=scoped_known_classes,
        is_package=is_package,
        path_stat=path_stat,
    )
    if context_payload is None:
        return None
    return _module_lowering_context_digest(context_payload)


def _read_persisted_module_lowering(
    project_root: Path,
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    context_digest: str,
    path_stat: os.stat_result | None = None,
) -> dict[str, Any] | None:
    cache_path = _module_lowering_cache_path(
        project_root,
        path,
        module_name=module_name,
        is_package=is_package,
    )
    payload = _read_cached_json_object(cache_path)
    if payload is None:
        return None
    if not isinstance(payload, dict) or payload.get("version") != 1:
        return None
    if payload.get("context_digest") != context_digest:
        return None
    if path_stat is None:
        try:
            path_stat = path.stat()
        except OSError:
            return None
    if (
        payload.get("size") != path_stat.st_size
        or payload.get("mtime_ns") != path_stat.st_mtime_ns
    ):
        return None
    raw_result = payload.get("result")
    if not isinstance(raw_result, dict):
        return None
    return cast(dict[str, Any], copy.deepcopy(_decode_cached_json_value(raw_result)))


def _write_persisted_module_lowering(
    project_root: Path,
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    context_digest: str,
    result: dict[str, Any],
) -> None:
    cache_path = _module_lowering_cache_path(
        project_root,
        path,
        module_name=module_name,
        is_package=is_package,
    )
    stat = path.stat()
    payload = {
        "version": 1,
        "context_digest": context_digest,
        "size": stat.st_size,
        "mtime_ns": stat.st_mtime_ns,
        "result": result,
    }
    cache_path.parent.mkdir(parents=True, exist_ok=True)
    _write_cached_json_object(cache_path, payload, default=_json_ir_default)


def _load_cached_module_lowering_result(
    project_root: Path | None,
    module_name: str,
    module_path: Path,
    *,
    logical_source_path: str,
    entry_override: str | None,
    is_package: bool,
    known_classes_snapshot: dict[str, Any],
    parse_codec: ParseCodec,
    type_hint_policy: TypeHintPolicy,
    fallback_policy: FallbackPolicy,
    type_facts: dict[str, Any] | None,
    enable_phi: bool,
    known_modules: Collection[str],
    stdlib_allowlist: Collection[str],
    known_func_defaults: dict[str, dict[str, Any]],
    module_deps: dict[str, set[str]],
    module_is_namespace: bool,
    module_chunking: bool,
    module_chunk_max_ops: int,
    optimization_profile: str,
    pgo_hot_function_names: Collection[str],
    known_modules_sorted: tuple[str, ...] | None = None,
    stdlib_allowlist_sorted: tuple[str, ...] | None = None,
    pgo_hot_function_names_sorted: tuple[str, ...] | None = None,
    module_dep_closures: dict[str, frozenset[str]] | None = None,
    scoped_lowering_inputs: _ScopedLoweringInputs | None = None,
    scoped_inputs: _ScopedLoweringInputView | None = None,
    scoped_known_classes_by_module: Mapping[str, dict[str, Any]] | None = None,
    scoped_known_classes: dict[str, Any] | None = None,
    context_digest: str | None = None,
    resolution_cache: _ModuleResolutionCache | None = None,
    path_stat: os.stat_result | None = None,
) -> dict[str, Any] | None:
    if project_root is None:
        return None
    if path_stat is None and resolution_cache is not None:
        with contextlib.suppress(OSError):
            path_stat = resolution_cache.path_stat(module_path)
    if context_digest is None:
        context_digest = _module_lowering_context_digest_for_module(
            module_name,
            module_path,
            logical_source_path=logical_source_path,
            entry_override=entry_override,
            known_classes_snapshot=known_classes_snapshot,
            parse_codec=parse_codec,
            type_hint_policy=type_hint_policy,
            fallback_policy=fallback_policy,
            type_facts=type_facts,
            enable_phi=enable_phi,
            known_modules=known_modules,
            stdlib_allowlist=stdlib_allowlist,
            known_func_defaults=known_func_defaults,
            module_deps=module_deps,
            module_is_namespace=module_is_namespace,
            module_chunking=module_chunking,
            module_chunk_max_ops=module_chunk_max_ops,
            optimization_profile=optimization_profile,
            pgo_hot_function_names=pgo_hot_function_names,
            known_modules_sorted=known_modules_sorted,
            stdlib_allowlist_sorted=stdlib_allowlist_sorted,
            pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
            module_dep_closures=module_dep_closures,
            scoped_lowering_inputs=scoped_lowering_inputs,
            scoped_inputs=scoped_inputs,
            scoped_known_classes_by_module=scoped_known_classes_by_module,
            scoped_known_classes=scoped_known_classes,
            is_package=is_package,
            path_stat=path_stat,
        )
        if context_digest is None:
            return None
    return _read_persisted_module_lowering(
        project_root,
        module_path,
        module_name=module_name,
        is_package=is_package,
        context_digest=context_digest,
        path_stat=path_stat,
    )


def _module_worker_payload(
    module_name: str,
    *,
    module_path: Path,
    logical_source_path: str,
    source: str,
    parse_codec: ParseCodec,
    type_hint_policy: TypeHintPolicy,
    fallback_policy: FallbackPolicy,
    module_is_namespace: bool,
    entry_module: str | None,
    type_facts: dict[str, Any] | None,
    enable_phi: bool,
    known_modules: Collection[str],
    known_classes_snapshot: dict[str, Any],
    stdlib_allowlist_sorted: Collection[str],
    known_func_defaults: dict[str, dict[str, Any]],
    module_deps: dict[str, set[str]],
    module_chunking: bool,
    module_chunk_max_ops: int,
    optimization_profile: str,
    pgo_hot_function_names: Collection[str],
    module_dep_closures: dict[str, frozenset[str]],
    scoped_lowering_inputs: _ScopedLoweringInputs | None = None,
    scoped_inputs: _ScopedLoweringInputView | None = None,
    scoped_known_classes_by_module: Mapping[str, dict[str, Any]] | None = None,
    scoped_known_classes: dict[str, Any] | None = None,
    stdlib_allowlist_payload: list[str] | None = None,
) -> dict[str, Any]:
    if scoped_inputs is None:
        scoped_inputs = _scoped_lowering_input_view(
            module_name,
            module_deps=module_deps,
            known_modules=known_modules,
            known_func_defaults=known_func_defaults,
            pgo_hot_function_names=pgo_hot_function_names,
            type_facts=cast(TypeFacts | None, type_facts),
            module_dep_closures=module_dep_closures,
            scoped_lowering_inputs=scoped_lowering_inputs,
            known_modules_sorted=tuple(known_modules),
            pgo_hot_function_names_sorted=tuple(pgo_hot_function_names),
        )
    if stdlib_allowlist_payload is None:
        stdlib_allowlist_payload = list(stdlib_allowlist_sorted)
    if scoped_known_classes is None:
        scoped_known_classes = _scoped_known_classes_view(
            module_name,
            module_deps=module_deps,
            known_classes_snapshot=known_classes_snapshot,
            module_dep_closures=module_dep_closures,
            scoped_known_classes_by_module=scoped_known_classes_by_module,
        )
    return {
        "module_name": module_name,
        "module_path": str(module_path),
        "logical_source_path": logical_source_path,
        "source": source,
        "parse_codec": parse_codec,
        "type_hint_policy": type_hint_policy,
        "fallback_policy": fallback_policy,
        "module_is_namespace": module_is_namespace,
        "entry_module": entry_module,
        "enable_phi": enable_phi,
        "known_modules": scoped_inputs.known_modules_payload,
        "known_classes": scoped_known_classes,
        "stdlib_allowlist": stdlib_allowlist_payload,
        "known_func_defaults": scoped_inputs.known_func_defaults,
        "module_chunking": module_chunking,
        "module_chunk_max_ops": module_chunk_max_ops,
        "optimization_profile": optimization_profile,
        "pgo_hot_functions": scoped_inputs.pgo_hot_function_names_payload,
        "type_facts": scoped_inputs.type_facts,
    }


def _prepare_frontend_parallel_batch(
    batch: list[str],
    *,
    module_graph: dict[str, Path],
    module_sources: dict[str, str],
    project_root: Path | None,
    known_classes_snapshot: dict[str, Any],
    module_resolution_cache: _ModuleResolutionCache,
    parse_codec: ParseCodec,
    type_hint_policy: TypeHintPolicy,
    fallback_policy: FallbackPolicy,
    type_facts: dict[str, Any] | None,
    enable_phi: bool,
    known_modules: Collection[str],
    stdlib_allowlist: Collection[str],
    known_func_defaults: dict[str, dict[str, Any]],
    module_deps: dict[str, set[str]],
    module_chunk_max_ops: int,
    optimization_profile: str,
    pgo_hot_function_names: Collection[str],
    known_modules_sorted: tuple[str, ...],
    stdlib_allowlist_sorted: tuple[str, ...],
    pgo_hot_function_names_sorted: tuple[str, ...],
    module_dep_closures: dict[str, frozenset[str]],
    module_graph_metadata: _ModuleGraphMetadata,
    path_stat_by_module: Mapping[str, os.stat_result | None] | None = None,
    module_chunking: bool,
    scoped_lowering_inputs: _ScopedLoweringInputs | None = None,
    scoped_known_classes_by_module: Mapping[str, dict[str, Any]] | None = None,
    dirty_lowering_modules: Collection[str],
) -> tuple[
    dict[str, dict[str, Any]],
    list[tuple[str, dict[str, Any]]],
    dict[str, str],
    str | None,
]:
    cached_results: dict[str, dict[str, Any]] = {}
    worker_payloads: list[tuple[str, dict[str, Any]]] = []
    context_digest_by_module: dict[str, str] = {}
    dirty_lowering = set(dirty_lowering_modules)
    stdlib_allowlist_payload = list(stdlib_allowlist_sorted)
    if scoped_known_classes_by_module is None:
        scoped_known_classes_by_module = _build_scoped_known_classes_snapshot(
            batch,
            module_deps=module_deps,
            module_dep_closures=module_dep_closures,
            known_classes_snapshot=known_classes_snapshot,
        )
    for module_name in batch:
        module_path = module_graph[module_name]
        execution_view = _module_lowering_execution_view(
            module_name,
            module_path=module_path,
            module_graph_metadata=module_graph_metadata,
            module_deps=module_deps,
            known_modules=known_modules,
            known_func_defaults=known_func_defaults,
            pgo_hot_function_names=pgo_hot_function_names,
            type_facts=cast(TypeFacts | None, type_facts),
            known_classes_snapshot=known_classes_snapshot,
            module_dep_closures=module_dep_closures,
            path_stat_by_module=path_stat_by_module,
            scoped_lowering_inputs=scoped_lowering_inputs,
            known_modules_sorted=known_modules_sorted,
            pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
            scoped_known_classes_by_module=scoped_known_classes_by_module,
        )
        metadata_view = execution_view.metadata
        scoped_inputs = execution_view.scoped_inputs
        logical_source_path = metadata_view.logical_source_path
        entry_override = metadata_view.entry_override
        module_is_namespace = metadata_view.module_is_namespace
        is_package = metadata_view.is_package
        path_stat = metadata_view.path_stat
        scoped_known_classes = execution_view.scoped_known_classes
        if project_root is not None:
            context_digest = _module_lowering_context_digest_for_module(
                module_name,
                module_path,
                logical_source_path=logical_source_path,
                entry_override=entry_override,
                known_classes_snapshot=known_classes_snapshot,
                parse_codec=parse_codec,
                type_hint_policy=type_hint_policy,
                fallback_policy=fallback_policy,
                type_facts=type_facts,
                enable_phi=enable_phi,
                known_modules=known_modules,
                stdlib_allowlist=stdlib_allowlist,
                known_func_defaults=known_func_defaults,
                module_deps=module_deps,
                module_is_namespace=module_is_namespace,
                module_chunking=module_chunking,
                module_chunk_max_ops=module_chunk_max_ops,
                optimization_profile=optimization_profile,
                pgo_hot_function_names=pgo_hot_function_names,
                known_modules_sorted=known_modules_sorted,
                stdlib_allowlist_sorted=stdlib_allowlist_sorted,
                pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
                module_dep_closures=module_dep_closures,
                scoped_lowering_inputs=scoped_lowering_inputs,
                scoped_inputs=scoped_inputs,
                scoped_known_classes_by_module=scoped_known_classes_by_module,
                scoped_known_classes=scoped_known_classes,
                is_package=is_package,
                path_stat=path_stat,
            )
            if context_digest is not None:
                context_digest_by_module[module_name] = context_digest
        if module_name not in dirty_lowering:
            cached_result = _load_cached_module_lowering_result(
                project_root,
                module_name,
                module_path,
                logical_source_path=logical_source_path,
                entry_override=entry_override,
                is_package=is_package,
                known_classes_snapshot=known_classes_snapshot,
                parse_codec=parse_codec,
                type_hint_policy=type_hint_policy,
                fallback_policy=fallback_policy,
                type_facts=type_facts,
                enable_phi=enable_phi,
                known_modules=known_modules,
                stdlib_allowlist=stdlib_allowlist,
                known_func_defaults=known_func_defaults,
                module_deps=module_deps,
                module_is_namespace=module_is_namespace,
                module_chunking=module_chunking,
                module_chunk_max_ops=module_chunk_max_ops,
                optimization_profile=optimization_profile,
                pgo_hot_function_names=pgo_hot_function_names,
                known_modules_sorted=known_modules_sorted,
                stdlib_allowlist_sorted=stdlib_allowlist_sorted,
                pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
                module_dep_closures=module_dep_closures,
                scoped_lowering_inputs=scoped_lowering_inputs,
                scoped_inputs=scoped_inputs,
                scoped_known_classes_by_module=scoped_known_classes_by_module,
                scoped_known_classes=scoped_known_classes,
                context_digest=context_digest_by_module.get(module_name),
                resolution_cache=module_resolution_cache,
                path_stat=path_stat,
            )
            if cached_result is not None:
                cached_results[module_name] = cached_result
                continue
        source = module_sources.get(module_name)
        if source is None:
            try:
                source = module_resolution_cache.read_module_source(module_path)
            except (SyntaxError, UnicodeDecodeError) as exc:
                return {}, [], {}, f"Syntax error in {module_path}: {exc}"
            except OSError as exc:
                return {}, [], {}, f"Failed to read module {module_path}: {exc}"
            module_sources[module_name] = source
        worker_payloads.append(
            (
                module_name,
                _module_worker_payload(
                    module_name,
                    module_path=module_path,
                    logical_source_path=logical_source_path,
                    source=source,
                    parse_codec=parse_codec,
                    type_hint_policy=type_hint_policy,
                    fallback_policy=fallback_policy,
                    module_is_namespace=module_is_namespace,
                    entry_module=entry_override,
                    type_facts=type_facts,
                    enable_phi=enable_phi,
                    known_modules=known_modules_sorted,
                    known_classes_snapshot=known_classes_snapshot,
                    stdlib_allowlist_sorted=stdlib_allowlist_sorted,
                    stdlib_allowlist_payload=stdlib_allowlist_payload,
                    known_func_defaults=known_func_defaults,
                    module_deps=module_deps,
                    module_chunking=module_chunking,
                    module_chunk_max_ops=module_chunk_max_ops,
                    optimization_profile=optimization_profile,
                    pgo_hot_function_names=pgo_hot_function_names_sorted,
                    module_dep_closures=module_dep_closures,
                    scoped_lowering_inputs=scoped_lowering_inputs,
                    scoped_inputs=scoped_inputs,
                    scoped_known_classes_by_module=scoped_known_classes_by_module,
                    scoped_known_classes=scoped_known_classes,
                ),
            )
        )
    return cached_results, worker_payloads, context_digest_by_module, None


def _link_fingerprint(
    *,
    project_root: Path,
    inputs: list[Path],
    link_cmd: list[str],
    stored_fingerprint: dict[str, Any] | None = None,
) -> dict[str, str | None] | None:
    inputs_meta = _hash_source_tree_metadata(inputs, project_root)
    inputs_digest = inputs_meta[0] if inputs_meta is not None else None
    if _stored_fingerprint_matches_source_metadata(
        stored_fingerprint,
        inputs_digest=inputs_digest,
        rustc=None,
    ):
        return {
            "hash": cast(str, stored_fingerprint.get("hash")),
            "rustc": None,
            "inputs_digest": inputs_digest,
        }
    hasher = hashlib.sha256()
    hasher.update("\0".join(link_cmd).encode("utf-8"))
    hasher.update(b"\0")
    try:
        for path in inputs:
            _hash_runtime_file(path, project_root, hasher)
    except OSError:
        return None
    return {
        "hash": hasher.hexdigest(),
        "rustc": None,
        "inputs_digest": inputs_digest,
    }


def _backend_fingerprint(
    project_root: Path,
    *,
    cargo_profile: str,
    rustflags: str,
    stored_fingerprint: dict[str, Any] | None = None,
) -> dict[str, str | None] | None:
    meta = f"profile:{cargo_profile}\n"
    meta += f"rustflags:{rustflags}\n"
    source_paths = _backend_source_paths(project_root)
    rustc_info = _rustc_version()
    inputs_meta = _hash_source_tree_metadata(source_paths, project_root)
    inputs_digest = inputs_meta[0] if inputs_meta is not None else None
    if _stored_fingerprint_matches_source_metadata(
        stored_fingerprint,
        inputs_digest=inputs_digest,
        rustc=rustc_info,
    ):
        return {
            "hash": cast(str, stored_fingerprint.get("hash")),
            "rustc": rustc_info,
            "inputs_digest": inputs_digest,
        }

    hasher = hashlib.sha256()
    hasher.update(meta.encode("utf-8"))
    try:
        for path in sorted(source_paths, key=lambda p: str(p)):
            if path.is_dir():
                for item in sorted(path.rglob("*"), key=lambda p: str(p)):
                    if item.is_file():
                        _hash_runtime_file(item, project_root, hasher)
            elif path.exists():
                _hash_runtime_file(path, project_root, hasher)
    except OSError:
        return None
    return {
        "hash": hasher.hexdigest(),
        "rustc": rustc_info,
        "inputs_digest": inputs_digest,
    }


def _ensure_backend_binary(
    backend_bin: Path,
    *,
    cargo_timeout: float | None,
    json_output: bool,
    cargo_profile: str,
    project_root: Path,
) -> bool:
    rustflags = os.environ.get("RUSTFLAGS", "")
    fingerprint_path = _backend_fingerprint_path(
        project_root, backend_bin, cargo_profile
    )
    stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
    fingerprint = _backend_fingerprint(
        project_root,
        cargo_profile=cargo_profile,
        rustflags=rustflags,
        stored_fingerprint=stored_fingerprint,
    )
    lock_name = f"backend.{cargo_profile}"
    with _build_lock(project_root, lock_name):
        if stored_fingerprint is None:
            stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
        if not _artifact_needs_rebuild(backend_bin, fingerprint, stored_fingerprint):
            return True
        if not json_output:
            print("Backend sources changed; rebuilding backend...")
        cmd = [
            "cargo",
            "build",
            "--package",
            "molt-backend",
            "--profile",
            cargo_profile,
        ]
        build_env = os.environ.copy()
        _maybe_enable_sccache(build_env)
        try:
            build = _run_cargo_with_sccache_retry(
                cmd,
                cwd=project_root,
                env=build_env,
                timeout=cargo_timeout,
                json_output=json_output,
                label="Backend build",
            )
        except subprocess.TimeoutExpired:
            if not json_output:
                timeout_note = (
                    f"Backend build timed out after {cargo_timeout:.1f}s."
                    if cargo_timeout is not None
                    else "Backend build timed out."
                )
                print(timeout_note, file=sys.stderr)
            return False
        if build.returncode != 0:
            if not json_output:
                err = build.stderr.strip() or build.stdout.strip()
                if err:
                    print(err, file=sys.stderr)
            return False
        if fingerprint is not None:
            try:
                fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
                _write_runtime_fingerprint(fingerprint_path, fingerprint)
            except OSError:
                if not json_output:
                    print(
                        "Warning: failed to write backend fingerprint metadata.",
                        file=sys.stderr,
                    )
    return True


def _ensure_runtime_lib(
    runtime_lib: Path,
    target_triple: str | None,
    json_output: bool,
    cargo_profile: str,
    project_root: Path,
    cargo_timeout: float | None,
) -> bool:
    rustflags = os.environ.get("RUSTFLAGS", "")
    runtime_features = _runtime_cargo_features(target_triple)
    fingerprint_path = _runtime_fingerprint_path(
        project_root, runtime_lib, cargo_profile, target_triple
    )
    stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
    fingerprint = _runtime_fingerprint(
        project_root,
        cargo_profile=cargo_profile,
        target_triple=target_triple,
        rustflags=rustflags,
        runtime_features=runtime_features,
        stored_fingerprint=stored_fingerprint,
    )
    lock_target = target_triple or "native"
    lock_name = f"runtime.{cargo_profile}.{lock_target}"
    with _build_lock(project_root, lock_name):
        if stored_fingerprint is None:
            stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
        if not _artifact_needs_rebuild(runtime_lib, fingerprint, stored_fingerprint):
            return True
        if not json_output:
            print("Runtime sources changed; rebuilding runtime...")
        cmd = ["cargo", "build", "-p", "molt-runtime", "--profile", cargo_profile]
        if runtime_features:
            cmd.extend(["--features", ",".join(runtime_features)])
        if target_triple:
            cmd.extend(["--target", target_triple])
        build_env = os.environ.copy()
        _maybe_enable_sccache(build_env)
        try:
            build = _run_cargo_with_sccache_retry(
                cmd,
                cwd=project_root,
                env=build_env,
                timeout=cargo_timeout,
                json_output=json_output,
                label="Runtime build",
            )
        except subprocess.TimeoutExpired:
            if not json_output:
                timeout_note = (
                    f"Runtime build timed out after {cargo_timeout:.1f}s."
                    if cargo_timeout is not None
                    else "Runtime build timed out."
                )
                print(timeout_note, file=sys.stderr)
            return False
        if build.returncode != 0:
            err = build.stderr.strip() or build.stdout.strip()
            if err:
                print(err, file=sys.stderr)
            return False
        if fingerprint is not None:
            try:
                fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
                _write_runtime_fingerprint(fingerprint_path, fingerprint)
            except OSError:
                if not json_output:
                    print(
                        "Warning: failed to write runtime fingerprint metadata.",
                        file=sys.stderr,
                    )
    return True


def _append_rustflags(env: MutableMapping[str, str], flags: str) -> None:
    existing = env.get("RUSTFLAGS", "")
    joined = f"{existing} {flags}".strip()
    env["RUSTFLAGS"] = joined


def _configure_wasm_cc_env(env: dict[str, str]) -> None:
    if env.get("CC_wasm32-wasip1") or env.get("CC_wasm32_wasip1"):
        return
    for candidate in (
        "/opt/homebrew/opt/llvm/bin/clang",
        "/usr/local/opt/llvm/bin/clang",
    ):
        cc_path = Path(candidate)
        if cc_path.exists() and os.access(cc_path, os.X_OK):
            env["CC_wasm32-wasip1"] = str(cc_path)
            env["CC_wasm32_wasip1"] = str(cc_path)
            return


def _wasm_runtime_artifact_path(target_root: Path, profile_dir: str) -> Path:
    return target_root / "wasm32-wasip1" / profile_dir / "molt_runtime.wasm"


def _wasm_runtime_recovery_target_root(target_root: Path) -> Path:
    return target_root.parent / f"{target_root.name}-wasm-runtime-recovery"


def _run_runtime_wasm_cargo_build(
    *,
    cmd: list[str],
    root: Path,
    env: dict[str, str],
    cargo_timeout: float | None,
    profile_dir: str,
    target_root_override: Path | None = None,
    json_output: bool,
) -> tuple[subprocess.CompletedProcess[str], Path]:
    build_env = env.copy()
    if target_root_override is not None:
        build_env["CARGO_TARGET_DIR"] = str(target_root_override)
        target_root = target_root_override
    else:
        target_root = _cargo_target_root(root)
    src = _wasm_runtime_artifact_path(target_root, profile_dir)
    try:
        src.unlink()
    except FileNotFoundError:
        pass
    except OSError:
        pass
    build = subprocess.run(
        cmd,
        cwd=root,
        env=build_env,
        timeout=cargo_timeout,
        check=False,
        text=True,
    )
    wrapper = build_env.get("RUSTC_WRAPPER", "")
    if build.returncode != 0 and wrapper and Path(wrapper).name == "sccache":
        retry_env = build_env.copy()
        retry_env.pop("RUSTC_WRAPPER", None)
        if not json_output:
            print(
                "Runtime wasm build: sccache wrapper failure detected; retrying without sccache.",
                file=sys.stderr,
            )
        build = subprocess.run(
            cmd,
            cwd=root,
            env=retry_env,
            timeout=cargo_timeout,
            check=False,
            text=True,
        )
    return build, src


def _ensure_runtime_wasm(
    runtime_wasm: Path,
    *,
    reloc: bool,
    json_output: bool,
    cargo_profile: str,
    cargo_timeout: float | None,
    project_root: Path | None = None,
) -> bool:
    root = project_root or Path(__file__).resolve().parents[2]
    requested_cargo_profile = cargo_profile
    cargo_profile = _resolve_wasm_cargo_profile(cargo_profile)
    env = os.environ.copy()
    use_legacy_wasm_flags = os.environ.get("MOLT_WASM_LEGACY_LINK_FLAGS") == "1"
    if use_legacy_wasm_flags:
        if reloc:
            flags = (
                "-C link-arg=--relocatable -C link-arg=--no-gc-sections"
                " -C relocation-model=pic"
            )
        else:
            flags = (
                "-C link-arg=--import-memory -C link-arg=--import-table"
                " -C link-arg=--growable-table"
            )
    else:
        flags = (
            "-C link-arg=--relocatable -C link-arg=--no-gc-sections"
            " -C relocation-model=pic"
            if reloc
            else ""
        )
    rustflags = env.get("RUSTFLAGS", "").strip()
    if flags:
        rustflags = f"{rustflags} {flags}".strip()
    # Enable WASM SIMD (128-bit) for vectorized string/bytes operations.
    # All modern WASM runtimes support simd128: Node.js >=16, wasmtime,
    # Chrome/Firefox/Safari since 2021. This dramatically speeds up
    # string search, hex encode, base64, and whitespace scanning.
    if "-C target-feature" not in rustflags:
        rustflags = f"{rustflags} -C target-feature=+simd128".strip()
    fingerprint_path = _runtime_fingerprint_path(
        root, runtime_wasm, cargo_profile, "wasm32-wasip1"
    )
    stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
    fingerprint = _runtime_fingerprint(
        root,
        cargo_profile=cargo_profile,
        target_triple="wasm32-wasip1",
        rustflags=rustflags,
        runtime_features=(),
        stored_fingerprint=stored_fingerprint,
    )
    lock_suffix = "reloc" if reloc else "shared"
    lock_name = f"runtime.{cargo_profile}.wasm32-wasip1.{lock_suffix}"
    with _build_lock(root, lock_name):
        if stored_fingerprint is None:
            stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
        needs_rebuild = _artifact_needs_rebuild(
            runtime_wasm, fingerprint, stored_fingerprint
        )
        if not needs_rebuild and _is_valid_wasm_binary(runtime_wasm):
            return True
        if not needs_rebuild and not json_output:
            print(
                "Runtime wasm artifact invalid/corrupt; forcing rebuild.",
                file=sys.stderr,
            )
        if not json_output:
            print("Runtime sources changed; rebuilding runtime...")
        if flags:
            _append_rustflags(env, flags)
        if os.environ.get("MOLT_WASM_FORCE_CC") == "1":
            _configure_wasm_cc_env(env)
        # Enable incremental compilation for dev-fast WASM builds; disable for
        # release (where LTO makes incremental irrelevant).
        if cargo_profile in ("dev", "dev-fast"):
            env.setdefault("CARGO_INCREMENTAL", "1")
        else:
            env["CARGO_INCREMENTAL"] = "0"
        # Enable sccache for WASM builds by default (same as native builds).
        # Set MOLT_WASM_DISABLE_SCCACHE=1 to opt out.
        if os.environ.get("MOLT_WASM_DISABLE_SCCACHE") != "1":
            _maybe_enable_sccache(env)
        else:
            env.pop("RUSTC_WRAPPER", None)
        profile_dir = _cargo_profile_dir(cargo_profile)
        cmd = [
            "cargo",
            "build",
            "--package",
            "molt-runtime",
            "--profile",
            cargo_profile,
            "--target",
            "wasm32-wasip1",
        ]
        try:
            build, src = _run_runtime_wasm_cargo_build(
                cmd=cmd,
                root=root,
                env=env,
                cargo_timeout=cargo_timeout,
                profile_dir=profile_dir,
                json_output=json_output,
            )
        except subprocess.TimeoutExpired:
            if not json_output:
                timeout_note = (
                    f"Runtime wasm build timed out after {cargo_timeout:.1f}s."
                    if cargo_timeout is not None
                    else "Runtime wasm build timed out."
                )
                print(timeout_note, file=sys.stderr)
            return False
        if build.returncode != 0:
            if not json_output:
                print("Runtime wasm build failed", file=sys.stderr)
            return False
        src_state = _inspect_wasm_binary(src)
        if src_state == "missing":
            if not json_output:
                print(
                    "Runtime wasm build succeeded but artifact is missing.",
                    file=sys.stderr,
                )
            return False
        if src_state != "valid":
            if not json_output:
                print(
                    f"Runtime wasm build produced invalid artifact: {src}; retrying with isolated target dir.",
                    file=sys.stderr,
                )
            recovery_target_root = _wasm_runtime_recovery_target_root(
                _cargo_target_root(root)
            )
            try:
                build, recovery_src = _run_runtime_wasm_cargo_build(
                    cmd=cmd,
                    root=root,
                    env=env,
                    cargo_timeout=cargo_timeout,
                    profile_dir=profile_dir,
                    target_root_override=recovery_target_root,
                    json_output=json_output,
                )
            except subprocess.TimeoutExpired:
                if not json_output:
                    timeout_note = (
                        f"Runtime wasm recovery build timed out after {cargo_timeout:.1f}s."
                        if cargo_timeout is not None
                        else "Runtime wasm recovery build timed out."
                    )
                    print(timeout_note, file=sys.stderr)
                return False
            if build.returncode != 0:
                if not json_output:
                    print("Runtime wasm recovery build failed", file=sys.stderr)
                return False
            recovery_state = _inspect_wasm_binary(recovery_src)
            if recovery_state == "missing":
                if not json_output:
                    print(
                        "Runtime wasm recovery build succeeded but artifact is missing.",
                        file=sys.stderr,
                    )
                return False
            if recovery_state != "valid":
                fallback_profile = os.environ.get(
                    "MOLT_WASM_RUNTIME_FALLBACK_PROFILE", "release-fast"
                ).strip()
                can_try_fallback_profile = (
                    requested_cargo_profile == "release"
                    and fallback_profile
                    and fallback_profile != cargo_profile
                    and _CARGO_PROFILE_NAME_RE.match(fallback_profile) is not None
                )
                if not can_try_fallback_profile:
                    if not json_output:
                        print(
                            f"Runtime wasm recovery build produced invalid artifact: {recovery_src}",
                            file=sys.stderr,
                        )
                    return False
                if not json_output:
                    print(
                        "Runtime wasm release profile produced invalid artifacts; "
                        f"retrying with fallback profile {fallback_profile}.",
                        file=sys.stderr,
                    )
                fallback_profile_dir = _cargo_profile_dir(fallback_profile)
                fallback_cmd = cmd.copy()
                fallback_cmd[5] = fallback_profile
                fallback_target_root = recovery_target_root.parent / (
                    f"{recovery_target_root.name}-{fallback_profile}"
                )
                try:
                    build, fallback_src = _run_runtime_wasm_cargo_build(
                        cmd=fallback_cmd,
                        root=root,
                        env=env,
                        cargo_timeout=cargo_timeout,
                        profile_dir=fallback_profile_dir,
                        target_root_override=fallback_target_root,
                        json_output=json_output,
                    )
                except subprocess.TimeoutExpired:
                    if not json_output:
                        timeout_note = (
                            f"Runtime wasm fallback build timed out after {cargo_timeout:.1f}s."
                            if cargo_timeout is not None
                            else "Runtime wasm fallback build timed out."
                        )
                        print(timeout_note, file=sys.stderr)
                    return False
                if build.returncode != 0:
                    if not json_output:
                        print("Runtime wasm fallback build failed", file=sys.stderr)
                    return False
                fallback_state = _inspect_wasm_binary(fallback_src)
                if fallback_state == "missing":
                    if not json_output:
                        print(
                            "Runtime wasm fallback build succeeded but artifact is missing.",
                            file=sys.stderr,
                        )
                    return False
                if fallback_state != "valid":
                    if not json_output:
                        print(
                            f"Runtime wasm fallback build produced invalid artifact: {fallback_src}",
                            file=sys.stderr,
                        )
                    return False
                src = fallback_src
            else:
                src = recovery_src
        runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, runtime_wasm)
        if _inspect_wasm_binary(runtime_wasm) != "valid":
            if not json_output:
                print(
                    f"Copied runtime wasm artifact is invalid: {runtime_wasm}",
                    file=sys.stderr,
                )
            return False
        if fingerprint is not None:
            try:
                fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
                _write_runtime_fingerprint(fingerprint_path, fingerprint)
            except OSError:
                if not json_output:
                    print(
                        "Warning: failed to write runtime fingerprint metadata.",
                        file=sys.stderr,
                    )
    return True


def _read_wasm_varuint(data: bytes, offset: int) -> tuple[int, int]:
    result = 0
    shift = 0
    while True:
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading varuint")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            return result, offset
        shift += 7
        if shift > 35:
            raise ValueError("varuint too large")


def _read_wasm_string(data: bytes, offset: int) -> tuple[str, int]:
    length, offset = _read_wasm_varuint(data, offset)
    end = offset + length
    if end > len(data):
        raise ValueError("Unexpected EOF while reading string")
    return data[offset:end].decode("utf-8"), end


def _read_wasm_varint(data: bytes, offset: int, bits: int) -> tuple[int, int]:
    result = 0
    shift = 0
    byte = 0
    while True:
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading varint")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        shift += 7
        if byte & 0x80 == 0:
            break
        if shift > bits + 7:
            raise ValueError("varint too large")
    if shift < bits and (byte & 0x40):
        result |= -1 << shift
    return result, offset


def _read_wasm_const_expr_i32(data: bytes, offset: int) -> tuple[int, int]:
    if offset >= len(data):
        raise ValueError("Unexpected EOF while reading const expr")
    opcode = data[offset]
    offset += 1
    if opcode == 0x41:  # i32.const
        value, offset = _read_wasm_varint(data, offset, 32)
    elif opcode == 0x42:  # i64.const
        value, offset = _read_wasm_varint(data, offset, 64)
    else:
        raise ValueError("Unsupported const expr opcode")
    if offset >= len(data) or data[offset] != 0x0B:
        raise ValueError("Invalid const expr terminator")
    offset += 1
    return value, offset


def _read_wasm_table_min(path: Path) -> int | None:
    try:
        data = path.read_bytes()
    except OSError:
        return None
    if len(data) < 8 or data[:4] != b"\0asm" or data[4:8] != b"\x01\x00\x00\x00":
        return None
    offset = 8
    try:
        while offset < len(data):
            section_id = data[offset]
            offset += 1
            size, offset = _read_wasm_varuint(data, offset)
            end = offset + size
            if end > len(data):
                raise ValueError("Unexpected EOF while reading section")
            if section_id != 2:
                offset = end
                continue
            payload = data[offset:end]
            offset = end
            cursor = 0
            count, cursor = _read_wasm_varuint(payload, cursor)
            for _ in range(count):
                module, cursor = _read_wasm_string(payload, cursor)
                name, cursor = _read_wasm_string(payload, cursor)
                if cursor >= len(payload):
                    raise ValueError("Unexpected EOF while reading import")
                kind = payload[cursor]
                cursor += 1
                if kind == 0:
                    _, cursor = _read_wasm_varuint(payload, cursor)
                elif kind == 1:
                    if cursor >= len(payload):
                        raise ValueError("Unexpected EOF while reading table type")
                    cursor += 1
                    flags, cursor = _read_wasm_varuint(payload, cursor)
                    minimum, cursor = _read_wasm_varuint(payload, cursor)
                    if flags & 0x1:
                        _, cursor = _read_wasm_varuint(payload, cursor)
                    if module == "env" and name == "__indirect_function_table":
                        return minimum
                elif kind == 2:
                    flags, cursor = _read_wasm_varuint(payload, cursor)
                    _, cursor = _read_wasm_varuint(payload, cursor)
                    if flags & 0x1:
                        _, cursor = _read_wasm_varuint(payload, cursor)
                elif kind == 3:
                    if cursor + 2 > len(payload):
                        raise ValueError("Unexpected EOF while reading global type")
                    cursor += 2
                else:
                    raise ValueError("Unknown import kind")
    except ValueError:
        return None
    return None


def _read_wasm_data_end(path: Path) -> int | None:
    try:
        data = path.read_bytes()
    except OSError:
        return None
    if len(data) < 8 or data[:4] != b"\0asm" or data[4:8] != b"\x01\x00\x00\x00":
        return None
    offset = 8
    max_end = None
    try:
        while offset < len(data):
            section_id = data[offset]
            offset += 1
            size, offset = _read_wasm_varuint(data, offset)
            end = offset + size
            if end > len(data):
                raise ValueError("Unexpected EOF while reading section")
            if section_id != 11:
                offset = end
                continue
            payload = data[offset:end]
            offset = end
            cursor = 0
            count, cursor = _read_wasm_varuint(payload, cursor)
            for _ in range(count):
                if cursor >= len(payload):
                    raise ValueError("Unexpected EOF while reading data segment")
                flags = payload[cursor]
                cursor += 1
                is_passive = flags & 0x1
                has_memidx = flags & 0x2
                if has_memidx:
                    _, cursor = _read_wasm_varuint(payload, cursor)
                if is_passive:
                    size_bytes, cursor = _read_wasm_varuint(payload, cursor)
                    cursor += size_bytes
                    continue
                offset_val, cursor = _read_wasm_const_expr_i32(payload, cursor)
                size_bytes, cursor = _read_wasm_varuint(payload, cursor)
                cursor += size_bytes
                if offset_val < 0:
                    continue
                end_val = offset_val + size_bytes
                if max_end is None or end_val > max_end:
                    max_end = end_val
    except ValueError:
        return None
    return max_end


def _read_wasm_memory_min_bytes(path: Path) -> int | None:
    try:
        data = path.read_bytes()
    except OSError:
        return None
    if len(data) < 8 or data[:4] != b"\0asm" or data[4:8] != b"\x01\x00\x00\x00":
        return None
    offset = 8
    memory_pages: int | None = None
    try:
        while offset < len(data):
            section_id = data[offset]
            offset += 1
            size, offset = _read_wasm_varuint(data, offset)
            end = offset + size
            if end > len(data):
                raise ValueError("Unexpected EOF while reading section")
            payload = data[offset:end]
            offset = end
            cursor = 0
            if section_id == 2:  # import section
                count, cursor = _read_wasm_varuint(payload, cursor)
                for _ in range(count):
                    module, cursor = _read_wasm_string(payload, cursor)
                    name, cursor = _read_wasm_string(payload, cursor)
                    if cursor >= len(payload):
                        raise ValueError("Unexpected EOF while reading import")
                    kind = payload[cursor]
                    cursor += 1
                    if kind == 0:
                        _, cursor = _read_wasm_varuint(payload, cursor)
                    elif kind == 1:
                        if cursor >= len(payload):
                            raise ValueError("Unexpected EOF while reading table type")
                        cursor += 1
                        flags, cursor = _read_wasm_varuint(payload, cursor)
                        _, cursor = _read_wasm_varuint(payload, cursor)
                        if flags & 0x1:
                            _, cursor = _read_wasm_varuint(payload, cursor)
                    elif kind == 2:
                        flags, cursor = _read_wasm_varuint(payload, cursor)
                        minimum, cursor = _read_wasm_varuint(payload, cursor)
                        if flags & 0x1:
                            _, cursor = _read_wasm_varuint(payload, cursor)
                        if module == "env" and name == "memory":
                            memory_pages = max(memory_pages or 0, minimum)
                    elif kind == 3:
                        if cursor + 2 > len(payload):
                            raise ValueError("Unexpected EOF while reading global type")
                        cursor += 2
                    else:
                        raise ValueError("Unknown import kind")
            elif section_id == 5:  # memory section
                count, cursor = _read_wasm_varuint(payload, cursor)
                for _ in range(count):
                    flags, cursor = _read_wasm_varuint(payload, cursor)
                    minimum, cursor = _read_wasm_varuint(payload, cursor)
                    if flags & 0x1:
                        _, cursor = _read_wasm_varuint(payload, cursor)
                    memory_pages = max(memory_pages or 0, minimum)
    except ValueError:
        return None
    if memory_pages is None:
        return None
    return memory_pages * 65536


@functools.lru_cache(maxsize=64)
def _cargo_profile_dir(cargo_profile: str) -> str:
    return "debug" if cargo_profile == "dev" else cargo_profile


@functools.lru_cache(maxsize=256)
def _resolve_env_path_cached(
    value: str | None,
    default_str: str,
    cwd_str: str,
) -> Path:
    if not value:
        return Path(default_str)
    path = Path(value).expanduser()
    if not path.is_absolute():
        path = (Path(cwd_str) / path).absolute()
    return path


def _resolve_env_path(var: str, default: Path) -> Path:
    return _resolve_env_path_cached(
        os.environ.get(var),
        os.fspath(default),
        os.fspath(Path.cwd()),
    )


@functools.lru_cache(maxsize=512)
def _safe_output_base(name: str) -> str:
    cleaned = _OUTPUT_BASE_SAFE_RE.sub("_", name)
    return cleaned or "molt"


@functools.lru_cache(maxsize=128)
def _default_molt_cache_cached(
    cache_override: str | None,
    xdg_cache_home: str | None,
    cwd_str: str,
    home_str: str,
    platform_name: str,
) -> Path:
    if cache_override:
        path = Path(cache_override).expanduser()
        if not path.is_absolute():
            path = (Path(cwd_str) / path).absolute()
        return path
    if platform_name == "darwin":
        base = Path(home_str) / "Library" / "Caches"
    else:
        if xdg_cache_home:
            base = Path(xdg_cache_home).expanduser()
            if not base.is_absolute():
                base = (Path(cwd_str) / base).absolute()
        else:
            base = Path(home_str) / ".cache"
    return base / "molt"


def _default_molt_cache() -> Path:
    return _default_molt_cache_cached(
        os.environ.get("MOLT_CACHE"),
        os.environ.get("XDG_CACHE_HOME"),
        os.fspath(Path.cwd()),
        os.fspath(Path.home()),
        sys.platform,
    )


@functools.lru_cache(maxsize=128)
def _default_molt_home_cached(
    home_override: str | None,
    cache_override: str | None,
    xdg_cache_home: str | None,
    cwd_str: str,
    home_str: str,
    platform_name: str,
) -> Path:
    if home_override:
        path = Path(home_override).expanduser()
        if not path.is_absolute():
            path = (Path(cwd_str) / path).absolute()
        return path
    return (
        _default_molt_cache_cached(
            cache_override,
            xdg_cache_home,
            cwd_str,
            home_str,
            platform_name,
        )
        / "home"
    )


def _default_molt_home() -> Path:
    return _default_molt_home_cached(
        os.environ.get("MOLT_HOME"),
        os.environ.get("MOLT_CACHE"),
        os.environ.get("XDG_CACHE_HOME"),
        os.fspath(Path.cwd()),
        os.fspath(Path.home()),
        sys.platform,
    )


@functools.lru_cache(maxsize=128)
def _default_molt_bin_cached(
    bin_override: str | None,
    home_override: str | None,
    cache_override: str | None,
    xdg_cache_home: str | None,
    cwd_str: str,
    home_str: str,
    platform_name: str,
) -> Path:
    if bin_override:
        path = Path(bin_override).expanduser()
        if not path.is_absolute():
            path = (Path(cwd_str) / path).absolute()
        return path
    return (
        _default_molt_home_cached(
            home_override,
            cache_override,
            xdg_cache_home,
            cwd_str,
            home_str,
            platform_name,
        )
        / "bin"
    )


def _default_molt_bin() -> Path:
    return _default_molt_bin_cached(
        os.environ.get("MOLT_BIN"),
        os.environ.get("MOLT_HOME"),
        os.environ.get("MOLT_CACHE"),
        os.environ.get("XDG_CACHE_HOME"),
        os.fspath(Path.cwd()),
        os.fspath(Path.home()),
        sys.platform,
    )


@functools.lru_cache(maxsize=256)
def _cargo_target_root_cached(
    project_root_str: str,
    cargo_target_override: str | None,
    cwd_str: str,
) -> Path:
    project_root = Path(project_root_str)
    if not cargo_target_override:
        return project_root / "target"
    path = Path(cargo_target_override).expanduser()
    if not path.is_absolute():
        path = (Path(cwd_str) / path).absolute()
    return path


def _cargo_target_root(project_root: Path) -> Path:
    return _cargo_target_root_cached(
        os.fspath(project_root),
        os.environ.get("CARGO_TARGET_DIR"),
        os.fspath(Path.cwd()),
    )


@functools.lru_cache(maxsize=256)
def _build_state_root_cached(
    project_root_str: str,
    build_state_override: str | None,
    cargo_target_override: str | None,
    cwd_str: str,
) -> Path:
    project_root = Path(project_root_str)
    if build_state_override:
        path = Path(build_state_override).expanduser()
        if not path.is_absolute():
            path = (project_root / path).absolute()
        return path
    return (
        _cargo_target_root_cached(
            project_root_str,
            cargo_target_override,
            cwd_str,
        )
        / ".molt_state"
    )


def _build_state_root(project_root: Path) -> Path:
    return _build_state_root_cached(
        os.fspath(project_root),
        os.environ.get("MOLT_BUILD_STATE_DIR"),
        os.environ.get("CARGO_TARGET_DIR"),
        os.fspath(Path.cwd()),
    )


@functools.lru_cache(maxsize=128)
def _wasm_runtime_root_cached(
    project_root_str: str,
    env_root: str | None,
    ext_root: str | None,
    cwd_str: str,
) -> Path:
    if env_root:
        return Path(env_root).expanduser()
    project_root = Path(project_root_str)
    external_root = Path(ext_root).expanduser() if ext_root else Path(cwd_str)
    if external_root.is_dir():
        return external_root / "wasm"
    return project_root / "wasm"


def _wasm_runtime_root(project_root: Path) -> Path:
    return _wasm_runtime_root_cached(
        os.fspath(project_root),
        os.environ.get("MOLT_WASM_RUNTIME_DIR"),
        os.environ.get("MOLT_EXT_ROOT"),
        os.fspath(Path.cwd()),
    )


@functools.lru_cache(maxsize=256)
def _default_build_root_cached(
    output_base: str,
    home_override: str | None,
    cache_override: str | None,
    xdg_cache_home: str | None,
    cwd_str: str,
    home_str: str,
    platform_name: str,
) -> Path:
    safe_base = _safe_output_base(output_base)
    home_root = _default_molt_home_cached(
        home_override,
        cache_override,
        xdg_cache_home,
        cwd_str,
        home_str,
        platform_name,
    )
    return home_root / "build" / safe_base


def _default_build_root(output_base: str) -> Path:
    return _default_build_root_cached(
        output_base,
        os.environ.get("MOLT_HOME"),
        os.environ.get("MOLT_CACHE"),
        os.environ.get("XDG_CACHE_HOME"),
        os.fspath(Path.cwd()),
        os.fspath(Path.home()),
        sys.platform,
    )


@functools.lru_cache(maxsize=256)
def _resolve_cache_root_cached(
    project_root_str: str,
    cache_dir: str | None,
    cache_override: str | None,
    xdg_cache_home: str | None,
    cwd_str: str,
    home_str: str,
    platform_name: str,
) -> Path:
    if not cache_dir:
        return _default_molt_cache_cached(
            cache_override,
            xdg_cache_home,
            cwd_str,
            home_str,
            platform_name,
        )
    project_root = Path(project_root_str)
    path = Path(cache_dir).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    return path


def _resolve_cache_root(project_root: Path, cache_dir: str | None) -> Path:
    return _resolve_cache_root_cached(
        os.fspath(project_root),
        cache_dir,
        os.environ.get("MOLT_CACHE"),
        os.environ.get("XDG_CACHE_HOME"),
        os.fspath(Path.cwd()),
        os.fspath(Path.home()),
        sys.platform,
    )


@functools.lru_cache(maxsize=256)
def _resolve_out_dir_cached(
    project_root_str: str,
    out_dir: str | None,
) -> Path | None:
    if not out_dir:
        return None
    project_root = Path(project_root_str)
    path = Path(out_dir).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    return path


def _resolve_out_dir(project_root: Path, out_dir: str | Path | None) -> Path | None:
    if not out_dir:
        return None
    path = _resolve_out_dir_cached(os.fspath(project_root), os.fspath(out_dir))
    assert path is not None
    path.mkdir(parents=True, exist_ok=True)
    return path


@functools.lru_cache(maxsize=256)
def _resolve_sysroot_cached(
    project_root_str: str,
    sysroot: str | None,
    env_sysroot: str | None,
    env_cross_sysroot: str | None,
) -> Path | None:
    raw = sysroot or env_sysroot or env_cross_sysroot
    if not raw:
        return None
    project_root = Path(project_root_str)
    path = Path(raw).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    return path


def _resolve_sysroot(project_root: Path, sysroot: str | None) -> Path | None:
    return _resolve_sysroot_cached(
        os.fspath(project_root),
        sysroot,
        os.environ.get("MOLT_SYSROOT"),
        os.environ.get("MOLT_CROSS_SYSROOT"),
    )


def _pgo_hotspot_entries(
    hotspots: Any, warnings: list[str]
) -> list[tuple[str, float | None]]:
    entries: list[tuple[str, float | None]] = []
    if hotspots is None:
        return entries
    if isinstance(hotspots, dict):
        for name, score in hotspots.items():
            if not isinstance(name, str) or not name:
                continue
            score_val = score if isinstance(score, (int, float)) else None
            entries.append((name, float(score_val) if score_val is not None else None))
        return entries
    if isinstance(hotspots, list):
        for entry in hotspots:
            if isinstance(entry, str) and entry:
                entries.append((entry, None))
                continue
            if isinstance(entry, (list, tuple)) and entry:
                name = entry[0]
                score = entry[1] if len(entry) > 1 else None
                if isinstance(name, str) and name:
                    score_val = score if isinstance(score, (int, float)) else None
                    entries.append(
                        (name, float(score_val) if score_val is not None else None)
                    )
                continue
            if isinstance(entry, dict):
                name = (
                    entry.get("symbol")
                    or entry.get("name")
                    or entry.get("func")
                    or entry.get("function")
                )
                if not isinstance(name, str) or not name:
                    continue
                score = entry.get("score")
                if score is None:
                    score = entry.get("time_ms")
                if score is None:
                    score = entry.get("time_us")
                if score is None:
                    score = entry.get("count")
                score_val = score if isinstance(score, (int, float)) else None
                entries.append(
                    (name, float(score_val) if score_val is not None else None)
                )
                continue
        return entries
    warnings.append("PGO profile hotspots must be a list or object; ignoring.")
    return entries


def _extract_hot_functions(profile: dict[str, Any], warnings: list[str]) -> list[str]:
    entries = _pgo_hotspot_entries(profile.get("hotspots"), warnings)
    if not entries:
        return []
    has_score = any(score is not None for _, score in entries)
    if has_score:
        entries = sorted(
            entries,
            key=lambda item: (-(item[1] or 0.0), item[0]),
        )
    else:
        entries = sorted(entries, key=lambda item: item[0])
    seen: set[str] = set()
    hot: list[str] = []
    for name, _score in entries:
        if name in seen:
            continue
        seen.add(name)
        hot.append(name)
    return hot


def _extract_runtime_feedback_hot_functions(
    payload: dict[str, Any], warnings: list[str]
) -> list[str]:
    raw = payload.get("hot_functions")
    if raw is None:
        return []
    entries = _pgo_hotspot_entries(raw, warnings)
    if not entries:
        return []
    has_score = any(score is not None for _, score in entries)
    if has_score:
        entries = sorted(entries, key=lambda item: (-(item[1] or 0.0), item[0]))
    else:
        entries = sorted(entries, key=lambda item: item[0])
    seen: set[str] = set()
    hot: list[str] = []
    for name, _score in entries:
        if name in seen:
            continue
        seen.add(name)
        hot.append(name)
    return hot


def _load_pgo_profile(
    project_root: Path,
    profile_path: str,
    warnings: list[str],
    json_output: bool,
    command: str,
) -> tuple[PgoProfileSummary | None, Path | None, int | None]:
    path = Path(profile_path).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    if not path.exists():
        return (
            None,
            None,
            _fail(f"PGO profile not found: {path}", json_output, command=command),
        )
    try:
        raw = path.read_bytes()
    except OSError as exc:
        return (
            None,
            None,
            _fail(
                f"Failed to read PGO profile {path}: {exc}",
                json_output,
                command=command,
            ),
        )
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as exc:
        return (
            None,
            None,
            _fail(
                f"Invalid PGO profile JSON at {path}:{exc.lineno}:{exc.colno}: {exc.msg}",
                json_output,
                command=command,
            ),
        )
    if not isinstance(payload, dict):
        return (
            None,
            None,
            _fail(
                f"Invalid PGO profile {path}: expected a JSON object.",
                json_output,
                command=command,
            ),
        )
    errors: list[str] = []
    version = payload.get("molt_profile_version")
    if not isinstance(version, str):
        errors.append("missing molt_profile_version")
    elif version != "0.1":
        errors.append(f"unsupported molt_profile_version {version}")
    python_impl = payload.get("python_implementation")
    if not isinstance(python_impl, str) or not python_impl:
        errors.append("missing python_implementation")
    python_version = payload.get("python_version")
    if not isinstance(python_version, str) or not python_version:
        errors.append("missing python_version")
    platform_meta = payload.get("platform")
    if not isinstance(platform_meta, dict):
        errors.append("missing platform")
    else:
        if not isinstance(platform_meta.get("os"), str):
            errors.append("platform.os must be a string")
        if not isinstance(platform_meta.get("arch"), str):
            errors.append("platform.arch must be a string")
    run_meta = payload.get("run_metadata")
    if not isinstance(run_meta, dict):
        errors.append("missing run_metadata")
    else:
        if not isinstance(run_meta.get("entrypoint"), str):
            errors.append("run_metadata.entrypoint must be a string")
        argv = run_meta.get("argv")
        if not isinstance(argv, list) or not all(isinstance(arg, str) for arg in argv):
            errors.append("run_metadata.argv must be a list of strings")
        if not isinstance(run_meta.get("env_fingerprint"), str):
            errors.append("run_metadata.env_fingerprint must be a string")
        if not isinstance(run_meta.get("inputs_fingerprint"), str):
            errors.append("run_metadata.inputs_fingerprint must be a string")
        duration_ms = run_meta.get("duration_ms")
        if not isinstance(duration_ms, (int, float)) or duration_ms < 0:
            errors.append("run_metadata.duration_ms must be a non-negative number")
    if errors:
        return (
            None,
            None,
            _fail(
                f"Invalid PGO profile {path}: " + "; ".join(errors),
                json_output,
                command=command,
            ),
        )
    hot_functions = _extract_hot_functions(payload, warnings)
    digest = hashlib.sha256(raw).hexdigest()
    summary = PgoProfileSummary(
        version=version, hash=digest, hot_functions=hot_functions
    )
    return summary, path, None


def _load_runtime_feedback(
    project_root: Path,
    feedback_path: str,
    warnings: list[str],
    json_output: bool,
    command: str,
) -> tuple[RuntimeFeedbackSummary | None, Path | None, int | None]:
    path = Path(feedback_path).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    if not path.exists():
        return (
            None,
            None,
            _fail(
                f"Runtime feedback artifact not found: {path}",
                json_output,
                command=command,
            ),
        )
    try:
        raw = path.read_bytes()
    except OSError as exc:
        return (
            None,
            None,
            _fail(
                f"Failed to read runtime feedback artifact {path}: {exc}",
                json_output,
                command=command,
            ),
        )
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as exc:
        return (
            None,
            None,
            _fail(
                "Invalid runtime feedback JSON at "
                f"{path}:{exc.lineno}:{exc.colno}: {exc.msg}",
                json_output,
                command=command,
            ),
        )
    if not isinstance(payload, dict):
        return (
            None,
            None,
            _fail(
                f"Invalid runtime feedback artifact {path}: expected a JSON object.",
                json_output,
                command=command,
            ),
        )
    errors: list[str] = []
    schema_version = payload.get("schema_version")
    if not isinstance(schema_version, int):
        errors.append("missing schema_version")
    if payload.get("kind") != "runtime_feedback":
        errors.append(f"unexpected kind {payload.get('kind')!r}")
    if not isinstance(payload.get("profile"), dict):
        errors.append("missing profile")
    if not isinstance(payload.get("hot_paths"), dict):
        errors.append("missing hot_paths")
    if not isinstance(payload.get("deopt_reasons"), dict):
        errors.append("missing deopt_reasons")
    if errors:
        return (
            None,
            None,
            _fail(
                f"Invalid runtime feedback artifact {path}: " + "; ".join(errors),
                json_output,
                command=command,
            ),
        )
    hot_functions = _extract_runtime_feedback_hot_functions(payload, warnings)
    digest = hashlib.sha256(raw).hexdigest()
    summary = RuntimeFeedbackSummary(
        schema_version=schema_version,
        hash=digest,
        hot_functions=hot_functions,
    )
    return summary, path, None


def _resolve_timeout_env(env_name: str) -> tuple[float | None, str | None]:
    raw = os.environ.get(env_name)
    if raw is None:
        return None, None
    try:
        timeout = float(raw)
    except ValueError:
        return None, f"Invalid {env_name} value: {raw}"
    if timeout <= 0:
        return None, f"{env_name} must be greater than zero."
    return timeout, None


@contextmanager
def _phase_timeout(timeout_s: float | None, *, phase_name: str):
    if timeout_s is None:
        yield
        return
    if os.name != "posix" or threading.current_thread() is not threading.main_thread():
        yield
        return
    if not hasattr(signal, "setitimer") or not hasattr(signal, "ITIMER_REAL"):
        yield
        return
    previous_handler = signal.getsignal(signal.SIGALRM)
    previous_timer = signal.getitimer(signal.ITIMER_REAL)

    def _timeout_handler(_signum: int, _frame: Any) -> None:
        raise TimeoutError(
            f"{phase_name} timed out after {timeout_s:.1f}s "
            "(MOLT_FRONTEND_PHASE_TIMEOUT)"
        )

    signal.signal(signal.SIGALRM, _timeout_handler)
    signal.setitimer(signal.ITIMER_REAL, timeout_s)
    try:
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0.0, 0.0)
        signal.signal(signal.SIGALRM, previous_handler)
        if previous_timer[0] > 0 or previous_timer[1] > 0:
            signal.setitimer(signal.ITIMER_REAL, previous_timer[0], previous_timer[1])


def _resolve_dev_linker() -> str | None:
    raw = os.environ.get("MOLT_DEV_LINKER", "auto").strip().lower()
    if raw in {"0", "false", "no", "off", "none", "disable"}:
        return None
    if raw in {"mold", "lld"}:
        return raw
    if raw != "auto":
        return None
    if shutil.which("mold"):
        return "mold"
    if shutil.which("ld.lld") or shutil.which("lld"):
        return "lld"
    return None


def _darwin_binary_imports_validation_error(binary_path: Path) -> str | None:
    if sys.platform != "darwin":
        return None
    dyld_info = shutil.which("dyld_info")
    if dyld_info is None or not binary_path.exists():
        return None
    try:
        proc = subprocess.run(
            [dyld_info, str(binary_path)],
            capture_output=True,
            text=True,
            timeout=10.0,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    combined = "\n".join(
        part.strip() for part in (proc.stdout, proc.stderr) if part and part.strip()
    )
    needle = combined.lower()
    if "unknown imports_format" in needle or "unknown imports format" in needle:
        return combined or "dyld_info reported unknown imports format."
    return None


def _darwin_binary_magic_error(binary_path: Path) -> str | None:
    """Return an error string when a purported Mach-O binary is obviously invalid.

    This is a hard correctness check: we should not claim a successful build when the
    linker returned 0 but produced a non-Mach-O output file (observed as all-zero data
    artifacts under some linker/toolchain configurations).
    """

    if sys.platform != "darwin":
        return None
    try:
        header = binary_path.read_bytes()[:4]
    except OSError as exc:
        return f"Failed to read output binary: {exc}"
    if len(header) < 4:
        return "Output binary is truncated (missing Mach-O header)."
    magic = int.from_bytes(header, "big", signed=False)
    # Accept thin and fat Mach-O headers (32/64-bit). We only need to reject
    # obviously-invalid outputs (e.g. all-zero placeholders).
    if magic in {
        0xFEEDFACE,
        0xFEEDFACF,
        0xCEFAEDFE,
        0xCFFAEDFE,
        0xCAFEBABE,
        0xBEBAFECA,
    }:
        return None
    return f"Output binary is not Mach-O (header=0x{magic:08x})."


def _resolve_output_roots(
    project_root: Path, out_dir: Path | None, output_base: str
) -> tuple[Path, Path, Path]:
    if out_dir is not None:
        # Keep `--out-dir` builds self-contained so ephemeral/benchmark runs do
        # not depend on global MOLT_HOME state.
        artifacts_root = out_dir / ".molt_build" / _safe_output_base(output_base)
        bin_root = out_dir
        output_root = out_dir
    else:
        artifacts_root = _default_build_root(output_base)
        bin_root = _default_molt_bin()
        output_root = project_root

    def _repair_broken_symlink_parents(path: Path) -> bool:
        repaired = False
        chain = list(path.parents)
        chain.reverse()
        for parent in chain:
            try:
                if parent.is_symlink() and not parent.exists():
                    parent.unlink()
                    parent.mkdir(parents=True, exist_ok=True)
                    repaired = True
            except OSError:
                continue
        return repaired

    def _mkdir_resilient(path: Path) -> None:
        try:
            path.mkdir(parents=True, exist_ok=True)
        except (FileExistsError, NotADirectoryError):
            if _repair_broken_symlink_parents(path):
                path.mkdir(parents=True, exist_ok=True)
            else:
                raise

    _mkdir_resilient(artifacts_root)
    _mkdir_resilient(bin_root)
    if output_root != bin_root:
        _mkdir_resilient(output_root)
    return artifacts_root, bin_root, output_root


def _resolve_output_path(
    output: str | None,
    default: Path,
    *,
    out_dir: Path | None,
    project_root: Path,
) -> Path:
    if not output:
        return default
    path = Path(output).expanduser()
    if not path.is_absolute():
        base = out_dir if out_dir is not None else project_root
        path = base / path
    if output.endswith(os.sep) or (os.altsep and output.endswith(os.altsep)):
        return path / default.name
    try:
        if path.exists() and path.is_dir():
            return path / default.name
    except OSError:
        pass
    return path


_CACHE_FINGERPRINT: str | None = None
_CACHE_TOOLING_FINGERPRINT: str | None = None
_CACHE_KEY_SCHEMA_VERSION = "v3"
_FUNCTION_CACHE_KEY_SCHEMA_VERSION = "func-v2"


def _cache_fingerprint() -> str:
    global _CACHE_FINGERPRINT
    if _CACHE_FINGERPRINT is not None:
        return _CACHE_FINGERPRINT
    root = Path(__file__).resolve().parents[2]
    hasher = hashlib.sha256()
    rustc_info = _rustc_version() or ""
    rustflags = os.environ.get("RUSTFLAGS", "")
    hasher.update(f"rustc:{rustc_info}\n".encode("utf-8"))
    hasher.update(f"rustflags:{rustflags}\n".encode("utf-8"))
    seen: set[Path] = set()
    # Keep cache invalidation scoped to runtime/backend codegen sources.
    # Frontend/stdlib semantics already flow into the IR payload hash, so
    # hashing the entire stdlib tree here would over-invalidate unrelated builds.
    source_paths = _backend_source_paths(root) + _runtime_source_paths(root)
    for path in sorted(source_paths, key=lambda p: str(p)):
        if path in seen:
            continue
        seen.add(path)
        if path.is_dir():
            for item in sorted(path.rglob("*"), key=lambda p: str(p)):
                if item.is_file():
                    _hash_runtime_file(item, root, hasher)
        elif path.exists():
            _hash_runtime_file(path, root, hasher)
    _CACHE_FINGERPRINT = hasher.hexdigest()
    return _CACHE_FINGERPRINT


def _cache_tooling_fingerprint() -> str:
    global _CACHE_TOOLING_FINGERPRINT
    if _CACHE_TOOLING_FINGERPRINT is not None:
        return _CACHE_TOOLING_FINGERPRINT
    root = Path(__file__).resolve().parents[2]
    hasher = hashlib.sha256()
    tooling_paths = [
        Path(__file__).resolve(),
        root / "src/molt/frontend/__init__.py",
        root / "src/molt/cli.py",
    ]
    seen: set[Path] = set()
    for path in tooling_paths:
        if path in seen:
            continue
        seen.add(path)
        if path.exists():
            _hash_runtime_file(path, root, hasher)
    _CACHE_TOOLING_FINGERPRINT = hasher.hexdigest()
    return _CACHE_TOOLING_FINGERPRINT


def _json_ir_default(value: Any) -> Any:
    if isinstance(value, complex):
        return {"__complex__": [value.real, value.imag]}
    if value is Ellipsis:
        return {"__ellipsis__": True}
    if isinstance(value, tuple):
        return {
            "__tuple__": [
                _json_ir_default(item)
                if not isinstance(item, (str, int, float, bool, type(None), list, dict))
                else item
                for item in value
            ]
        }
    if isinstance(value, ast.AST):
        return {"__ast__": ast.dump(value, include_attributes=False)}
    if isinstance(value, (set, frozenset)):
        try:
            items = sorted(value)
        except TypeError:
            items = sorted((repr(item) for item in value))
        return {"__set__": items}
    if isinstance(value, MoltValue):
        return {
            "__molt_value__": {
                "name": value.name,
                "type_hint": value.type_hint,
            }
        }
    raise TypeError(f"Object of type {type(value).__name__} is not JSON serializable")


def _cache_ir_payload(ir: dict[str, Any]) -> bytes:
    funcs = ir.get("functions")
    normalized: dict[str, Any] = dict(ir)
    if isinstance(funcs, list):
        normalized["functions"] = _sorted_ir_functions(funcs)
    return json.dumps(
        normalized, sort_keys=True, separators=(",", ":"), default=_json_ir_default
    ).encode("utf-8")


def _sorted_ir_functions(functions: list[Any]) -> list[Any]:
    def _func_sort_key(entry: Any) -> str:
        if isinstance(entry, dict):
            name = entry.get("name")
            if isinstance(name, str):
                return name
        return ""

    return sorted(functions, key=_func_sort_key)


def _cache_payloads_for_ir(ir: dict[str, Any]) -> tuple[bytes, bytes]:
    functions = ir.get("functions")
    sorted_funcs: list[Any] = []
    if isinstance(functions, list):
        sorted_funcs = _sorted_ir_functions(functions)

    module_payload_ir: dict[str, Any] = dict(ir)
    module_payload_ir["functions"] = sorted_funcs
    module_payload = json.dumps(
        module_payload_ir,
        sort_keys=True,
        separators=(",", ":"),
        default=_json_ir_default,
    ).encode("utf-8")

    backend_payload_ir: dict[str, Any] = {
        "functions": sorted_funcs,
        "profile": ir.get("profile"),
        "top_level_extras_digest": _ir_top_level_extras_digest(ir),
    }
    backend_payload = json.dumps(
        backend_payload_ir,
        sort_keys=True,
        separators=(",", ":"),
        default=_json_ir_default,
    ).encode("utf-8")
    return module_payload, backend_payload


def _cache_key(
    ir: dict[str, Any],
    target: str,
    target_triple: str | None,
    variant: str = "",
    payload: bytes | None = None,
) -> str:
    if payload is None:
        payload = _cache_ir_payload(ir)
    suffix = target_triple or target
    if variant:
        suffix = f"{suffix}:{variant}"
    fingerprint = _cache_fingerprint().encode("utf-8")
    tooling_fingerprint = _cache_tooling_fingerprint().encode("utf-8")
    digest = hashlib.sha256(
        payload
        + b"|"
        + suffix.encode("utf-8")
        + b"|"
        + fingerprint
        + b"|"
        + tooling_fingerprint
        + b"|"
        + _CACHE_KEY_SCHEMA_VERSION.encode("utf-8")
    ).hexdigest()
    return digest


def _ir_top_level_extras_digest(ir: dict[str, Any]) -> str:
    extras = {
        key: value for key, value in ir.items() if key not in {"functions", "profile"}
    }
    encoded = json.dumps(
        extras, sort_keys=True, separators=(",", ":"), default=_json_ir_default
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _cache_backend_ir_payload(ir: dict[str, Any]) -> bytes:
    _, backend_payload = _cache_payloads_for_ir(ir)
    return backend_payload


def _backend_ir_text(ir: dict[str, Any]) -> str:
    return json.dumps(ir, separators=(",", ":"), default=_json_ir_default)


def _backend_ir_bytes(ir: dict[str, Any]) -> bytes:
    return _backend_ir_text(ir).encode("utf-8")


def _subprocess_output_text(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value


def _function_cache_key(
    ir: dict[str, Any],
    target: str,
    target_triple: str | None,
    variant: str = "",
    payload: bytes | None = None,
) -> str:
    if payload is None:
        payload = _cache_backend_ir_payload(ir)
    suffix = target_triple or target
    if variant:
        suffix = f"{suffix}:{variant}"
    fingerprint = _cache_fingerprint().encode("utf-8")
    tooling_fingerprint = _cache_tooling_fingerprint().encode("utf-8")
    return hashlib.sha256(
        payload
        + b"|"
        + suffix.encode("utf-8")
        + b"|"
        + fingerprint
        + b"|"
        + tooling_fingerprint
        + b"|"
        + _FUNCTION_CACHE_KEY_SCHEMA_VERSION.encode("utf-8")
    ).hexdigest()


def _ensure_rustup_target(target_triple: str, warnings: list[str]) -> bool:
    rustup_path = shutil.which("rustup")
    if not rustup_path:
        warnings.append(f"rustup not found; cannot ensure target {target_triple}")
        return False
    try:
        result = subprocess.run(
            ["rustup", "target", "list", "--installed"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        warnings.append(f"Failed to query rustup targets: {exc}")
        return False
    installed = result.stdout.split()
    if target_triple in installed:
        return True
    try:
        add = subprocess.run(
            ["rustup", "target", "add", target_triple],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        warnings.append(f"Failed to add rustup target {target_triple}: {exc}")
        return False
    if add.returncode != 0:
        detail = (add.stderr or add.stdout).strip() or "unknown error"
        warnings.append(f"rustup target add failed for {target_triple}: {detail}")
        return False
    return True


def _strip_arch_flags(args: list[str]) -> list[str]:
    cleaned: list[str] = []
    skip_next = False
    for arg in args:
        if skip_next:
            skip_next = False
            continue
        if arg == "-arch":
            skip_next = True
            continue
        if arg.startswith("-arch="):
            continue
        cleaned.append(arg)
    return cleaned


def _zig_target_query(target_triple: str) -> str:
    triple = target_triple.strip()
    if not triple:
        return target_triple
    parts = [part for part in triple.split("-") if part]
    if len(parts) < 2:
        return target_triple

    arch_aliases = {
        "amd64": "x86_64",
        "x64": "x86_64",
        "arm64": "aarch64",
        "armv7l": "armv7",
        "i386": "x86",
        "i486": "x86",
        "i586": "x86",
        "i686": "x86",
    }
    os_aliases = {
        "darwin": "macos",
        "macosx": "macos",
        "win32": "windows",
        "mingw32": "windows",
        "mingw64": "windows",
        "cygwin": "windows",
    }
    abi_aliases = {
        "sim": "simulator",
        "androideabi": "android",
    }
    abi_tokens = {
        "gnu",
        "gnueabi",
        "gnueabihf",
        "gnuabi64",
        "gnux32",
        "musl",
        "musleabi",
        "musleabihf",
        "msvc",
        "eabi",
        "eabihf",
        "android",
        "simulator",
        "sim",
        "ilp32",
        "uclibc",
        "ohos",
        "macabi",
        "androideabi",
    }
    os_tokens = {
        "linux",
        "windows",
        "darwin",
        "macos",
        "macosx",
        "ios",
        "tvos",
        "watchos",
        "freebsd",
        "netbsd",
        "openbsd",
        "dragonfly",
        "solaris",
        "haiku",
        "hurd",
        "android",
        "wasi",
        "emscripten",
        "fuchsia",
        "uefi",
        "mingw32",
        "mingw64",
        "cygwin",
        "illumos",
        "aix",
    }

    def is_os_token(token: str) -> bool:
        lowered = token.lower()
        return lowered in os_tokens or lowered in os_aliases

    arch = arch_aliases.get(parts[0].lower(), parts[0].lower())
    remainder = [part.lower() for part in parts[1:]]
    abi = None
    if remainder:
        last = remainder[-1]
        if len(remainder) >= 2 and last in abi_tokens and is_os_token(remainder[-2]):
            abi = abi_aliases.get(last, last)
            remainder = remainder[:-1]
        elif last in abi_tokens and last not in os_tokens:
            abi = abi_aliases.get(last, last)
            remainder = remainder[:-1]
    os_part = remainder[-1] if remainder else None
    vendor_parts = remainder[:-1] if len(remainder) > 1 else []
    if os_part is None:
        return f"{arch}-{abi}" if abi else arch
    os_token = os_part.lower()
    match = re.match(r"^(darwin|macosx|macos|ios|tvos|watchos)([0-9].*)$", os_token)
    if match:
        os_token = match.group(1)
    os_name = os_aliases.get(os_token, os_token)
    if os_name in {"unknown", "none"}:
        os_name = "freestanding"
    if os_name == "windows" and abi is None:
        if any(token in {"w64", "mingw32", "mingw64"} for token in vendor_parts):
            abi = "gnu"
    if os_name in {"mingw32", "mingw64"}:
        os_name = "windows"
        if abi is None:
            abi = "gnu"
    if os_name in {"macos", "ios", "tvos", "watchos"}:
        if abi == "sim":
            abi = "simulator"
        elif os_name == "macos":
            abi = None
        elif abi in {
            "gnu",
            "gnueabi",
            "gnueabihf",
            "gnuabi64",
            "gnux32",
            "musl",
            "musleabi",
            "musleabihf",
            "msvc",
            "android",
            "eabi",
            "eabihf",
            "uclibc",
        }:
            abi = None

    if abi:
        return f"{arch}-{os_name}-{abi}"
    return f"{arch}-{os_name}"


def _detect_macos_arch(obj_path: Path) -> str | None:
    try:
        result = subprocess.run(
            ["lipo", "-archs", str(obj_path)],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    archs = result.stdout.strip().split()
    return archs[0] if archs else None


def _detect_macos_deployment_target() -> str | None:
    env_target = os.environ.get("MOLT_MACOSX_DEPLOYMENT_TARGET")
    if env_target:
        return env_target
    env_target = os.environ.get("MACOSX_DEPLOYMENT_TARGET")
    if env_target:
        return env_target
    try:
        result = subprocess.run(
            ["xcrun", "--show-sdk-version"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    version = result.stdout.strip()
    return version or None


def build(
    file_path: str | None,
    target: Target = "native",
    parse_codec: ParseCodec = "msgpack",
    type_hint_policy: TypeHintPolicy = "ignore",
    fallback_policy: FallbackPolicy = "error",
    type_facts_path: str | None = None,
    pgo_profile: str | None = None,
    runtime_feedback: str | None = None,
    output: str | None = None,
    json_output: bool = False,
    verbose: bool = False,
    deterministic: bool = True,
    deterministic_warn: bool = False,
    trusted: bool = False,
    capabilities: CapabilityInput | None = None,
    cache: bool = True,
    cache_dir: str | None = None,
    cache_report: bool = False,
    sysroot: str | None = None,
    emit_ir: str | None = None,
    emit: EmitMode | None = None,
    out_dir: str | None = None,
    profile: BuildProfile = "release",
    linked: bool = False,
    linked_output: str | None = None,
    require_linked: bool = False,
    respect_pythonpath: bool = False,
    module: str | None = None,
    diagnostics: bool | None = None,
    diagnostics_file: str | None = None,
    diagnostics_verbosity: str | None = None,
    portable: bool = False,
) -> int:
    if isinstance(profile, bool):
        profile = "release"
    if profile not in {"dev", "release"}:
        return _fail(f"Invalid build profile: {profile}", json_output, command="build")
    # --portable: force baseline ISA for cross-machine reproducible codegen.
    if portable:
        os.environ["MOLT_PORTABLE"] = "1"
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.", json_output, command="build"
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="build")

    diagnostics_path_spec = (
        diagnostics_file.strip() if isinstance(diagnostics_file, str) else ""
    )
    diagnostics_enabled = (
        _build_diagnostics_enabled() if diagnostics is None else diagnostics
    )
    if diagnostics is False and diagnostics_path_spec:
        return _fail(
            "--diagnostics-file requires diagnostics to be enabled.",
            json_output,
            command="build",
        )
    if diagnostics_path_spec:
        diagnostics_enabled = True
    elif diagnostics_enabled:
        diagnostics_path_spec = os.environ.get(
            "MOLT_BUILD_DIAGNOSTICS_FILE", ""
        ).strip()
    resolved_diagnostics_verbosity = _resolve_build_diagnostics_verbosity(
        diagnostics_verbosity or os.environ.get("MOLT_BUILD_DIAGNOSTICS_VERBOSITY")
    )
    allocation_diagnostics_enabled = _build_allocation_diagnostics_enabled()
    if allocation_diagnostics_enabled and not tracemalloc.is_tracing():
        tracemalloc.start(25)
    frontend_timing_raw = os.environ.get("MOLT_FRONTEND_TIMINGS", "").strip()
    frontend_timing_enabled = diagnostics_enabled or bool(frontend_timing_raw)
    frontend_timing_threshold = 0.0
    if frontend_timing_raw and frontend_timing_raw.lower() not in {
        "1",
        "true",
        "yes",
        "all",
    }:
        try:
            frontend_timing_threshold = max(0.0, float(frontend_timing_raw))
        except ValueError:
            frontend_timing_threshold = 0.0
    frontend_module_timings: list[dict[str, Any]] = []
    midend_policy_outcomes_by_function: dict[str, dict[str, Any]] = {}
    midend_pass_stats_by_function: dict[str, dict[str, dict[str, Any]]] = {}
    frontend_parallel_details: dict[str, Any] = {
        "enabled": False,
        "workers": 0,
        "mode": "serial",
        "reason": "disabled",
        "policy": {},
        "layers": [],
        "worker_timings": [],
        "worker_summary": {
            "count": 0,
            "queue_ms_total": 0.0,
            "queue_ms_max": 0.0,
            "wait_ms_total": 0.0,
            "wait_ms_max": 0.0,
            "exec_ms_total": 0.0,
            "exec_ms_max": 0.0,
        },
    }
    diagnostics_start = time.perf_counter()
    phase_starts: dict[str, float] = {}
    backend_daemon_health: dict[str, Any] | None = None
    backend_daemon_cached: bool | None = None
    backend_daemon_cache_tier: str | None = None
    backend_daemon_config_digest: str | None = None
    module_reasons: dict[str, set[str]] = {}
    if diagnostics_enabled:
        phase_starts["resolve_entry"] = diagnostics_start

    stdlib_root = _stdlib_root_path()
    warnings: list[str] = []
    native_arch_perf_enabled = False
    if _native_arch_perf_requested():
        if target != "native":
            warnings.append(
                "Native-arch perf profile requested, but non-native target selected; ignoring."
            )
        else:
            _enable_native_arch_rustflags()
            native_arch_perf_enabled = True
    cwd_root = _find_project_root(Path.cwd())
    project_root = (
        _find_project_root(Path(file_path).resolve()) if file_path else cwd_root
    )
    if not _has_project_markers(project_root) and _has_project_markers(cwd_root):
        project_root = cwd_root
    molt_root = _find_molt_root(project_root, cwd_root)
    root_error = _require_molt_root(molt_root, json_output, "build")
    if root_error is not None:
        return root_error
    lock_error = _check_lockfiles(
        molt_root,
        json_output,
        warnings,
        deterministic,
        deterministic_warn,
        "build",
    )
    if lock_error is not None:
        return lock_error
    sysroot_path = _resolve_sysroot(project_root, sysroot)
    if sysroot_path is not None and not sysroot_path.exists():
        return _fail(
            f"Sysroot not found: {sysroot_path}",
            json_output,
            command="build",
        )
    pgo_profile_summary: PgoProfileSummary | None = None
    pgo_profile_path: Path | None = None
    runtime_feedback_summary: RuntimeFeedbackSummary | None = None
    runtime_feedback_path: Path | None = None
    pgo_hot_function_names: set[str] = set()
    if pgo_profile:
        summary, resolved, err = _load_pgo_profile(
            project_root,
            pgo_profile,
            warnings,
            json_output,
            command="build",
        )
        if err is not None:
            return err
        pgo_profile_summary = summary
        pgo_profile_path = resolved
    if pgo_profile_summary is not None:
        pgo_hot_function_names = {
            symbol.strip()
            for symbol in pgo_profile_summary.hot_functions
            if isinstance(symbol, str) and symbol.strip()
        }
    pgo_hot_function_names_sorted = tuple(sorted(pgo_hot_function_names))
    if runtime_feedback:
        summary, resolved, err = _load_runtime_feedback(
            project_root,
            runtime_feedback,
            warnings,
            json_output,
            command="build",
        )
        if err is not None:
            return err
        runtime_feedback_summary = summary
        runtime_feedback_path = resolved
    if runtime_feedback_summary is not None:
        pgo_hot_function_names.update(
            symbol.strip()
            for symbol in runtime_feedback_summary.hot_functions
            if isinstance(symbol, str) and symbol.strip()
        )
    pgo_profile_payload: dict[str, Any] | None = None
    if pgo_profile_summary is not None and pgo_profile_path is not None:
        pgo_profile_payload = {
            "path": str(pgo_profile_path),
            "version": pgo_profile_summary.version,
            "hash": pgo_profile_summary.hash,
            "hot_functions": pgo_profile_summary.hot_functions,
        }
    runtime_feedback_payload: dict[str, Any] | None = None
    if runtime_feedback_summary is not None and runtime_feedback_path is not None:
        runtime_feedback_payload = {
            "path": str(runtime_feedback_path),
            "schema_version": runtime_feedback_summary.schema_version,
            "hash": runtime_feedback_summary.hash,
            "hot_functions": runtime_feedback_summary.hot_functions,
        }
    cargo_timeout, timeout_err = _resolve_timeout_env("MOLT_CARGO_TIMEOUT")
    if timeout_err:
        return _fail(timeout_err, json_output, command="build")
    backend_timeout, timeout_err = _resolve_timeout_env("MOLT_BACKEND_TIMEOUT")
    if timeout_err:
        return _fail(timeout_err, json_output, command="build")
    link_timeout, timeout_err = _resolve_timeout_env("MOLT_LINK_TIMEOUT")
    if timeout_err:
        return _fail(timeout_err, json_output, command="build")
    frontend_phase_timeout, timeout_err = _resolve_timeout_env(
        "MOLT_FRONTEND_PHASE_TIMEOUT"
    )
    if timeout_err:
        return _fail(timeout_err, json_output, command="build")
    backend_profile, profile_err = _resolve_backend_profile(profile)
    if profile_err:
        return _fail(profile_err, json_output, command="build")
    runtime_cargo_profile, runtime_profile_err = _resolve_cargo_profile_name(profile)
    if runtime_profile_err:
        return _fail(runtime_profile_err, json_output, command="build")
    backend_cargo_profile, backend_profile_err = _resolve_cargo_profile_name(
        backend_profile
    )
    if backend_profile_err:
        return _fail(backend_profile_err, json_output, command="build")
    capabilities_list: list[str] | None = None
    capabilities_source = None
    capability_profiles: list[str] = []
    if capabilities is not None:
        parsed, profiles, source, errors = _parse_capabilities(capabilities)
        if errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(errors),
                json_output,
                command="build",
            )
        capabilities_list = parsed
        capability_profiles = profiles
        capabilities_source = source
    cwd_root = _find_project_root(Path.cwd())
    module_roots: list[Path] = []
    extra_roots = os.environ.get("MOLT_MODULE_ROOTS", "")
    if extra_roots:
        for entry in extra_roots.split(os.pathsep):
            if not entry:
                continue
            entry_path = Path(entry).expanduser()
            if entry_path.exists():
                module_roots.append(entry_path)
    for root in (project_root, cwd_root):
        if root.exists():
            module_roots.append(root)
        src_root = root / "src"
        if src_root.exists():
            module_roots.append(src_root)
        module_roots.extend(_vendor_roots(root))
    source_path: Path | None = None
    entry_module: str | None = None
    entry_module_import_alias: str | None = None
    if file_path:
        source_path = Path(file_path).resolve()
        if not source_path.exists():
            return _fail(f"File not found: {source_path}", json_output, command="build")
        module_roots.append(source_path.parent)
    if respect_pythonpath:
        pythonpath = os.environ.get("PYTHONPATH", "")
        if pythonpath:
            for entry in pythonpath.split(os.pathsep):
                if not entry:
                    continue
                entry_path = Path(entry).expanduser()
                if entry_path.exists():
                    module_roots.append(entry_path)
    module_roots = list(dict.fromkeys(root.resolve() for root in module_roots))
    if module:
        resolved = _resolve_entry_module(module, module_roots)
        if resolved is None:
            return _fail(
                f"Entry module not found: {module}",
                json_output,
                command="build",
            )
        entry_module, source_path = resolved
        module_roots.append(source_path.parent.resolve())
        module_roots = list(dict.fromkeys(module_roots))
    elif source_path is not None:
        entry_module = _module_name_from_path(source_path, module_roots, stdlib_root)
    if source_path is None or entry_module is None:
        return _fail("Failed to resolve entry module.", json_output, command="build")
    try:
        entry_source = _read_module_source(source_path)
    except (SyntaxError, UnicodeDecodeError) as exc:
        return _fail(
            f"Syntax error in {source_path}: {exc}",
            json_output,
            command="build",
        )
    except OSError as exc:
        return _fail(
            f"Failed to read entry module {source_path}: {exc}",
            json_output,
            command="build",
        )
    try:
        entry_tree = ast.parse(entry_source, filename=str(source_path))
    except SyntaxError as exc:
        return _fail(
            f"Syntax error in {source_path}: {exc}",
            json_output,
            command="build",
        )
    (
        entry_pkg_override_set,
        entry_pkg_override,
        entry_spec_override_set,
        entry_spec_override,
        entry_spec_override_is_package,
    ) = _infer_module_overrides(entry_tree)
    if diagnostics_enabled:
        phase_starts["module_graph"] = time.perf_counter()
    if entry_pkg_override_set and entry_pkg_override:
        root = _package_root_for_override(source_path, entry_pkg_override)
        if root is not None:
            source_parent = source_path.parent.resolve()
            module_roots = [
                candidate
                for candidate in module_roots
                if candidate.resolve() != source_parent
            ]
            module_roots.append(root)
            entry_module = _module_name_from_path(source_path, [root], stdlib_root)
    elif entry_spec_override_set and entry_spec_override:
        override_is_package = (
            entry_spec_override_is_package
            if entry_spec_override_is_package is not None
            else source_path.name == "__init__.py"
        )
        package_name = _spec_parent(entry_spec_override, override_is_package)
        if package_name:
            root = _package_root_for_override(source_path, package_name)
            if root is not None:
                source_parent = source_path.parent.resolve()
                module_roots = [
                    candidate
                    for candidate in module_roots
                    if candidate.resolve() != source_parent
                ]
                module_roots.append(root)
                entry_module = _module_name_from_path(source_path, [root], stdlib_root)
    module_roots = list(dict.fromkeys(root.resolve() for root in module_roots))
    if source_path is not None and entry_module is not None:
        source_parent = source_path.parent.resolve()
        alias_roots = [root for root in module_roots if root != source_parent]
        if alias_roots:
            alias_name = _module_name_from_path(source_path, alias_roots, stdlib_root)
            if alias_name and alias_name != entry_module:
                entry_module_import_alias = alias_name
    entry_imports = set(
        _collect_imports(entry_tree, entry_module, source_path.name == "__init__.py")
    )
    stub_skip_modules = STUB_MODULES - entry_imports
    stub_parents = STUB_PARENT_MODULES - entry_imports
    stdlib_allowlist = _stdlib_allowlist()
    roots = module_roots + [stdlib_root]
    module_resolution_cache = _ModuleResolutionCache()
    module_graph, explicit_imports = _discover_module_graph(
        source_path,
        roots,
        module_roots,
        stdlib_root,
        project_root,
        stdlib_allowlist,
        skip_modules=stub_skip_modules,
        stub_parents=stub_parents,
        resolver_cache=module_resolution_cache,
    )
    if diagnostics_enabled:
        for name in module_graph:
            _record_module_reason(module_reasons, name, "entry_closure")
    if (
        entry_module_import_alias
        and entry_module_import_alias not in module_graph
        and source_path is not None
    ):
        module_graph[entry_module_import_alias] = source_path
        if diagnostics_enabled:
            _record_module_reason(
                module_reasons, entry_module_import_alias, "entry_alias"
            )
    package_before = set(module_graph)
    _collect_package_parents(
        module_graph,
        roots,
        stdlib_root,
        stdlib_allowlist,
        resolver_cache=module_resolution_cache,
    )
    if diagnostics_enabled:
        _record_new_module_reasons(
            module_graph,
            package_before,
            module_reasons,
            "package_parent",
        )
    core_before = set(module_graph)
    _ensure_core_stdlib_modules(module_graph, stdlib_root)
    if diagnostics_enabled:
        _record_new_module_reasons(
            module_graph,
            core_before,
            module_reasons,
            "core_required",
        )
    intrinsic_enforced = _enforce_intrinsic_stdlib(
        module_graph, stdlib_root, json_output
    )
    if intrinsic_enforced is not None:
        return intrinsic_enforced
    core_paths = [
        path
        for name in (
            "builtins",
            "sys",
            "types",
            "importlib",
            "importlib.util",
            "importlib.machinery",
        )
        if (path := module_graph.get(name)) is not None
    ]
    for core_path in core_paths:
        core_graph, _ = _discover_module_graph(
            core_path,
            roots,
            module_roots,
            stdlib_root,
            project_root,
            stdlib_allowlist,
            skip_modules=stub_skip_modules,
            stub_parents=stub_parents,
            nested_stdlib_scan_modules=set(),
            resolver_cache=module_resolution_cache,
        )
        if diagnostics_enabled:
            _merge_module_graph_with_reason(
                module_graph,
                core_graph,
                module_reasons,
                "core_closure",
            )
        else:
            for name, path in core_graph.items():
                module_graph.setdefault(name, path)
    spawn_enabled = False
    spawn_required = target != "wasm" and _requires_spawn_entry_override(
        module_graph, explicit_imports
    )
    if spawn_required:
        spawn_path = module_resolution_cache.resolve_module(
            ENTRY_OVERRIDE_SPAWN,
            roots,
            stdlib_root,
            stdlib_allowlist,
        )
        if spawn_path is None:
            return _fail(
                (
                    f"Missing required stdlib module: {ENTRY_OVERRIDE_SPAWN}. "
                    "multiprocessing spawn entry override cannot be lowered."
                ),
                json_output,
                command="build",
            )
        spawn_enabled = True
        spawn_graph, _ = _discover_module_graph(
            spawn_path,
            roots,
            module_roots,
            stdlib_root,
            project_root,
            stdlib_allowlist,
            skip_modules=stub_skip_modules,
            stub_parents=stub_parents,
            resolver_cache=module_resolution_cache,
        )
        if diagnostics_enabled:
            _merge_module_graph_with_reason(
                module_graph,
                spawn_graph,
                module_reasons,
                "spawn_closure",
            )
        else:
            for name, path in spawn_graph.items():
                module_graph.setdefault(name, path)
    namespace_parents = _collect_namespace_parents(
        module_graph,
        roots,
        stdlib_root,
        stdlib_allowlist,
        explicit_imports,
        resolver_cache=module_resolution_cache,
    )
    if verbose and not json_output:
        print(f"Project root: {project_root}")
        print(f"Module roots: {', '.join(str(root) for root in module_roots)}")
        print(f"Modules discovered: {len(module_graph)}")
    output_base = _output_base_for_entry(entry_module, source_path)
    out_dir_path = _resolve_out_dir(project_root, out_dir)
    artifacts_root, bin_root, output_root = _resolve_output_roots(
        project_root, out_dir_path, output_base
    )

    def _record_frontend_timing(
        *,
        module_name: str,
        module_path: Path,
        visit_s: float,
        lower_s: float,
        total_s: float,
        timed_out: bool = False,
        detail: str | None = None,
    ) -> None:
        if not frontend_timing_enabled:
            return
        item: dict[str, Any] = {
            "module": module_name,
            "path": str(module_path),
            "visit_s": round(max(0.0, visit_s), 6),
            "lower_s": round(max(0.0, lower_s), 6),
            "total_s": round(max(0.0, total_s), 6),
            "timed_out": timed_out,
        }
        if detail:
            item["detail"] = detail
        frontend_module_timings.append(item)
        if (
            frontend_timing_raw
            and (timed_out or total_s >= frontend_timing_threshold)
            and not json_output
        ):
            suffix = f" timeout={detail}" if timed_out and detail else ""
            print(
                "frontend module timing: "
                f"{module_name} visit={visit_s:.3f}s lower={lower_s:.3f}s "
                f"total={total_s:.3f}s{suffix}",
                file=sys.stderr,
            )

    def _build_diagnostics_payload() -> tuple[dict[str, Any] | None, Path | None]:
        if not diagnostics_enabled:
            return None, None
        module_reason_map = {
            name: sorted(reasons) for name, reasons in sorted(module_reasons.items())
        }
        payload: dict[str, Any] = {
            "enabled": True,
            "total_sec": round(time.perf_counter() - diagnostics_start, 6),
            "phase_sec": _phase_duration_map(phase_starts),
            "module_count": len(module_graph),
            "module_reason_summary": _build_reason_summary(module_reasons),
            "module_reasons": module_reason_map,
        }
        if frontend_module_timings:
            payload["frontend_module_timings"] = frontend_module_timings
            payload["frontend_module_timings_top"] = sorted(
                frontend_module_timings,
                key=lambda item: float(item.get("total_s", 0.0)),
                reverse=True,
            )[:20]
        if allocation_diagnostics_enabled:
            allocations_payload = _capture_build_allocation_diagnostics()
            if allocations_payload is not None:
                payload["allocations"] = allocations_payload
        payload["frontend_parallel"] = dict(frontend_parallel_details)
        midend_payload = _build_midend_diagnostics_payload(
            requested_profile=profile,
            policy_outcomes_by_function=midend_policy_outcomes_by_function,
            pass_stats_by_function=midend_pass_stats_by_function,
        )
        if midend_payload is not None:
            payload["midend"] = midend_payload
        if backend_daemon_health is not None:
            payload["backend_daemon"] = backend_daemon_health
        if (
            backend_daemon_cached is not None
            or backend_daemon_cache_tier is not None
            or backend_daemon_config_digest is not None
        ):
            daemon_compile_info: dict[str, Any] = {}
            if backend_daemon_cached is not None:
                daemon_compile_info["cached"] = backend_daemon_cached
            if backend_daemon_cache_tier is not None:
                daemon_compile_info["cache_tier"] = backend_daemon_cache_tier
            if backend_daemon_config_digest is not None:
                daemon_compile_info["config_digest"] = backend_daemon_config_digest
            payload["backend_daemon_compile"] = daemon_compile_info
        out_path: Path | None = None
        if diagnostics_path_spec:
            out_path = _resolve_build_diagnostics_path(
                diagnostics_path_spec,
                artifacts_root,
            )
        return payload, out_path

    namespace_modules: dict[str, Path] = {}
    if namespace_parents:
        for name in sorted(namespace_parents):
            paths = _namespace_paths(
                name,
                _roots_for_module(name, roots, stdlib_root, stdlib_allowlist),
            )
            if not paths:
                continue
            stub_path = _write_namespace_module(name, paths, artifacts_root)
            namespace_modules[name] = stub_path
        if namespace_modules:
            module_graph.update(namespace_modules)
            if diagnostics_enabled:
                for name in namespace_modules:
                    _record_module_reason(module_reasons, name, "namespace_stub")
    namespace_module_names = set(namespace_modules)
    generated_module_source_paths: dict[str, str] = {
        name: _logical_generated_module_path(name) for name in namespace_modules
    }
    is_wasm = target == "wasm"
    is_rust_transpile = target == "rust"
    if trusted and is_wasm:
        return _fail(
            "Trusted mode is not supported for wasm targets",
            json_output,
            command="build",
        )
    if require_linked and not is_wasm:
        return _fail(
            "--require-linked is only supported for wasm targets",
            json_output,
            command="build",
        )
    if linked_output and not linked and not require_linked:
        return _fail(
            "--linked-output requires --linked",
            json_output,
            command="build",
        )
    if linked and not is_wasm:
        if not is_rust_transpile:
            return _fail(
                "Linked output is only supported for wasm targets",
                json_output,
                command="build",
            )
    if require_linked and not linked:
        linked = True
    # Default to linked mode for WASM targets (10-20% faster runtime, single
    # module output).  Opt out with MOLT_WASM_LINKED=0.
    if is_wasm and not linked:
        wasm_linked_env = os.environ.get("MOLT_WASM_LINKED", "1").strip().lower()
        if wasm_linked_env not in {"0", "false", "no", "off"}:
            linked = True
    target_triple = None if target in {"native", "wasm", "rust"} else target
    if is_rust_transpile:
        emit_mode = "bin"  # placeholder — not used for transpiler targets
    else:
        emit_mode = emit or ("wasm" if is_wasm else "bin")
    if not is_rust_transpile and emit_mode not in {"bin", "obj", "wasm"}:
        return _fail(
            f"Invalid emit mode: {emit_mode}",
            json_output,
            command="build",
        )
    if is_wasm and emit_mode != "wasm":
        return _fail(
            f"Invalid emit mode for wasm target: {emit_mode}",
            json_output,
            command="build",
        )
    if not is_wasm and not is_rust_transpile and emit_mode == "wasm":
        return _fail(
            "emit=wasm requires --target wasm",
            json_output,
            command="build",
        )
    output_binary: Path | None = None
    linked_output_path: Path | None = None
    if is_rust_transpile:
        output_rs = _resolve_output_path(
            output,
            output_root / f"{output_base}.rs",
            out_dir=out_dir_path,
            project_root=project_root,
        )
        output_artifact = output_rs
    elif is_wasm:
        output_wasm = _resolve_output_path(
            output,
            output_root / "output.wasm",
            out_dir=out_dir_path,
            project_root=project_root,
        )
        output_artifact = output_wasm
        if linked:
            stem = output_wasm.stem
            if stem.endswith("_linked"):
                stem = stem[: -len("_linked")]
            linked_output_path = output_wasm.with_name(
                f"{stem}_linked{output_wasm.suffix}"
            )
            if linked_output is not None:
                linked_output_path = _resolve_output_path(
                    linked_output,
                    linked_output_path,
                    out_dir=out_dir_path,
                    project_root=project_root,
                )
    else:
        output_obj = artifacts_root / "output.o"
        if emit_mode == "obj":
            output_obj = _resolve_output_path(
                output,
                output_root / "output.o",
                out_dir=out_dir_path,
                project_root=project_root,
            )
        output_artifact = output_obj
        if emit_mode == "bin":
            output_binary = _resolve_output_path(
                output,
                bin_root / f"{output_base}_molt",
                out_dir=out_dir_path,
                project_root=project_root,
            )
    for path in (output_artifact, output_binary):
        if path is not None and path.parent != Path("."):
            path.parent.mkdir(parents=True, exist_ok=True)
    emit_ir_path: Path | None = None
    if emit_ir:
        emit_ir_path = Path(emit_ir)
        if not emit_ir_path.is_absolute():
            emit_ir_path = artifacts_root / emit_ir_path
        if emit_ir_path.parent != Path("."):
            emit_ir_path.parent.mkdir(parents=True, exist_ok=True)
    for stub in stub_parents:
        if stub != entry_module:
            module_graph.pop(stub, None)
    if IMPORTER_MODULE_NAME not in module_graph:
        importer_names = sorted(
            {
                name
                for name in module_graph
                if name not in {IMPORTER_MODULE_NAME, "builtins"}
            }.union(stub_parents)
        )
        importer_path = _write_importer_module(importer_names, artifacts_root)
        module_graph[IMPORTER_MODULE_NAME] = importer_path
        if diagnostics_enabled:
            _record_module_reason(
                module_reasons, IMPORTER_MODULE_NAME, "importer_generated"
            )
    if IMPORTER_MODULE_NAME in module_graph:
        generated_module_source_paths.setdefault(
            IMPORTER_MODULE_NAME, _logical_generated_module_path(IMPORTER_MODULE_NAME)
        )
    machinery_path = _resolve_module_path("importlib.machinery", [stdlib_root])
    if machinery_path is not None:
        module_graph.setdefault("importlib.machinery", machinery_path)
        if diagnostics_enabled and "importlib.machinery" in module_graph:
            _record_module_reason(
                module_reasons,
                "importlib.machinery",
                "machinery_support",
            )
    if diagnostics_enabled:
        phase_starts["module_analysis"] = time.perf_counter()
    known_modules = set(module_graph.keys())
    stdlib_allowlist.update(STUB_MODULES)
    stdlib_allowlist.update(stub_parents)
    stdlib_allowlist.add("molt.stdlib")
    known_modules_sorted = tuple(sorted(known_modules))
    stdlib_allowlist_sorted = tuple(sorted(stdlib_allowlist))
    module_graph_metadata = _build_module_graph_metadata(
        module_graph,
        generated_module_source_paths=generated_module_source_paths,
        entry_module=entry_module,
        namespace_module_names=namespace_module_names,
    )
    module_deps: dict[str, set[str]] = {}
    module_sources: dict[str, str] = {}
    known_func_defaults: dict[str, dict[str, dict[str, Any]]] = {}
    module_trees: dict[str, ast.AST] = {}
    module_path_stats: dict[str, os.stat_result | None] = {}
    syntax_error_modules: dict[str, ModuleSyntaxErrorInfo] = {}
    analysis_cache_miss_modules: set[str] = set()
    interface_changed_modules: set[str] = set()
    for module_name, module_path in module_graph.items():
        try:
            (
                tree,
                module_imports,
                func_defaults,
                source,
                analysis_cache_hit,
                interface_changed,
                path_stat,
            ) = _load_module_analysis(
                module_path,
                module_name=module_name,
                is_package=module_graph_metadata.module_is_package_by_module[module_name],
                include_nested=True,
                source=None,
                logical_source_path=module_graph_metadata.logical_source_path_by_module[
                    module_name
                ],
                resolution_cache=module_resolution_cache,
                project_root=project_root,
            )
            module_path_stats[module_name] = path_stat
            if source is not None:
                module_sources[module_name] = source
            if not analysis_cache_hit:
                analysis_cache_miss_modules.add(module_name)
            if interface_changed:
                interface_changed_modules.add(module_name)
        except SyntaxError as exc:
            if module_name == entry_module:
                return _fail(
                    f"Syntax error in {module_path}: {exc}",
                    json_output,
                    command="build",
                )
            syntax_error_modules[module_name] = _syntax_error_info_from_exception(
                exc, path=module_path
            )
            module_deps[module_name] = set()
            known_func_defaults[module_name] = {}
            module_path_stats[module_name] = None
            continue
        except OSError as exc:
            return _fail(
                f"Failed to read module {module_path}: {exc}",
                json_output,
                command="build",
            )
        if tree is not None:
            module_trees[module_name] = tree
        module_deps[module_name] = _module_dependencies_from_imports(
            module_name,
            module_graph,
            module_imports,
        )
        known_func_defaults[module_name] = func_defaults
    (
        module_order,
        reverse_module_deps,
        has_back_edges,
        module_layers,
        module_dep_closures,
    ) = _analyze_module_schedule(module_graph, module_deps)
    dirty_lowering_modules = set(analysis_cache_miss_modules)
    dirty_lowering_modules.update(
        _dependent_module_closure(
            interface_changed_modules,
            module_deps,
            module_graph,
            reverse_module_deps=reverse_module_deps,
        )
    )
    if diagnostics_enabled:
        phase_starts["ir_lowering"] = time.perf_counter()
    type_facts = None
    if type_facts_path is None and type_hint_policy in {"trust", "check"}:
        type_facts, ty_ok = _collect_type_facts_for_build(
            list(module_graph.values()), type_hint_policy, source_path
        )
        if type_facts is None and type_hint_policy == "trust":
            return _fail(
                "Type facts unavailable; refusing trusted build.",
                json_output,
                command="build",
            )
        if type_hint_policy == "trust" and not ty_ok:
            return _fail(
                "ty check failed; refusing trusted build.",
                json_output,
                command="build",
            )
        if type_hint_policy == "check" and not ty_ok:
            warning = "ty check failed; continuing with guarded hints only."
            warnings.append(warning)
            if not json_output:
                print(warning, file=sys.stderr)
    if type_facts_path is not None:
        facts_path = Path(type_facts_path)
        if not facts_path.exists():
            return _fail(
                f"Type facts not found: {facts_path}",
                json_output,
                command="build",
            )
        try:
            type_facts = load_type_facts(facts_path)
        except (OSError, json.JSONDecodeError, ValueError) as exc:
            return _fail(
                f"Failed to load type facts: {exc}",
                json_output,
                command="build",
            )
    known_classes: dict[str, Any] = {}
    scoped_lowering_inputs = _build_scoped_lowering_inputs(
        module_graph,
        module_deps=module_deps,
        module_dep_closures=module_dep_closures,
        known_modules=known_modules,
        known_func_defaults=known_func_defaults,
        pgo_hot_function_names=pgo_hot_function_names,
        type_facts=cast(TypeFacts | None, type_facts),
    )
    module_graph_metadata = _build_module_graph_metadata(
        module_graph,
        generated_module_source_paths=generated_module_source_paths,
        entry_module=entry_module,
        namespace_module_names=namespace_module_names,
        module_sources=module_sources,
        module_deps=module_deps,
    )
    frontend_module_costs = module_graph_metadata.frontend_module_costs
    stdlib_like_by_module = module_graph_metadata.stdlib_like_by_module
    assert frontend_module_costs is not None
    assert stdlib_like_by_module is not None

    enable_phi = not is_wasm
    module_chunk_max_ops = 0
    if is_wasm:
        module_chunk_max_ops = 2000
        env_chunk_ops = os.environ.get("MOLT_WASM_MODULE_CHUNK_OPS")
        if env_chunk_ops:
            try:
                module_chunk_max_ops = max(0, int(env_chunk_ops))
            except ValueError:
                warnings.append(
                    "Invalid MOLT_WASM_MODULE_CHUNK_OPS; using default of 2000."
                )
    module_chunking = is_wasm and module_chunk_max_ops > 0
    if target_triple:
        _ensure_rustup_target(target_triple, warnings)

    frontend_parallel_config = _resolve_frontend_parallel_config(
        module_count=len(module_order),
        has_back_edges=has_back_edges,
        frontend_phase_timeout=frontend_phase_timeout,
    )
    (
        frontend_parallel_layers,
        frontend_parallel_worker_timings,
    ) = _initialize_frontend_parallel_details(
        frontend_parallel_details,
        frontend_parallel_config=frontend_parallel_config,
    )

    def _record_frontend_parallel_worker_timing(
        *,
        layer_index: int,
        module_name: str,
        module_path: Path,
        mode: str,
        queue_ms: float,
        wait_ms: float,
        exec_ms: float,
        roundtrip_ms: float,
        worker_pid: int | None,
    ) -> dict[str, Any]:
        item: dict[str, Any] = {
            "layer": layer_index,
            "module": module_name,
            "path": str(module_path),
            "mode": mode,
            "queue_ms": round(max(0.0, queue_ms), 6),
            "wait_ms": round(max(0.0, wait_ms), 6),
            "exec_ms": round(max(0.0, exec_ms), 6),
            "roundtrip_ms": round(max(0.0, roundtrip_ms), 6),
        }
        if isinstance(worker_pid, int):
            item["worker_pid"] = worker_pid
        frontend_parallel_worker_timings.append(item)
        return item

    frontend_layer_execution_context = _FrontendLayerExecutionContext(
        syntax_error_modules=syntax_error_modules,
        module_graph=module_graph,
        module_sources=module_sources,
        project_root=project_root,
        module_resolution_cache=module_resolution_cache,
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
        type_facts=type_facts,
        enable_phi=enable_phi,
        known_modules=known_modules,
        stdlib_allowlist=stdlib_allowlist,
        known_func_defaults=known_func_defaults,
        module_deps=module_deps,
        module_chunk_max_ops=module_chunk_max_ops,
        optimization_profile=profile,
        pgo_hot_function_names=pgo_hot_function_names,
        known_modules_sorted=known_modules_sorted,
        stdlib_allowlist_sorted=stdlib_allowlist_sorted,
        pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
        module_dep_closures=module_dep_closures,
        module_graph_metadata=module_graph_metadata,
        path_stat_by_module=module_path_stats,
        module_chunking=module_chunking,
        scoped_lowering_inputs=scoped_lowering_inputs,
        dirty_lowering_modules=dirty_lowering_modules,
        frontend_module_costs=frontend_module_costs,
        stdlib_like_by_module=stdlib_like_by_module,
        known_classes=known_classes,
    )
    serial_frontend_lowering_context = _SerialFrontendLoweringContext(
        syntax_error_modules=syntax_error_modules,
        module_trees=module_trees,
        module_sources=module_sources,
        generated_module_source_paths=generated_module_source_paths,
        module_resolution_cache=module_resolution_cache,
        project_root=project_root,
        dirty_lowering_modules=dirty_lowering_modules,
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
        type_facts=type_facts,
        enable_phi=enable_phi,
        known_modules=known_modules,
        stdlib_allowlist=stdlib_allowlist,
        known_func_defaults=known_func_defaults,
        module_deps=module_deps,
        module_chunking=module_chunking,
        module_chunk_max_ops=module_chunk_max_ops,
        optimization_profile=profile,
        pgo_hot_function_names=pgo_hot_function_names,
        known_modules_sorted=known_modules_sorted,
        stdlib_allowlist_sorted=stdlib_allowlist_sorted,
        pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
        module_dep_closures=module_dep_closures,
        scoped_lowering_inputs=scoped_lowering_inputs,
        module_graph_metadata=module_graph_metadata,
        module_path_stats=module_path_stats,
        known_classes=known_classes,
        frontend_phase_timeout=frontend_phase_timeout,
    )
    serial_frontend_lowering_hooks = _SerialFrontendLoweringHooks(
        record_frontend_timing=_record_frontend_timing,
        fail=_fail,
        json_output=json_output,
    )
    integration_state = _FrontendIntegrationState(functions=[], known_classes=known_classes)
    midend_diagnostics_state = _MidendDiagnosticsState(
        policy_outcomes_by_function=midend_policy_outcomes_by_function,
        pass_stats_by_function=midend_pass_stats_by_function,
    )
    functions = integration_state.functions
    global_code_ids = integration_state.global_code_ids

    def _register_global_code_id(symbol: str) -> int:
        return _register_global_code_id_with_state(integration_state, symbol)

    def _run_serial_frontend_lower(
        module_name: str,
        module_path: Path,
    ) -> tuple[
        dict[str, Any] | None,
        _FrontendModuleResultTimings | None,
        dict[str, Any] | None,
    ]:
        return _run_serial_frontend_lower_with_context(
            module_name,
            module_path,
            lowering_context=serial_frontend_lowering_context,
            lowering_hooks=serial_frontend_lowering_hooks,
        )

    frontend_layer_runtime_hooks = _FrontendLayerRuntimeHooks(
        warnings=warnings,
        frontend_parallel_details=frontend_parallel_details,
        record_frontend_parallel_worker_timing=_record_frontend_parallel_worker_timing,
        record_frontend_timing=_record_frontend_timing,
        integrate_module_frontend_result=functools.partial(
            _integrate_module_frontend_result_with_state,
            integration_state,
        ),
        accumulate_midend_diagnostics=functools.partial(
            _accumulate_midend_diagnostics_with_state,
            midend_diagnostics_state,
        ),
        fail=_fail,
        json_output=json_output,
        run_serial_frontend_lower=_run_serial_frontend_lower,
    )

    if frontend_parallel_config.enabled:
        frontend_layer_error = _run_frontend_parallel_enabled_layers(
            module_layers,
            execution_context=frontend_layer_execution_context,
            runtime_hooks=frontend_layer_runtime_hooks,
            frontend_parallel_config=frontend_parallel_config,
            frontend_parallel_layers=frontend_parallel_layers,
        )
    else:
        frontend_layer_error = _run_frontend_serial_disabled_layers(
            module_order,
            execution_context=frontend_layer_execution_context,
            runtime_hooks=frontend_layer_runtime_hooks,
            frontend_parallel_layers=frontend_parallel_layers,
            frontend_parallel_config=frontend_parallel_config,
        )
    if frontend_layer_error is not None:
        return frontend_layer_error

    _summarize_frontend_parallel_worker_timings(
        frontend_parallel_details,
        frontend_parallel_worker_timings,
    )

    entry_path: Path | None = None
    if entry_module != "__main__":
        entry_path = module_graph.get(entry_module)
        if entry_path is None:
            return _fail(
                f"Entry module not found: {entry_module}",
                json_output,
                command="build",
            )
        entry_lower_error = _lower_entry_module_as_main(
            lowering_context=_EntryFrontendLoweringContext(
                entry_module=entry_module,
                entry_path=entry_path,
                parse_codec=parse_codec,
                type_hint_policy=type_hint_policy,
                fallback_policy=fallback_policy,
                type_facts=type_facts,
                enable_phi=enable_phi,
                known_modules=known_modules,
                known_classes=known_classes,
                stdlib_allowlist=stdlib_allowlist,
                known_func_defaults=known_func_defaults,
                module_chunking=module_chunking,
                module_chunk_max_ops=module_chunk_max_ops,
                optimization_profile=profile,
                pgo_hot_function_names=pgo_hot_function_names,
                frontend_phase_timeout=frontend_phase_timeout,
            ),
            integration_state=integration_state,
            diagnostics_state=midend_diagnostics_state,
            record_frontend_timing=_record_frontend_timing,
            fail=_fail,
            json_output=json_output,
        )
        if entry_lower_error is not None:
            return entry_lower_error

    entry_init_name = "__main__" if entry_module != "__main__" else entry_module
    entry_init = SimpleTIRGenerator.module_init_symbol(entry_init_name)
    version_ops = _build_version_info_ops(
        register_global_code_id=_register_global_code_id
    )
    entry_ops = _build_entry_main_ops(
        entry_init=entry_init,
        version_ops=version_ops,
        register_global_code_id=_register_global_code_id,
    )
    entry_call_idx = _entry_call_index(entry_ops, entry_init)
    next_var = _next_tir_var_index(entry_ops)
    if "sys" in module_graph:
        next_var = _append_entry_sys_init_op(
            entry_ops,
            entry_init=entry_init,
            register_global_code_id=_register_global_code_id,
            next_var=next_var,
        )
        entry_call_idx = _entry_call_index(entry_ops, entry_init)
    module_code_ops, next_var = _build_module_code_ops(
        module_order=module_order,
        module_graph=module_graph,
        generated_module_source_paths=generated_module_source_paths,
        entry_module=entry_module,
        entry_path=entry_path,
        register_global_code_id=_register_global_code_id,
        next_var=next_var,
    )
    entry_ops[entry_call_idx:entry_call_idx] = module_code_ops
    if spawn_enabled:
        next_var = _replace_entry_call_with_spawn_override(
            entry_ops,
            entry_init=entry_init,
            register_global_code_id=_register_global_code_id,
            next_var=next_var,
        )
    entry_ops.insert(1, {"kind": "code_slots_init", "value": len(global_code_ids)})
    functions.append({"name": "molt_main", "params": [], "ops": entry_ops})
    isolate_bootstrap_ops = _build_isolate_bootstrap_ops(
        code_slot_count=len(global_code_ids),
        version_ops=version_ops,
        module_code_ops=module_code_ops,
    )
    functions.append(
        {"name": "molt_isolate_bootstrap", "params": [], "ops": isolate_bootstrap_ops}
    )
    import_ops = _build_isolate_import_ops(
        module_order=module_order,
        register_global_code_id=_register_global_code_id,
    )
    functions.append(
        {"name": "molt_isolate_import", "params": ["p0"], "ops": import_ops}
    )
    ir = _finalize_backend_ir(
        functions=functions,
        pgo_profile_summary=pgo_profile_summary,
        runtime_feedback_summary=runtime_feedback_summary,
    )
    if diagnostics_enabled:
        phase_starts["runtime_setup"] = time.perf_counter()
    emit_ir_error = _write_emitted_ir(emit_ir_path, ir)
    if emit_ir_error is not None:
        return _fail(emit_ir_error, json_output, command="build")
    backend_ir_bytes: bytes | None = None

    def _ensure_backend_ir_bytes() -> bytes:
        nonlocal backend_ir_bytes
        if backend_ir_bytes is None:
            backend_ir_bytes = _backend_ir_bytes(ir)
        return backend_ir_bytes

    runtime_state = _initialize_runtime_artifact_state(
        is_rust_transpile=is_rust_transpile,
        is_wasm=is_wasm,
        emit_mode=emit_mode,
        molt_root=molt_root,
        runtime_cargo_profile=runtime_cargo_profile,
        target_triple=target_triple,
    )
    runtime_lib = runtime_state.runtime_lib
    runtime_wasm = runtime_state.runtime_wasm
    runtime_reloc_wasm = runtime_state.runtime_reloc_wasm
    if runtime_lib is not None and not _ensure_runtime_lib_ready(
        runtime_state,
        target_triple=target_triple,
        json_output=json_output,
        runtime_cargo_profile=runtime_cargo_profile,
        molt_root=molt_root,
        cargo_timeout=cargo_timeout,
    ):
            return _fail("Runtime build failed", json_output, command="build")

    def ensure_runtime_wasm_shared() -> bool:
        return _ensure_runtime_wasm_artifact(
            runtime_state,
            reloc=False,
            json_output=json_output,
            cargo_profile=runtime_cargo_profile,
            cargo_timeout=cargo_timeout,
            project_root=molt_root,
        )

    def ensure_runtime_wasm_reloc() -> bool:
        return _ensure_runtime_wasm_artifact(
            runtime_state,
            reloc=True,
            json_output=json_output,
            cargo_profile=runtime_cargo_profile,
            cargo_timeout=cargo_timeout,
            project_root=molt_root,
        )

    cache_hit = False
    cache_hit_tier: str | None = None
    cache_key = None
    function_cache_key = None
    cache_path: Path | None = None
    function_cache_path: Path | None = None
    cache_candidates: list[tuple[str, Path]] = []

    if diagnostics_enabled:
        phase_starts["cache_lookup"] = time.perf_counter()
    if cache:
        cache_variant_parts = [
            f"profile={profile}",
            f"runtime_cargo={runtime_cargo_profile}",
            f"backend_cargo={backend_cargo_profile}",
            f"emit={emit_mode}",
            f"codegen_env={_backend_codegen_env_digest(is_wasm=is_wasm)}",
        ]
        if linked:
            cache_variant_parts.append("linked=1")
        cache_variant = ";".join(cache_variant_parts)
        module_cache_payload, backend_cache_payload = _cache_payloads_for_ir(ir)
        cache_key = _cache_key(
            ir,
            target,
            target_triple,
            cache_variant,
            payload=module_cache_payload,
        )
        function_cache_key = _function_cache_key(
            ir,
            target,
            target_triple,
            cache_variant,
            payload=backend_cache_payload,
        )
        cache_root = _resolve_cache_root(project_root, cache_dir)
        try:
            cache_root.mkdir(parents=True, exist_ok=True)
        except OSError as exc:
            warnings.append(f"Cache disabled: {exc}")
            cache = False
        else:
            ext = "wasm" if is_wasm else "o"
            cache_path = cache_root / f"{cache_key}.{ext}"

            if function_cache_key and function_cache_key != cache_key:
                function_cache_path = cache_root / f"{function_cache_key}.{ext}"

            if cache_path is not None:
                cache_candidates.append(("module", cache_path))
            if function_cache_path is not None and function_cache_path != cache_path:
                cache_candidates.append(("function", function_cache_path))
            cache_hit, cache_hit_tier = _try_cached_backend_candidates(
                project_root=project_root,
                cache_candidates=cache_candidates,
                output_artifact=output_artifact,
                is_wasm=is_wasm,
                cache_key=cache_key,
                function_cache_key=function_cache_key,
                cache_path=cache_path,
                warnings=warnings,
            )

    if (verbose or cache_report) and not json_output:
        if not cache:
            print("Cache: disabled")
        elif cache_key:
            cache_state = "hit" if cache_hit else "miss"
            cache_detail = f" ({cache_key})" if cache_key else ""
            if cache_hit and cache_hit_tier:
                cache_detail = f"{cache_detail} [{cache_hit_tier}]"
            print(f"Cache: {cache_state}{cache_detail}")

    compile_lock = (
        _build_lock(project_root, f"compile.{cache_key}")
        if cache and cache_key is not None
        else nullcontext()
    )
    with compile_lock:
        if not cache_hit and cache:
            cache_hit, cache_hit_tier = _try_cached_backend_candidates(
                project_root=project_root,
                cache_candidates=cache_candidates,
                output_artifact=output_artifact,
                is_wasm=is_wasm,
                cache_key=cache_key,
                function_cache_key=function_cache_key,
                cache_path=cache_path,
                warnings=warnings,
            )

        # 2. Backend: JSON IR -> output.o / output.wasm
        if not cache_hit:
            if diagnostics_enabled:
                now = time.perf_counter()
                if "backend_codegen" not in phase_starts:
                    phase_starts["backend_codegen"] = now
                if "backend_prepare" not in phase_starts:
                    phase_starts["backend_prepare"] = now
            backend_env = os.environ.copy() if is_wasm else None
            # Supply-chain: pin SOURCE_DATE_EPOCH for deterministic/release builds
            if deterministic or profile == "release":
                os.environ.setdefault("SOURCE_DATE_EPOCH", "315532800")
            reloc_requested = is_wasm and (
                linked or os.environ.get("MOLT_WASM_LINK") == "1"
            )
            if is_wasm and backend_env is not None:
                if "MOLT_WASM_DATA_BASE" not in backend_env:
                    if not ensure_runtime_wasm_shared():
                        return _fail(
                            "Runtime wasm build failed",
                            json_output,
                            command="build",
                        )
                if runtime_wasm is not None and runtime_wasm.exists():
                    data_base_candidates: list[int] = []
                    data_end = _read_wasm_data_end(runtime_wasm)
                    if data_end is not None:
                        data_base_candidates.append((data_end + 7) & ~7)
                    memory_min = _read_wasm_memory_min_bytes(runtime_wasm)
                    if memory_min is not None:
                        data_base_candidates.append((memory_min + 7) & ~7)
                    if data_base_candidates:
                        backend_env["MOLT_WASM_DATA_BASE"] = str(
                            max(data_base_candidates)
                        )
                    else:
                        warnings.append(
                            "Failed to read runtime memory layout; using default data base."
                        )
                if "MOLT_WASM_TABLE_BASE" not in backend_env:
                    table_probe_path = runtime_wasm
                    if reloc_requested:
                        if linked and not ensure_runtime_wasm_reloc():
                            return _fail(
                                "Runtime wasm build failed",
                                json_output,
                                command="build",
                            )
                        if (
                            linked
                            and runtime_reloc_wasm is not None
                            and runtime_reloc_wasm.exists()
                        ):
                            table_probe_path = runtime_reloc_wasm
                    if table_probe_path is not None and table_probe_path.exists():
                        table_base = _read_wasm_table_min(table_probe_path)
                        if table_base is not None:
                            backend_env["MOLT_WASM_TABLE_BASE"] = str(table_base)
                        else:
                            warnings.append(
                                "Failed to read runtime table size; using default table base."
                            )
            if reloc_requested and backend_env is not None:
                backend_env["MOLT_WASM_LINK"] = "1"
            backend_bin = _backend_bin_path(molt_root, backend_cargo_profile)
            if not _ensure_backend_binary(
                backend_bin,
                cargo_timeout=cargo_timeout,
                json_output=json_output,
                cargo_profile=backend_cargo_profile,
                project_root=molt_root,
            ):
                return _fail("Backend build failed", json_output, command="build")
            if not backend_bin.exists():
                return _fail("Backend binary missing", json_output, command="build")
            daemon_socket: Path | None = None
            daemon_ready = False
            if _backend_daemon_enabled():
                backend_daemon_config_digest = _backend_daemon_config_digest(
                    molt_root, backend_cargo_profile
                )
                if diagnostics_enabled and "backend_daemon_setup" not in phase_starts:
                    phase_starts["backend_daemon_setup"] = time.perf_counter()
                daemon_socket = _backend_daemon_socket_path(
                    molt_root,
                    backend_cargo_profile,
                    config_digest=backend_daemon_config_digest,
                )
                startup_timeout = _backend_daemon_start_timeout()
                with _build_lock(molt_root, f"backend-daemon.{backend_cargo_profile}"):
                    pid_path = _backend_daemon_pid_path(
                        molt_root, backend_cargo_profile
                    )
                    existing_pid = _read_backend_daemon_pid(pid_path)
                    if (
                        existing_pid is not None
                        and _pid_alive(existing_pid)
                        and _backend_daemon_binary_is_newer(backend_bin, pid_path)
                    ):
                        _terminate_backend_daemon_pid(existing_pid, grace=1.0)
                        _remove_backend_daemon_pid(pid_path)
                        try:
                            if daemon_socket.exists():
                                daemon_socket.unlink()
                        except OSError:
                            pass
                    daemon_ready = daemon_socket.exists()
                    if not daemon_ready:
                        daemon_ready = _start_backend_daemon(
                            backend_bin,
                            daemon_socket,
                            cargo_profile=backend_cargo_profile,
                            project_root=molt_root,
                            startup_timeout=startup_timeout,
                            json_output=json_output,
                        )
            if diagnostics_enabled and "backend_dispatch" not in phase_starts:
                phase_starts["backend_dispatch"] = time.perf_counter()

            backend_output_ctx: ContextManager[Path]
            if cache and cache_path is not None:
                backend_output_ctx = nullcontext(cache_path)
            else:
                backend_output_ctx = _temporary_backend_output_path(
                    artifacts_root,
                    is_wasm=is_wasm,
                )
            with backend_output_ctx as backend_output:
                backend_compiled = False
                backend_output_written = True
                backend_output_exists = False
                daemon_error: str | None = None
                backend_daemon_request_bytes_cached: bytes | None = None
                output_sync_state_path: Path | None = None
                output_sync_state: dict[str, Any] | None = None
                output_artifact_stat: os.stat_result | None = None
                if daemon_ready and daemon_socket is not None:
                    output_sync_state_path = _artifact_sync_state_path(
                        project_root, output_artifact
                    )
                    output_sync_state = _read_artifact_sync_state(
                        output_sync_state_path
                    )
                    try:
                        output_artifact_stat = output_artifact.stat()
                    except OSError:
                        output_artifact_stat = None
                    (
                        skip_module_output_if_synced,
                        skip_function_output_if_synced,
                    ) = _backend_daemon_skip_output_sync_flags(
                        project_root,
                        output_artifact,
                        cache_key=cache_key if cache else None,
                        function_cache_key=(
                            function_cache_key
                            if cache and function_cache_key != cache_key
                            else None
                        ),
                        state_path=output_sync_state_path,
                        state=output_sync_state,
                        output_stat=output_artifact_stat,
                    )
                    backend_daemon_request_bytes_cached, request_encode_err = (
                        _backend_daemon_compile_request_bytes(
                            ir=ir,
                            backend_output=backend_output,
                            is_wasm=is_wasm,
                            target_triple=target_triple,
                            cache_key=cache_key,
                            function_cache_key=function_cache_key,
                            config_digest=backend_daemon_config_digest,
                            skip_module_output_if_synced=skip_module_output_if_synced,
                            skip_function_output_if_synced=skip_function_output_if_synced,
                        )
                    )
                    if request_encode_err is not None:
                        return _fail(
                            request_encode_err,
                            json_output,
                            command="build",
                        )
                    if (
                        diagnostics_enabled
                        and "backend_daemon_compile" not in phase_starts
                    ):
                        phase_starts["backend_daemon_compile"] = time.perf_counter()
                    daemon_compile = _compile_with_backend_daemon(
                        daemon_socket,
                        ir=ir,
                        backend_output=backend_output,
                        is_wasm=is_wasm,
                        target_triple=target_triple,
                        cache_key=cache_key,
                        function_cache_key=function_cache_key,
                        config_digest=backend_daemon_config_digest,
                        skip_module_output_if_synced=skip_module_output_if_synced,
                        skip_function_output_if_synced=skip_function_output_if_synced,
                        timeout=None,
                        request_bytes=backend_daemon_request_bytes_cached,
                    )
                    backend_compiled = daemon_compile.ok
                    backend_output_written = daemon_compile.output_written
                    daemon_error = daemon_compile.error
                    backend_output_exists = daemon_compile.output_exists
                    if daemon_compile.cached is not None:
                        backend_daemon_cached = daemon_compile.cached
                    if daemon_compile.cache_tier is not None:
                        backend_daemon_cache_tier = daemon_compile.cache_tier
                    daemon_health = daemon_compile.health
                    if daemon_health is not None:
                        backend_daemon_health = daemon_health
                    if not backend_compiled and _backend_daemon_retryable_error(
                        daemon_error
                    ):
                        if (
                            diagnostics_enabled
                            and "backend_daemon_restart" not in phase_starts
                        ):
                            phase_starts["backend_daemon_restart"] = time.perf_counter()
                        restart_timeout = _backend_daemon_start_timeout()
                        with _build_lock(
                            molt_root, f"backend-daemon.{backend_cargo_profile}"
                        ):
                            daemon_ready = _start_backend_daemon(
                                backend_bin,
                                daemon_socket,
                                cargo_profile=backend_cargo_profile,
                                project_root=molt_root,
                                startup_timeout=restart_timeout,
                                json_output=json_output,
                            )
                        if daemon_ready:
                            daemon_compile = _compile_with_backend_daemon(
                                daemon_socket,
                                ir=ir,
                                backend_output=backend_output,
                                is_wasm=is_wasm,
                                target_triple=target_triple,
                                cache_key=cache_key,
                                function_cache_key=function_cache_key,
                                config_digest=backend_daemon_config_digest,
                                skip_module_output_if_synced=skip_module_output_if_synced,
                                skip_function_output_if_synced=(
                                    skip_function_output_if_synced
                                ),
                                timeout=None,
                                request_bytes=backend_daemon_request_bytes_cached,
                            )
                            backend_compiled = daemon_compile.ok
                            backend_output_written = daemon_compile.output_written
                            daemon_error = daemon_compile.error
                            backend_output_exists = daemon_compile.output_exists
                            if daemon_compile.cached is not None:
                                backend_daemon_cached = daemon_compile.cached
                            if daemon_compile.cache_tier is not None:
                                backend_daemon_cache_tier = daemon_compile.cache_tier
                            daemon_health = daemon_compile.health
                            if daemon_health is not None:
                                backend_daemon_health = daemon_health
                    if (
                        not backend_compiled
                        and verbose
                        and not json_output
                        and daemon_error
                    ):
                        print(
                            "Backend daemon compile failed; falling back to one-shot mode: "
                            f"{daemon_error}",
                            file=sys.stderr,
                        )
                if not backend_compiled:
                    if (
                        diagnostics_enabled
                        and "backend_subprocess_compile" not in phase_starts
                    ):
                        phase_starts["backend_subprocess_compile"] = time.perf_counter()
                    cmd = [str(backend_bin)]
                    if is_wasm:
                        cmd.extend(["--target", "wasm"])
                    elif target_triple:
                        cmd.extend(["--target-triple", target_triple])
                    cmd_with_output = cmd + ["--output", str(backend_output)]
                    try:
                        backend_process = subprocess.run(
                            cmd_with_output,
                            input=_ensure_backend_ir_bytes(),
                            capture_output=True,
                            env=backend_env,
                            timeout=backend_timeout,
                        )
                    except subprocess.TimeoutExpired:
                        return _fail(
                            "Backend compilation timed out",
                            json_output,
                            command="build",
                        )
                    if backend_process.returncode != 0:
                        backend_stderr = _subprocess_output_text(backend_process.stderr)
                        backend_stdout = _subprocess_output_text(backend_process.stdout)
                        if not json_output:
                            if backend_stderr:
                                print(backend_stderr, end="", file=sys.stderr)
                            if backend_stdout:
                                print(backend_stdout, end="")
                        return _fail(
                            "Backend compilation failed",
                            json_output,
                            backend_process.returncode or 1,
                            command="build",
                        )
                    if verbose and not json_output:
                        backend_stdout = _subprocess_output_text(backend_process.stdout)
                        backend_stderr = _subprocess_output_text(backend_process.stderr)
                        if backend_stdout:
                            print(backend_stdout, end="")
                        if backend_stderr:
                            print(backend_stderr, end="", file=sys.stderr)
                    backend_output_written = True
                if backend_output_written and not (
                    daemon_ready and backend_compiled and backend_output_exists
                ):
                    if not backend_output.exists():
                        return _fail(
                            "Backend output missing", json_output, command="build"
                        )
                if backend_output_written:
                    if (
                        diagnostics_enabled
                        and "backend_artifact_stage" not in phase_starts
                    ):
                        phase_starts["backend_artifact_stage"] = time.perf_counter()
                    if cache and cache_path is not None:
                        if (
                            diagnostics_enabled
                            and "backend_cache_write" not in phase_starts
                        ):
                            phase_starts["backend_cache_write"] = time.perf_counter()
                    stage_error = _stage_backend_output_and_caches(
                        project_root,
                        backend_output,
                        output_artifact,
                        cache_path=cache_path if cache else None,
                        cache_key=cache_key if cache else None,
                        function_cache_path=function_cache_path if cache else None,
                        warnings=warnings,
                        output_already_synced=(
                            skip_module_output_if_synced
                            if daemon_ready and cache and cache_key
                            else None
                        ),
                        state_path=output_sync_state_path,
                        state=output_sync_state,
                        output_stat=output_artifact_stat,
                    )
                    if stage_error is not None:
                        return _fail(stage_error, json_output, command="build")

    if is_wasm:
        output_wasm = output_artifact
        if linked:
            if not ensure_runtime_wasm_reloc():
                return _fail(
                    "Runtime wasm build failed",
                    json_output,
                    command="build",
                )
            if runtime_reloc_wasm is None:
                return _fail(
                    "Runtime wasm build failed",
                    json_output,
                    command="build",
                )
            if linked_output_path is None:
                linked_output_path = output_wasm.with_name("output_linked.wasm")
            if linked_output_path.parent != Path("."):
                linked_output_path.parent.mkdir(parents=True, exist_ok=True)
            tool = molt_root / "tools" / "wasm_link.py"
            link_process = subprocess.run(
                [
                    sys.executable,
                    str(tool),
                    "--runtime",
                    str(runtime_reloc_wasm),
                    "--input",
                    str(output_wasm),
                    "--output",
                    str(linked_output_path),
                ],
                cwd=molt_root,
                capture_output=True,
                text=True,
            )
            if link_process.returncode != 0:
                err = link_process.stderr.strip() or link_process.stdout.strip()
                msg = "Wasm link failed"
                if err:
                    msg = f"{msg}: {err}"
                return _fail(msg, json_output, command="build")
            if require_linked and linked_output_path is not None:
                if output_wasm != linked_output_path and output_wasm.exists():
                    try:
                        output_wasm.unlink()
                    except OSError as exc:
                        return _fail(
                            f"Failed to remove unlinked wasm: {exc}",
                            json_output,
                            command="build",
                        )
        primary_output = output_wasm
        if require_linked and linked_output_path is not None:
            primary_output = linked_output_path
        diagnostics_payload, diagnostics_path = _build_diagnostics_payload()
        if json_output:
            cache_info = _build_cache_info(
                enabled=cache,
                hit=cache_hit,
                cache_key=cache_key,
                function_cache_key=function_cache_key,
                cache_path=cache_path,
                function_cache_path=function_cache_path,
                cache_hit_tier=cache_hit_tier,
                backend_daemon_cached=backend_daemon_cached,
                backend_daemon_cache_tier=backend_daemon_cache_tier,
                backend_daemon_config_digest=backend_daemon_config_digest,
            )
            data = _build_common_build_json_data(
                target=target,
                target_triple=target_triple,
                source_path=source_path,
                output=primary_output,
                deterministic=deterministic,
                trusted=trusted,
                capabilities_list=capabilities_list,
                capability_profiles=capability_profiles,
                capabilities_source=capabilities_source,
                sysroot_path=sysroot_path,
                cache_info=cache_info,
                emit_mode=emit_mode,
                profile=profile,
                native_arch_perf_enabled=native_arch_perf_enabled,
            )
            data["linked"] = linked
            data["require_linked"] = require_linked
            _attach_build_metadata(
                data,
                diagnostics_payload=diagnostics_payload,
                pgo_profile_payload=pgo_profile_payload,
                runtime_feedback_payload=runtime_feedback_payload,
                emit_ir_path=emit_ir_path,
            )
            if linked_output_path is not None:
                data["linked_output"] = str(linked_output_path)
            _emit_build_success_json(
                data=data,
                warnings=warnings,
                json_output=json_output,
            )
        else:
            if require_linked:
                print(f"Successfully built {primary_output}")
            else:
                print(f"Successfully built {output_wasm}")
            if linked_output_path is not None and not require_linked:
                print(f"Successfully linked {linked_output_path}")
        _emit_build_diagnostics_if_present(
            diagnostics_payload=diagnostics_payload,
            diagnostics_path=diagnostics_path,
            json_output=json_output,
            verbosity=resolved_diagnostics_verbosity,
        )
        return 0

    output_obj = output_artifact
    if emit_mode == "obj":
        diagnostics_payload, diagnostics_path = _build_diagnostics_payload()
        if json_output:
            cache_info = _build_cache_info(
                enabled=cache,
                hit=cache_hit,
                cache_key=cache_key,
                function_cache_key=function_cache_key,
                cache_path=cache_path,
                function_cache_path=function_cache_path,
                cache_hit_tier=cache_hit_tier,
                backend_daemon_cached=backend_daemon_cached,
                backend_daemon_cache_tier=backend_daemon_cache_tier,
                backend_daemon_config_digest=backend_daemon_config_digest,
            )
            data = _build_common_build_json_data(
                target=target,
                target_triple=target_triple,
                source_path=source_path,
                output=output_obj,
                deterministic=deterministic,
                trusted=trusted,
                capabilities_list=capabilities_list,
                capability_profiles=capability_profiles,
                capabilities_source=capabilities_source,
                sysroot_path=sysroot_path,
                cache_info=cache_info,
                emit_mode=emit_mode,
                profile=profile,
                native_arch_perf_enabled=native_arch_perf_enabled,
            )
            data["artifacts"] = {"object": str(output_obj)}
            _attach_build_metadata(
                data,
                diagnostics_payload=diagnostics_payload,
                pgo_profile_payload=pgo_profile_payload,
                runtime_feedback_payload=runtime_feedback_payload,
                emit_ir_path=emit_ir_path,
            )
            _emit_build_success_json(
                data=data,
                warnings=warnings,
                json_output=json_output,
            )
        else:
            print(f"Successfully built {output_obj}")
        _emit_build_diagnostics_if_present(
            diagnostics_payload=diagnostics_payload,
            diagnostics_path=diagnostics_path,
            json_output=json_output,
            verbosity=resolved_diagnostics_verbosity,
        )
        return 0

    # 3. Linking: output.o + main.c -> binary
    main_c_content = _render_native_main_stub(
        trusted=trusted,
        capabilities_list=capabilities_list,
    )
    stub_path = artifacts_root / "main_stub.c"
    _write_text_if_changed(stub_path, main_c_content)

    if output_binary is None:
        return _fail("Binary output unavailable", json_output, command="build")
    if output_binary.parent != Path("."):
        output_binary.parent.mkdir(parents=True, exist_ok=True)
    if runtime_lib is None:
        runtime_lib = _runtime_lib_path(
            molt_root,
            runtime_cargo_profile,
            target_triple,
        )
    try:
        link_cmd, linker_hint, normalized_target = _build_native_link_command(
            output_obj=output_obj,
            stub_path=stub_path,
            runtime_lib=runtime_lib,
            output_binary=output_binary,
            target_triple=target_triple,
            sysroot_path=sysroot_path,
            profile=profile,
        )
    except RuntimeError as exc:
        return _fail(str(exc), json_output, command="build")
    if (
        normalized_target is not None
        and target_triple is not None
        and normalized_target != target_triple
    ):
        warnings.append(
            f"Zig target normalized to {normalized_target} from {target_triple}."
        )

    link_fingerprint_path = _link_fingerprint_path(
        project_root, output_binary, profile, target_triple
    )
    stored_link_fingerprint = _read_runtime_fingerprint(link_fingerprint_path)
    link_fingerprint = _link_fingerprint(
        project_root=project_root,
        inputs=[stub_path, output_obj, runtime_lib],
        link_cmd=link_cmd,
        stored_fingerprint=stored_link_fingerprint,
    )
    link_skipped = not _artifact_needs_rebuild(
        output_binary,
        link_fingerprint,
        stored_link_fingerprint,
    )
    if link_skipped:
        link_process = subprocess.CompletedProcess(
            args=link_cmd,
            returncode=0,
            stdout="",
            stderr="",
        )
    else:
        if diagnostics_enabled and "link" not in phase_starts:
            phase_starts["link"] = time.perf_counter()
        try:
            link_process = _run_native_link_command(
                link_cmd=link_cmd,
                json_output=json_output,
                link_timeout=link_timeout,
            )
        except subprocess.TimeoutExpired:
            return _fail("Linker timed out", json_output, command="build")

    if not link_skipped and link_process.returncode != 0 and linker_hint is not None:
        try:
            retry_process, _ = _retry_native_link_without_hint(
                link_cmd=link_cmd,
                linker_hint=linker_hint,
                json_output=json_output,
                link_timeout=link_timeout,
            )
        except subprocess.TimeoutExpired:
            return _fail("Linker timed out", json_output, command="build")
        if retry_process is not None and retry_process.returncode == 0:
            warnings.append(
                f"Linker fallback: -fuse-ld={linker_hint} failed; retried default linker."
            )
            link_process = retry_process

    if (
        not link_skipped
        and link_process.returncode == 0
        and sys.platform == "darwin"
        and not target_triple
    ):
        try:
            link_process = _validate_darwin_link_output(
                link_process=link_process,
                link_cmd=link_cmd,
                linker_hint=linker_hint,
                output_binary=output_binary,
                validation_kind="magic",
                json_output=json_output,
                link_timeout=link_timeout,
                warnings=warnings,
            )
        except subprocess.TimeoutExpired:
            return _fail("Linker timed out", json_output, command="build")

    if (
        not link_skipped
        and link_process.returncode == 0
        and sys.platform == "darwin"
        and not target_triple
    ):
        try:
            link_process = _validate_darwin_link_output(
                link_process=link_process,
                link_cmd=link_cmd,
                linker_hint=linker_hint,
                output_binary=output_binary,
                validation_kind="dyld",
                json_output=json_output,
                link_timeout=link_timeout,
                warnings=warnings,
            )
        except subprocess.TimeoutExpired:
            return _fail("Linker timed out", json_output, command="build")

    diagnostics_payload, diagnostics_path = _build_diagnostics_payload()
    return _emit_native_link_result(
        link_process=link_process,
        link_skipped=link_skipped,
        link_fingerprint=link_fingerprint,
        link_fingerprint_path=link_fingerprint_path,
        cache=cache,
        cache_hit=cache_hit,
        cache_key=cache_key,
        function_cache_key=function_cache_key,
        cache_path=cache_path,
        function_cache_path=function_cache_path,
        cache_hit_tier=cache_hit_tier,
        backend_daemon_cached=backend_daemon_cached,
        backend_daemon_cache_tier=backend_daemon_cache_tier,
        backend_daemon_config_digest=backend_daemon_config_digest,
        target=target,
        target_triple=target_triple,
        source_path=source_path,
        output_binary=output_binary,
        deterministic=deterministic,
        trusted=trusted,
        capabilities_list=capabilities_list,
        capability_profiles=capability_profiles,
        capabilities_source=capabilities_source,
        sysroot_path=sysroot_path,
        emit_mode=emit_mode,
        profile=profile,
        native_arch_perf_enabled=native_arch_perf_enabled,
        output_obj=output_obj,
        stub_path=stub_path,
        runtime_lib=runtime_lib,
        diagnostics_payload=diagnostics_payload,
        diagnostics_path=diagnostics_path,
        pgo_profile_payload=pgo_profile_payload,
        runtime_feedback_payload=runtime_feedback_payload,
        emit_ir_path=emit_ir_path,
        warnings=warnings,
        json_output=json_output,
        resolved_diagnostics_verbosity=resolved_diagnostics_verbosity,
    )


def run_script(
    file_path: str | None,
    module: str | None,
    script_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
    timing: bool = False,
    trusted: bool = False,
    capabilities: CapabilityInput | None = None,
    build_args: list[str] | None = None,
    build_profile: BuildProfile | None = None,
) -> int:
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.", json_output, command="run"
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="run")
    project_root = (
        _find_project_root(Path(file_path).resolve())
        if file_path
        else _find_project_root(Path.cwd())
    )
    molt_root = _find_molt_root(project_root, Path.cwd())
    source_path: Path | None = None
    entry_module_name: str | None = None
    if file_path:
        source_path = Path(file_path)
        if not source_path.exists():
            return _fail(f"File not found: {source_path}", json_output, command="run")
    elif module:
        cwd_root = _find_project_root(Path.cwd())
        module_roots = _resolve_module_roots(
            project_root,
            cwd_root,
            respect_pythonpath=_build_args_respect_pythonpath(build_args or []),
        )
        resolved = _resolve_entry_module(module, module_roots)
        if resolved is None:
            return _fail(
                f"Entry module not found: {module}",
                json_output,
                command="run",
            )
        entry_module_name, source_path = resolved
    env = _base_env(project_root, source_path, molt_root=molt_root)
    if file_path:
        env.update(_collect_env_overrides(file_path))
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    if capabilities is not None:
        parsed, _profiles, _source, errors = _parse_capabilities(capabilities)
        if errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(errors),
                json_output,
                command="run",
            )
        if parsed is not None:
            env["MOLT_CAPABILITIES"] = ",".join(parsed)

    build_args = list(build_args or [])
    capabilities_tmp: Path | None = None
    if build_profile is not None and not _build_args_has_profile_flag(build_args):
        build_args.extend(["--profile", build_profile])
    if trusted and not _build_args_has_trusted_flag(build_args):
        build_args.append("--trusted")
    if capabilities is not None and not _build_args_has_capabilities_flag(build_args):
        cap_arg, capabilities_tmp = _materialize_capabilities_arg(capabilities)
        build_args.extend(["--capabilities", cap_arg])
    build_cmd = [sys.executable, "-m", "molt.cli", "build", *build_args]
    if module:
        build_cmd.extend(["--module", module])
    else:
        build_cmd.append(file_path)
    try:
        if timing:
            build_res = _run_command_timed(
                build_cmd,
                env=env,
                cwd=project_root,
                verbose=verbose,
                capture_output=json_output,
            )
            if build_res.returncode != 0:
                if json_output:
                    data: dict[str, Any] = {
                        "returncode": build_res.returncode,
                        "timing": {"build_s": build_res.duration_s},
                    }
                    if build_res.stdout:
                        data["build_stdout"] = build_res.stdout
                    if build_res.stderr:
                        data["build_stderr"] = build_res.stderr
                    payload = _json_payload(
                        "run",
                        "error",
                        data=data,
                        errors=["build failed"],
                    )
                    _emit_json(payload, json_output=True)
                return build_res.returncode
        else:
            build_res = subprocess.run(
                build_cmd,
                env=env,
                cwd=project_root,
                capture_output=json_output,
                text=json_output,
            )
            if build_res.returncode != 0:
                if json_output:
                    data = {"returncode": build_res.returncode}
                    if build_res.stdout:
                        data["build_stdout"] = build_res.stdout
                    if build_res.stderr:
                        data["build_stderr"] = build_res.stderr
                    payload = _json_payload(
                        "run",
                        "error",
                        data=data,
                        errors=["build failed"],
                    )
                    _emit_json(payload, json_output=True)
                elif build_res.stdout:
                    print(build_res.stdout, end="")
                    if build_res.stderr:
                        print(build_res.stderr, end="", file=sys.stderr)
                return build_res.returncode
    finally:
        if capabilities_tmp is not None:
            try:
                capabilities_tmp.unlink()
            except OSError:
                pass
    emit_arg = _extract_emit_arg(build_args)
    if emit_arg and emit_arg != "bin":
        return _fail(
            f"Compiled run requires emit=bin (got {emit_arg})",
            json_output,
            command="run",
        )
    output_binary = _extract_output_arg(build_args)
    out_dir = _extract_out_dir_arg(build_args)
    out_dir_path = _resolve_out_dir(project_root, out_dir)
    if entry_module_name is None:
        cwd_root = _find_project_root(Path.cwd())
        module_roots = _resolve_module_roots(
            project_root,
            cwd_root,
            respect_pythonpath=_build_args_respect_pythonpath(build_args),
        )
        if source_path is not None:
            module_roots.append(source_path.parent.resolve())
            module_roots = list(dict.fromkeys(module_roots))
            entry_module_name = _module_name_from_path(
                source_path, module_roots, _stdlib_root_path()
            )
    if entry_module_name is None or source_path is None:
        return _fail("Failed to resolve entry module.", json_output, command="run")
    output_base = _output_base_for_entry(entry_module_name, source_path)
    _artifacts_root, bin_root, _output_root = _resolve_output_roots(
        project_root, out_dir_path, output_base
    )
    output_binary = _resolve_output_path(
        str(output_binary) if output_binary is not None else None,
        bin_root / f"{output_base}_molt",
        out_dir=out_dir_path,
        project_root=project_root,
    )
    if timing:
        run_res = _run_command_timed(
            [str(output_binary), *script_args],
            env=env,
            cwd=project_root,
            verbose=verbose,
            capture_output=json_output,
        )
        if not isinstance(build_res, _TimedResult) or not isinstance(
            run_res, _TimedResult
        ):
            raise RuntimeError("timed run expected")
        if json_output:
            data = {
                "returncode": run_res.returncode,
                "timing": {
                    "build_s": build_res.duration_s,
                    "run_s": run_res.duration_s,
                    "total_s": build_res.duration_s + run_res.duration_s,
                },
            }
            if run_res.stdout:
                data["stdout"] = run_res.stdout
            if run_res.stderr:
                data["stderr"] = run_res.stderr
            payload = _json_payload(
                "run",
                "ok" if run_res.returncode == 0 else "error",
                data=data,
            )
            _emit_json(payload, json_output=True)
        else:
            print("Timing (compiled):", file=sys.stderr)
            print(f"- build: {_format_duration(build_res.duration_s)}", file=sys.stderr)
            print(
                f"- run: {_format_duration(run_res.duration_s)}",
                file=sys.stderr,
            )
            total = build_res.duration_s + run_res.duration_s
            print(f"- total: {_format_duration(total)}", file=sys.stderr)
        return run_res.returncode
    return _run_command(
        [str(output_binary), *script_args],
        env=env,
        cwd=project_root,
        json_output=json_output,
        verbose=verbose,
        label="run",
    )


def compare(
    file_path: str | None,
    module: str | None,
    python_exe: str | None,
    script_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
    trusted: bool = False,
    capabilities: CapabilityInput | None = None,
    build_args: list[str] | None = None,
    rebuild: bool = False,
    build_profile: BuildProfile | None = None,
) -> int:
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.",
            json_output,
            command="compare",
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="compare")
    source_path: Path | None = None
    if file_path:
        source_path = Path(file_path)
        if not source_path.exists():
            return _fail(
                f"File not found: {source_path}", json_output, command="compare"
            )
    project_root = (
        _find_project_root(Path(file_path).resolve())
        if file_path
        else _find_project_root(Path.cwd())
    )
    molt_root = _find_molt_root(project_root, Path.cwd())
    env = _base_env(project_root, source_path, molt_root=molt_root)
    if file_path:
        env.update(_collect_env_overrides(file_path))
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    if capabilities is not None:
        parsed, _profiles, _source, errors = _parse_capabilities(capabilities)
        if errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(errors),
                json_output,
                command="compare",
            )
        if parsed is not None:
            env["MOLT_CAPABILITIES"] = ",".join(parsed)

    python_exe = _resolve_python_exe(python_exe)
    if module:
        cpy_cmd = [python_exe, "-m", module, *script_args]
    else:
        cpy_cmd = [python_exe, str(source_path), *script_args]
    cpy_res = _run_command_timed(
        cpy_cmd,
        env=env,
        cwd=project_root,
        verbose=verbose,
        capture_output=True,
    )

    build_args = list(build_args or [])
    capabilities_tmp: Path | None = None
    if build_profile is not None and not _build_args_has_profile_flag(build_args):
        build_args.extend(["--profile", build_profile])
    if rebuild and not _build_args_has_cache_flag(build_args):
        build_args.append("--no-cache")
    if trusted and not _build_args_has_trusted_flag(build_args):
        build_args.append("--trusted")
    if capabilities is not None and not _build_args_has_capabilities_flag(build_args):
        cap_arg, capabilities_tmp = _materialize_capabilities_arg(capabilities)
        build_args.extend(["--capabilities", cap_arg])
    emit_arg = _extract_emit_arg(build_args)
    if emit_arg and emit_arg != "bin":
        return _fail(
            f"Compare requires emit=bin (got {emit_arg})",
            json_output,
            command="compare",
        )
    build_cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--json",
        *build_args,
    ]
    if module:
        build_cmd.extend(["--module", module])
    else:
        build_cmd.append(file_path)
    try:
        build_res = _run_command_timed(
            build_cmd,
            env=env,
            cwd=project_root,
            verbose=verbose,
            capture_output=True,
        )
    finally:
        if capabilities_tmp is not None:
            try:
                capabilities_tmp.unlink()
            except OSError:
                pass
    if build_res.returncode != 0:
        if json_output:
            data: dict[str, Any] = {
                "returncode": build_res.returncode,
                "timing": {"build_s": build_res.duration_s},
            }
            if build_res.stdout:
                data["build_stdout"] = build_res.stdout
            if build_res.stderr:
                data["build_stderr"] = build_res.stderr
            payload = _json_payload(
                "compare",
                "error",
                data=data,
                errors=["build failed"],
            )
            _emit_json(payload, json_output=True)
        else:
            err = build_res.stderr or build_res.stdout
            if err:
                print(err, end="", file=sys.stderr)
        return build_res.returncode

    try:
        build_payload = json.loads(build_res.stdout.strip() or "{}")
    except json.JSONDecodeError:
        return _fail(
            "Failed to parse build JSON output.", json_output, command="compare"
        )
    output_str = build_payload.get("data", {}).get("output") or build_payload.get(
        "output"
    )
    if not output_str:
        return _fail(
            "Build output missing in JSON payload.", json_output, command="compare"
        )
    output_path = _resolve_binary_output(output_str)
    if output_path is None:
        return _fail(
            f"Compiled binary not found at {output_str}.",
            json_output,
            command="compare",
        )

    molt_res = _run_command_timed(
        [str(output_path), *script_args],
        env=env,
        cwd=project_root,
        verbose=verbose,
        capture_output=True,
    )

    stdout_match = cpy_res.stdout == molt_res.stdout
    stderr_match = cpy_res.stderr == molt_res.stderr
    exit_match = cpy_res.returncode == molt_res.returncode
    compare_ok = stdout_match and stderr_match and exit_match

    if json_output:
        data = {
            "entry": str(source_path),
            "python": python_exe,
            "output": str(output_path),
            "returncodes": {
                "cpython": cpy_res.returncode,
                "molt": molt_res.returncode,
                "build": build_res.returncode,
            },
            "match": {
                "stdout": stdout_match,
                "stderr": stderr_match,
                "exitcode": exit_match,
            },
            "timing": {
                "cpython_run_s": cpy_res.duration_s,
                "molt_build_s": build_res.duration_s,
                "molt_run_s": molt_res.duration_s,
                "molt_total_s": build_res.duration_s + molt_res.duration_s,
            },
            "cpython_stdout": cpy_res.stdout,
            "cpython_stderr": cpy_res.stderr,
            "molt_stdout": molt_res.stdout,
            "molt_stderr": molt_res.stderr,
        }
        payload = _json_payload(
            "compare",
            "ok" if compare_ok else "error",
            data=data,
        )
        _emit_json(payload, json_output=True)
        return 0 if compare_ok else 1

    print("Compare (CPython vs Molt):")
    print(
        f"- CPython run: {_format_duration(cpy_res.duration_s)} "
        f"(rc={cpy_res.returncode})"
    )
    print(f"- Molt build: {_format_duration(build_res.duration_s)}")
    print(
        f"- Molt run: {_format_duration(molt_res.duration_s)} "
        f"(rc={molt_res.returncode})"
    )
    total = build_res.duration_s + molt_res.duration_s
    print(f"- Molt total: {_format_duration(total)}")
    if cpy_res.duration_s > 0 and molt_res.duration_s > 0:
        speedup = cpy_res.duration_s / molt_res.duration_s
        print(f"- Molt speedup (run): {speedup:.2f}x")
    print(
        "- Output match: "
        f"stdout={'yes' if stdout_match else 'no'}, "
        f"stderr={'yes' if stderr_match else 'no'}, "
        f"exitcode={'yes' if exit_match else 'no'}"
    )
    if not compare_ok:
        if not stdout_match:
            print(
                f"- Stdout mismatch: CPython={len(cpy_res.stdout)} bytes, "
                f"Molt={len(molt_res.stdout)} bytes"
            )
        if not stderr_match:
            print(
                f"- Stderr mismatch: CPython={len(cpy_res.stderr)} bytes, "
                f"Molt={len(molt_res.stderr)} bytes"
            )
        if not exit_match:
            print(
                f"- Exitcode mismatch: CPython={cpy_res.returncode}, "
                f"Molt={molt_res.returncode}"
            )
        if verbose:
            print("CPython stdout:")
            print(cpy_res.stdout, end="" if cpy_res.stdout.endswith("\n") else "\n")
            print("Molt stdout:")
            print(molt_res.stdout, end="" if molt_res.stdout.endswith("\n") else "\n")
            print("CPython stderr:", file=sys.stderr)
            print(
                cpy_res.stderr,
                end="" if cpy_res.stderr.endswith("\n") else "\n",
                file=sys.stderr,
            )
            print("Molt stderr:", file=sys.stderr)
            print(
                molt_res.stderr,
                end="" if molt_res.stderr.endswith("\n") else "\n",
                file=sys.stderr,
            )
    return 0 if compare_ok else 1


def parity_run(
    file_path: str | None,
    module: str | None,
    python_exe: str | None,
    script_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
    timing: bool = False,
) -> int:
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.",
            json_output,
            command="parity-run",
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="parity-run")

    source_path: Path | None = None
    if file_path:
        source_path = Path(file_path)
        if not source_path.exists():
            return _fail(
                f"File not found: {source_path}",
                json_output,
                command="parity-run",
            )

    project_root = (
        _find_project_root(Path(file_path).resolve())
        if file_path
        else _find_project_root(Path.cwd())
    )
    molt_root = _find_molt_root(project_root, Path.cwd())
    env = _base_env(project_root, source_path, molt_root=molt_root)
    if file_path:
        env.update(_collect_env_overrides(file_path))

    python_exe = _resolve_python_exe(python_exe)
    if module:
        command = [python_exe, "-m", module, *script_args]
    else:
        command = [python_exe, str(source_path), *script_args]

    if timing:
        run_res = _run_command_timed(
            command,
            env=env,
            cwd=project_root,
            verbose=verbose,
            capture_output=json_output,
        )
        if json_output:
            data: dict[str, Any] = {
                "python": python_exe,
                "entry": module if module is not None else str(source_path),
                "returncode": run_res.returncode,
                "timing": {"cpython_run_s": run_res.duration_s},
            }
            if run_res.stdout:
                data["stdout"] = run_res.stdout
            if run_res.stderr:
                data["stderr"] = run_res.stderr
            payload = _json_payload(
                "parity-run",
                "ok" if run_res.returncode == 0 else "error",
                data=data,
            )
            _emit_json(payload, json_output=True)
        else:
            print("Timing (CPython parity-run):", file=sys.stderr)
            print(
                f"- run: {_format_duration(run_res.duration_s)} "
                f"(rc={run_res.returncode})",
                file=sys.stderr,
            )
        return run_res.returncode

    return _run_command(
        command,
        env=env,
        cwd=project_root,
        json_output=json_output,
        verbose=verbose,
        label="parity-run",
    )


def diff(
    file_path: str | None,
    python_version: str | None,
    build_profile: BuildProfile | None = None,
    trusted: bool = False,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "diff")
    if root_error is not None:
        return root_error
    env = _base_env(root, molt_root=root)
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    cmd = [sys.executable, "tests/molt_diff.py"]
    if python_version:
        cmd.extend(["--python-version", python_version])
    if build_profile is not None:
        cmd.extend(["--build-profile", build_profile])
    if file_path:
        cmd.append(file_path)
    return _run_command(
        cmd,
        env=env,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="diff",
    )


@contextmanager
def _temporary_env_overrides(overrides: dict[str, str]):
    previous: dict[str, str] = {}
    missing: list[str] = []
    for key, value in overrides.items():
        if key in os.environ:
            previous[key] = os.environ[key]
        else:
            missing.append(key)
        os.environ[key] = value
    try:
        yield
    finally:
        for key in missing:
            os.environ.pop(key, None)
        for key, value in previous.items():
            os.environ[key] = value


def _internal_batch_build_server(
    *, json_output: bool = False, verbose: bool = False
) -> int:
    del json_output
    del verbose

    def _emit_response(payload: dict[str, Any]) -> None:
        sys.stdout.write(json.dumps(payload, sort_keys=True) + "\n")
        sys.stdout.flush()

    for raw_line in sys.stdin:
        if not raw_line.strip():
            continue
        req_id: Any = None
        try:
            request = json.loads(raw_line)
        except json.JSONDecodeError as exc:
            _emit_response(
                {
                    "id": None,
                    "ok": False,
                    "error": f"invalid request JSON: {exc}",
                }
            )
            continue
        if not isinstance(request, dict):
            _emit_response(
                {"id": None, "ok": False, "error": "request must be an object"}
            )
            continue
        req_id = request.get("id")
        op = request.get("op")
        if op == "ping":
            _emit_response({"id": req_id, "ok": True, "pong": True})
            continue
        if op == "shutdown":
            _emit_response({"id": req_id, "ok": True, "shutdown": True})
            return 0
        if op != "build":
            _emit_response(
                {"id": req_id, "ok": False, "error": f"unsupported op: {op!r}"}
            )
            continue

        params = request.get("params")
        if not isinstance(params, dict):
            _emit_response({"id": req_id, "ok": False, "error": "missing build params"})
            continue
        env_overrides_raw = params.get("env_overrides", {})
        if not isinstance(env_overrides_raw, dict) or any(
            not isinstance(key, str) or not isinstance(value, str)
            for key, value in env_overrides_raw.items()
        ):
            _emit_response(
                {
                    "id": req_id,
                    "ok": False,
                    "error": "env_overrides must be a string->string object",
                }
            )
            continue
        env_overrides: dict[str, str] = dict(env_overrides_raw)
        stdout_buf = io.StringIO()
        stderr_buf = io.StringIO()
        try:
            with _temporary_env_overrides(env_overrides):
                with redirect_stdout(stdout_buf), redirect_stderr(stderr_buf):
                    rc = build(
                        file_path=params.get("file_path"),
                        target=cast(Target, params.get("target", "native")),
                        parse_codec=cast(ParseCodec, params.get("codec", "msgpack")),
                        type_hint_policy=cast(
                            TypeHintPolicy, params.get("type_hints", "ignore")
                        ),
                        fallback_policy=cast(
                            FallbackPolicy, params.get("fallback", "error")
                        ),
                        type_facts_path=params.get("type_facts"),
                        pgo_profile=params.get("pgo_profile"),
                        runtime_feedback=params.get("runtime_feedback"),
                        output=params.get("output"),
                        json_output=False,
                        verbose=bool(params.get("verbose", False)),
                        deterministic=bool(params.get("deterministic", True)),
                        deterministic_warn=bool(
                            params.get("deterministic_warn", False)
                        ),
                        trusted=bool(params.get("trusted", False)),
                        capabilities=params.get("capabilities"),
                        cache=bool(params.get("cache", True)),
                        cache_dir=params.get("cache_dir"),
                        cache_report=bool(params.get("cache_report", False)),
                        sysroot=params.get("sysroot"),
                        emit_ir=params.get("emit_ir"),
                        emit=cast(EmitMode | None, params.get("emit")),
                        out_dir=params.get("out_dir"),
                        profile=cast(BuildProfile, params.get("profile", "dev")),
                        linked=bool(params.get("linked", False)),
                        linked_output=params.get("linked_output"),
                        require_linked=bool(params.get("require_linked", False)),
                        respect_pythonpath=bool(
                            params.get("respect_pythonpath", False)
                        ),
                        module=params.get("module"),
                        diagnostics_verbosity=params.get("diagnostics_verbosity"),
                    )
        except Exception as exc:  # pragma: no cover - defensive server hardening
            _emit_response(
                {
                    "id": req_id,
                    "ok": False,
                    "error": f"batch build server exception: {exc}",
                    "stdout": stdout_buf.getvalue(),
                    "stderr": stderr_buf.getvalue(),
                }
            )
            continue
        _emit_response(
            {
                "id": req_id,
                "ok": rc == 0,
                "returncode": rc,
                "stdout": stdout_buf.getvalue(),
                "stderr": stderr_buf.getvalue(),
            }
        )
    return 0


def lint(json_output: bool = False, verbose: bool = False) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "lint")
    if root_error is not None:
        return root_error
    cmd = [sys.executable, "tools/dev.py", "lint"]
    return _run_command(
        cmd,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="lint",
    )


def test(
    suite: str,
    file_path: str | None,
    python_version: str | None,
    pytest_args: list[str],
    build_profile: BuildProfile | None = None,
    trusted: bool = False,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "test")
    if root_error is not None:
        return root_error
    env = _base_env(root, molt_root=root)
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    if suite == "dev":
        cmd = [sys.executable, "tools/dev.py", "test"]
    elif suite == "diff":
        cmd = [sys.executable, "tests/molt_diff.py"]
        if python_version:
            cmd.extend(["--python-version", python_version])
        if build_profile is not None:
            cmd.extend(["--build-profile", build_profile])
        if file_path:
            cmd.append(file_path)
    else:
        cmd = ["uv", "run", "--python", "3.12", "pytest", "-q"]
        if file_path:
            cmd.append(file_path)
        cmd.extend(pytest_args)
    return _run_command(
        cmd,
        env=env,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="test",
    )


def bench(
    wasm: bool,
    bench_args: list[str],
    bench_script: list[str] | None = None,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "bench")
    if root_error is not None:
        return root_error
    tool = "tools/bench_wasm.py" if wasm else "tools/bench.py"
    cmd = [sys.executable, tool]
    for script in bench_script or []:
        cmd.extend(["--script", script])
    cmd.extend(bench_args)
    return _run_command(
        cmd,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="bench",
    )


def profile(
    profile_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "profile")
    if root_error is not None:
        return root_error
    cmd = [sys.executable, "tools/profile.py", *profile_args]
    return _run_command(
        cmd,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="profile",
    )


def doctor(
    json_output: bool = False,
    verbose: bool = False,
    strict: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "doctor")
    if root_error is not None:
        return root_error
    checks: list[dict[str, Any]] = []
    warnings: list[str] = []
    errors: list[str] = []
    system = platform.system()

    def record(
        name: str,
        ok: bool,
        detail: str,
        *,
        level: Literal["warning", "error"] = "error",
        advice: list[str] | None = None,
    ) -> None:
        entry: dict[str, Any] = {"name": name, "ok": ok, "detail": detail}
        if not ok:
            entry["level"] = level
            if advice:
                entry["advice"] = advice
            message = f"{name}: {detail}"
            if advice:
                message = f"{message}. See advice."
            if level == "error":
                errors.append(message)
            else:
                warnings.append(message)
        checks.append(entry)

    def _python_advice() -> list[str]:
        if system == "Darwin":
            return ["brew install python@3.12", "Ensure python3 is on PATH"]
        if system == "Windows":
            return ["winget install Python.Python.3.12", "Reopen your terminal"]
        return ["Install Python 3.12+ via your package manager"]

    def _uv_advice() -> list[str]:
        if system == "Darwin":
            return ["brew install uv"]
        if system == "Windows":
            return ["winget install Astral.Uv", "or: scoop install uv"]
        return ["curl -LsSf https://astral.sh/uv/install.sh | sh"]

    def _rustup_advice() -> list[str]:
        if system == "Windows":
            return ["winget install Rustlang.Rustup", "Reopen your terminal"]
        return ["curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"]

    def _cargo_advice() -> list[str]:
        return _rustup_advice() + ["source $HOME/.cargo/env (Unix)"]

    def _clang_advice() -> list[str]:
        if system == "Darwin":
            return ["xcode-select --install"]
        if system == "Windows":
            return ["winget install LLVM.LLVM", "set CC=clang"]
        return ["sudo apt-get update", "sudo apt-get install -y clang lld"]

    def _resolved_env_dir(var: str) -> Path | None:
        raw = os.environ.get(var, "").strip()
        if not raw:
            return None
        path = Path(raw).expanduser()
        if not path.is_absolute():
            path = (root / path).absolute()
        return path

    def _is_within(path: Path, container: Path) -> bool:
        try:
            path.resolve().relative_to(container.resolve())
        except ValueError:
            return False
        return True

    python_ok = sys.version_info >= (3, 12)
    record(
        "python",
        python_ok,
        f"{sys.version.split()[0]} (requires >=3.12)",
        level="error",
        advice=_python_advice() if not python_ok else None,
    )

    uv_path = shutil.which("uv")
    record(
        "uv",
        bool(uv_path),
        uv_path or "not found",
        level="warning",
        advice=_uv_advice() if not uv_path else None,
    )

    cargo_path = shutil.which("cargo")
    record(
        "cargo",
        bool(cargo_path),
        cargo_path or "not found",
        level="error",
        advice=_cargo_advice() if not cargo_path else None,
    )

    rustup_path = shutil.which("rustup")
    record(
        "rustup",
        bool(rustup_path),
        rustup_path or "not found",
        level="warning",
        advice=_rustup_advice() if not rustup_path else None,
    )

    cc = os.environ.get("CC", "clang")
    cc_path = shutil.which(cc) or shutil.which("clang")
    record(
        "clang",
        bool(cc_path),
        cc_path or "not found",
        level="error",
        advice=_clang_advice() if not cc_path else None,
    )

    zig_path = shutil.which("zig")
    record(
        "zig",
        bool(zig_path),
        zig_path or "not found",
        level="warning",
        advice=["Install zig if you need wasm linking"] if not zig_path else None,
    )

    rustc_wrapper = os.environ.get("RUSTC_WRAPPER", "").strip()
    sccache_mode = os.environ.get("MOLT_USE_SCCACHE", "auto").strip().lower() or "auto"
    sccache_path = shutil.which("sccache")
    if rustc_wrapper:
        wrapper_name = Path(rustc_wrapper).name
        sccache_ok = wrapper_name == "sccache"
        sccache_detail = f"RUSTC_WRAPPER={rustc_wrapper}"
        sccache_advice = (
            [
                "Use RUSTC_WRAPPER=sccache for compile throughput",
                "or unset RUSTC_WRAPPER and set MOLT_USE_SCCACHE=auto",
            ]
            if not sccache_ok
            else None
        )
    elif sccache_mode in {"0", "false", "no", "off"}:
        sccache_ok = False
        sccache_detail = "disabled via MOLT_USE_SCCACHE"
        sccache_advice = ["Set MOLT_USE_SCCACHE=auto or 1 for faster rebuilds"]
    elif sccache_path is None:
        sccache_ok = False
        sccache_detail = "not found on PATH"
        sccache_advice = [
            "Install sccache and keep MOLT_USE_SCCACHE=auto (or set to 1)"
        ]
    else:
        sccache_ok = True
        sccache_detail = f"{sccache_path} (mode={sccache_mode})"
        sccache_advice = None
    record(
        "sccache",
        sccache_ok,
        sccache_detail,
        level="warning",
        advice=sccache_advice,
    )

    if os.name == "posix":
        daemon_enabled = _backend_daemon_enabled()
        daemon_raw = os.environ.get("MOLT_BACKEND_DAEMON", "1").strip() or "1"
        record(
            "backend-daemon",
            daemon_enabled,
            f"MOLT_BACKEND_DAEMON={daemon_raw}",
            level="warning",
            advice=["Set MOLT_BACKEND_DAEMON=1 for faster native compile loops"]
            if not daemon_enabled
            else None,
        )
    else:
        record("backend-daemon", True, "unsupported on non-posix hosts")

    cargo_target_dir = _resolved_env_dir("CARGO_TARGET_DIR")
    if cargo_target_dir is None:
        record(
            "cargo-target-dir",
            False,
            f"defaulting to {root / 'target'}",
            level="warning",
            advice=[
                "export CARGO_TARGET_DIR=<external>/cargo-target",
                "export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR",
            ],
        )
    else:
        record("cargo-target-dir", True, str(cargo_target_dir))

    molt_cache_dir = _resolved_env_dir("MOLT_CACHE")
    if molt_cache_dir is None:
        record(
            "molt-cache-dir",
            False,
            f"defaulting to {_default_molt_cache()}",
            level="warning",
            advice=["export MOLT_CACHE=<external>/molt_cache"],
        )
    else:
        record("molt-cache-dir", True, str(molt_cache_dir))

    diff_target_dir = _resolved_env_dir("MOLT_DIFF_CARGO_TARGET_DIR")
    if diff_target_dir is None:
        record(
            "molt-diff-target-dir",
            False,
            "not set",
            level="warning",
            advice=["export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR"],
        )
    elif cargo_target_dir is not None and diff_target_dir != cargo_target_dir:
        record(
            "molt-diff-target-dir",
            False,
            f"{diff_target_dir} (CARGO_TARGET_DIR={cargo_target_dir})",
            level="warning",
            advice=["Set MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR"],
        )
    else:
        record("molt-diff-target-dir", True, str(diff_target_dir))

    configured_ext_root = os.environ.get("MOLT_EXT_ROOT", "").strip()
    ext_root = (
        Path(configured_ext_root).expanduser().resolve()
        if configured_ext_root
        else None
    )
    if ext_root is not None and ext_root.is_dir():
        routed_paths: list[Path] = []
        if cargo_target_dir is not None:
            routed_paths.append(cargo_target_dir)
        if molt_cache_dir is not None:
            routed_paths.append(molt_cache_dir)
        ext_ok = bool(routed_paths) and all(
            _is_within(path, ext_root) for path in routed_paths
        )
        detail = (
            "CARGO_TARGET_DIR and MOLT_CACHE routed to configured artifact root"
            if ext_ok
            else "Set CARGO_TARGET_DIR and MOLT_CACHE under the configured artifact root"
        )
        record(
            "artifact-root-routing",
            ext_ok,
            detail,
            level="warning",
            advice=[
                "export MOLT_EXT_ROOT=<artifact-root>",
                "export CARGO_TARGET_DIR=$MOLT_EXT_ROOT/target",
                "export MOLT_CACHE=$MOLT_EXT_ROOT/.molt_cache",
            ]
            if not ext_ok
            else None,
        )
    else:
        record(
            "artifact-root",
            True,
            "Using repo-local canonical artifact roots",
            advice=[
                "Set MOLT_EXT_ROOT=<external-root> if you want shared external artifacts",
                "or keep repo-local target/tmp/log/cache roots for local development",
            ],
        )

    pyproject = root / "pyproject.toml"
    lock_path = root / "uv.lock"
    if pyproject.exists():
        record(
            "uv.lock",
            lock_path.exists(),
            str(lock_path),
            level="warning",
            advice=["uv sync", "or: uv lock"] if not lock_path.exists() else None,
        )
        if lock_path.exists():
            try:
                if lock_path.stat().st_mtime < pyproject.stat().st_mtime:
                    record(
                        "uv.lock_fresh",
                        False,
                        "uv.lock older than pyproject.toml",
                        level="warning",
                        advice=["uv lock", "or: uv sync"],
                    )
            except OSError:
                record(
                    "uv.lock_fresh",
                    False,
                    "unable to stat uv.lock",
                    level="warning",
                    advice=["Ensure uv.lock exists and is readable"],
                )

    runtime_lib = _runtime_lib_path(root, "release", None)
    record(
        "molt-runtime",
        runtime_lib.exists(),
        str(runtime_lib),
        level="warning",
        advice=["cargo build --release --package molt-runtime"]
        if not runtime_lib.exists()
        else None,
    )

    if rustup_path:
        try:
            result = subprocess.run(
                ["rustup", "target", "list", "--installed"],
                capture_output=True,
                text=True,
                check=False,
            )
        except OSError as exc:
            record("rustup-targets", False, f"failed to query: {exc}")
        else:
            targets = result.stdout.split()
            wasm_ok = any(
                target in targets
                for target in ("wasm32-wasip1", "wasm32-unknown-unknown")
            )
            record(
                "wasm-target",
                wasm_ok,
                "wasm32-wasip1 or wasm32-unknown-unknown",
                level="warning",
                advice=["rustup target add wasm32-wasip1"] if not wasm_ok else None,
            )

    failures = [
        check
        for check in checks
        if not check["ok"] and check.get("level", "error") == "error"
    ]
    status = "ok" if not failures else "error"
    if json_output:
        payload = _json_payload(
            "doctor",
            status,
            data={"checks": checks},
            warnings=warnings,
            errors=errors,
        )
        _emit_json(payload, json_output=True)
    else:
        for check in checks:
            if check["ok"]:
                print(f"OK: {check['name']} ({check['detail']})")
                continue
            level = check.get("level", "error").upper()
            print(f"{level}: {check['name']} ({check['detail']})")
            for hint in check.get("advice", []):
                print(f"  -> {hint}")
    if strict and any(not check["ok"] for check in checks):
        return 1
    return 0


def _strip_c_like_comments_and_literals(text: str) -> str:
    without_blocks = _C_BLOCK_COMMENT_RE.sub(" ", text)
    without_lines = _C_LINE_COMMENT_RE.sub(" ", without_blocks)
    return _C_STRING_LITERAL_RE.sub(" ", without_lines)


def _extract_py_c_api_tokens(text: str) -> set[str]:
    sanitized = _strip_c_like_comments_and_literals(text)
    return {match.group(0) for match in _PY_C_API_TOKEN_RE.finditer(sanitized)}


def _load_supported_py_c_api_surface(
    molt_root: Path,
) -> tuple[set[str], Path, str | None]:
    header_path = molt_root / "include" / "molt" / "Python.h"
    supported_tokens: set[str] = set()
    try:
        header_text = header_path.read_text()
    except OSError as exc:
        return set(), header_path, str(exc)
    supported_tokens.update(_extract_py_c_api_tokens(header_text))
    datetime_header = molt_root / "include" / "datetime.h"
    if datetime_header.exists():
        try:
            datetime_text = datetime_header.read_text()
        except OSError:
            datetime_text = ""
        if datetime_text:
            supported_tokens.update(_extract_py_c_api_tokens(datetime_text))
    numpy_include_root = molt_root / "include" / "numpy"
    if numpy_include_root.exists():
        for numpy_header in sorted(numpy_include_root.rglob("*.h")):
            try:
                numpy_text = numpy_header.read_text()
            except OSError:
                continue
            supported_tokens.update(_extract_py_c_api_tokens(numpy_text))
    return supported_tokens, header_path, None


def _resolve_extension_scan_sources(
    project_root: Path, explicit_sources: list[str] | None
) -> tuple[list[Path], list[str]]:
    errors: list[str] = []
    source_entries: list[str] = []
    if explicit_sources:
        source_entries = [
            entry for entry in explicit_sources if entry and entry.strip()
        ]
        if not source_entries:
            errors.append("--source must include at least one non-empty path")
    else:
        pyproject = _load_toml(project_root / "pyproject.toml")
        extension_meta = _config_value(pyproject, ["tool", "molt", "extension"])
        if not isinstance(extension_meta, dict):
            errors.append("pyproject.toml must contain [tool.molt.extension]")
        else:
            source_entries = _coerce_str_list(
                extension_meta.get("sources"),
                "tool.molt.extension.sources",
                errors,
                allow_empty=False,
            )
            if not source_entries:
                errors.append(
                    "tool.molt.extension.sources must include at least one source"
                )
    source_paths: list[Path] = []
    for entry in source_entries:
        source_path = Path(entry).expanduser()
        if not source_path.is_absolute():
            source_path = (project_root / source_path).absolute()
        if not source_path.exists() or not source_path.is_file():
            errors.append(f"source file not found: {source_path}")
            continue
        source_paths.append(source_path)
    return source_paths, errors


def extension_scan(
    project: str | None = None,
    sources: list[str] | None = None,
    fail_on_missing: bool = False,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    project_root = Path(project).expanduser() if project else Path.cwd()
    if not project_root.is_absolute():
        project_root = (Path.cwd() / project_root).absolute()
    if not project_root.exists() or not project_root.is_dir():
        return _fail(
            f"Project directory not found: {project_root}",
            json_output,
            command="extension-scan",
        )

    source_paths, errors = _resolve_extension_scan_sources(project_root, sources)
    if errors:
        return _fail(
            "Extension scan configuration errors: " + "; ".join(errors),
            json_output,
            command="extension-scan",
        )

    cwd_root = _find_project_root(Path.cwd())
    molt_root = _find_molt_root(project_root, cwd_root)
    root_error = _require_molt_root(molt_root, json_output, "extension-scan")
    if root_error is not None:
        return root_error

    supported_surface, header_path, header_error = _load_supported_py_c_api_surface(
        molt_root
    )
    if header_error is not None:
        return _fail(
            f"Failed to read libmolt Python.h surface ({header_path}): {header_error}",
            json_output,
            command="extension-scan",
        )

    required_by_file: dict[str, list[str]] = {}
    missing_by_file: dict[str, list[str]] = {}
    required_symbols: set[str] = set()
    for source_path in source_paths:
        try:
            source_text = source_path.read_text()
        except OSError as exc:
            return _fail(
                f"Failed to read source file {source_path}: {exc}",
                json_output,
                command="extension-scan",
            )
        file_required = sorted(_extract_py_c_api_tokens(source_text))
        required_by_file[str(source_path)] = file_required
        required_symbols.update(file_required)
        file_missing = sorted(
            symbol for symbol in file_required if symbol not in supported_surface
        )
        if file_missing:
            missing_by_file[str(source_path)] = file_missing

    required_sorted = sorted(required_symbols)
    missing_sorted = sorted(
        symbol for symbol in required_sorted if symbol not in supported_surface
    )
    supported_used_sorted = sorted(
        symbol for symbol in required_sorted if symbol in supported_surface
    )
    warnings: list[str] = []
    if missing_sorted and not fail_on_missing:
        warnings.append(
            "Unsupported Py* C-API symbols detected (run with --fail-on-missing to gate)."
        )
    status = "ok"
    if fail_on_missing and missing_sorted:
        status = "error"

    if json_output:
        payload = _json_payload(
            "extension-scan",
            status,
            data={
                "project": str(project_root),
                "header": str(header_path),
                "source_count": len(source_paths),
                "required_symbol_count": len(required_sorted),
                "supported_symbol_count": len(supported_used_sorted),
                "missing_symbol_count": len(missing_sorted),
                "required_symbols": required_sorted,
                "supported_symbols": supported_used_sorted,
                "missing_symbols": missing_sorted,
                "required_by_file": required_by_file,
                "missing_by_file": missing_by_file,
                "fail_on_missing": fail_on_missing,
            },
            warnings=warnings,
            errors=["unsupported C-API symbols found"] if status == "error" else None,
        )
        _emit_json(payload, json_output=True)
    else:
        print(f"Extension C-API scan header: {header_path}")
        print(f"Scanned source files: {len(source_paths)}")
        print(f"Required Py* symbols: {len(required_sorted)}")
        print(f"Supported Py* symbols used: {len(supported_used_sorted)}")
        print(f"Missing Py* symbols: {len(missing_sorted)}")
        if missing_sorted:
            limit = len(missing_sorted) if verbose else min(30, len(missing_sorted))
            for symbol in missing_sorted[:limit]:
                print(f"MISSING: {symbol}")
            if limit < len(missing_sorted):
                print(f"... {len(missing_sorted) - limit} additional symbols omitted")
        if verbose and missing_by_file:
            for file_path in sorted(missing_by_file):
                print(f"{file_path}: {', '.join(missing_by_file[file_path])}")
        for warning in warnings:
            print(f"WARN: {warning}")

    if status == "error":
        return 1
    return 0


def extension_build(
    project: str | None = None,
    out_dir: str | None = None,
    molt_abi: str | None = None,
    capabilities: CapabilityInput | None = None,
    deterministic: bool = True,
    profile: BuildProfile = "release",
    target: str | None = None,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    project_root = Path(project).expanduser() if project else Path.cwd()
    if not project_root.is_absolute():
        project_root = (Path.cwd() / project_root).absolute()
    if not project_root.exists() or not project_root.is_dir():
        return _fail(
            f"Project directory not found: {project_root}",
            json_output,
            command="extension-build",
        )

    pyproject = _load_toml(project_root / "pyproject.toml")
    project_meta = pyproject.get("project")
    extension_meta = _config_value(pyproject, ["tool", "molt", "extension"])
    errors: list[str] = []
    warnings: list[str] = []

    if not isinstance(project_meta, dict):
        return _fail(
            "pyproject.toml must contain a [project] table.",
            json_output,
            command="extension-build",
        )
    if not isinstance(extension_meta, dict):
        return _fail(
            "pyproject.toml must contain [tool.molt.extension].",
            json_output,
            command="extension-build",
        )

    project_name = project_meta.get("name")
    project_version = project_meta.get("version")
    if not isinstance(project_name, str) or not project_name.strip():
        errors.append("project.name must be a non-empty string")
    if not isinstance(project_version, str) or not project_version.strip():
        errors.append("project.version must be a non-empty string")

    module_name = extension_meta.get("module")
    if not isinstance(module_name, str):
        errors.append("tool.molt.extension.module must be a string")
        module_name = ""
    module_parts = _module_parts(module_name)
    if module_parts is None:
        errors.append("tool.molt.extension.module must be a dotted Python identifier")
        module_parts = ["extension"]

    raw_sources = _coerce_str_list(
        extension_meta.get("sources"),
        "tool.molt.extension.sources",
        errors,
        allow_empty=False,
    )
    if not raw_sources:
        errors.append("tool.molt.extension.sources must include at least one source")
    source_paths: list[Path] = []
    for entry in raw_sources:
        source_path = Path(entry).expanduser()
        if not source_path.is_absolute():
            source_path = (project_root / source_path).absolute()
        if not source_path.exists() or not source_path.is_file():
            errors.append(f"source file not found: {source_path}")
            continue
        source_paths.append(source_path)

    include_dirs_raw = _coerce_str_list(
        extension_meta.get("include_dirs") or extension_meta.get("include-dirs"),
        "tool.molt.extension.include_dirs",
        errors,
    )
    include_paths: list[Path] = []
    for entry in include_dirs_raw:
        include_path = Path(entry).expanduser()
        if not include_path.is_absolute():
            include_path = (project_root / include_path).absolute()
        include_paths.append(include_path)

    compile_args = _coerce_str_list(
        extension_meta.get("extra_compile_args")
        or extension_meta.get("extra-compile-args"),
        "tool.molt.extension.extra_compile_args",
        errors,
    )
    link_args = _coerce_str_list(
        extension_meta.get("extra_link_args") or extension_meta.get("extra-link-args"),
        "tool.molt.extension.extra_link_args",
        errors,
    )

    effects = _normalize_effects(extension_meta.get("effects"))
    determinism_mode = "deterministic" if deterministic else "nondet"
    determinism_raw = extension_meta.get("determinism")
    if determinism_raw is not None:
        if not isinstance(determinism_raw, str):
            errors.append(
                "tool.molt.extension.determinism must be 'deterministic' or 'nondet'"
            )
        else:
            normalized = determinism_raw.strip().lower()
            if normalized not in {"deterministic", "nondet"}:
                errors.append(
                    "tool.molt.extension.determinism must be 'deterministic' or "
                    "'nondet'"
                )
            else:
                determinism_mode = normalized
    if deterministic:
        determinism_mode = "deterministic"

    requested_target = (target or "native").strip()
    if not requested_target:
        requested_target = "native"
    normalized_target = requested_target.lower()
    runtime_target_triple: str | None = None
    manifest_target_triple = _host_target_triple()
    if normalized_target == "native":
        runtime_target_triple = None
    elif normalized_target == "wasm" or normalized_target.startswith("wasm32"):
        errors.append("Extension build only supports native target triples, not wasm")
    else:
        if any(ch.isspace() for ch in requested_target):
            errors.append("target must be 'native' or a Rust target triple")
        runtime_target_triple = normalized_target
        manifest_target_triple = normalized_target

    capability_input: CapabilityInput | None = capabilities
    if capability_input is None:
        cfg_capabilities = extension_meta.get("capabilities")
        if isinstance(cfg_capabilities, (str, list, dict)):
            capability_input = cfg_capabilities
    if capability_input is None:
        errors.append(
            "Missing extension capabilities: set tool.molt.extension.capabilities "
            "or pass --capabilities."
        )
    capabilities_list: list[str] = []
    capability_profiles: list[str] = []
    if capability_input is not None:
        spec = _parse_capabilities_spec(capability_input)
        if spec.errors:
            errors.append("Invalid capabilities: " + ", ".join(spec.errors))
        else:
            capabilities_list = spec.capabilities or []
            capability_profiles = spec.profiles

    cwd_root = _find_project_root(Path.cwd())
    molt_root = _find_molt_root(project_root, cwd_root)
    root_error = _require_molt_root(molt_root, json_output, "extension-build")
    if root_error is not None:
        return root_error

    lock_error = _check_lockfiles(
        molt_root,
        json_output,
        warnings,
        deterministic,
        False,
        "extension-build",
    )
    if lock_error is not None:
        return lock_error

    default_abi = _default_molt_c_api_version(molt_root)
    abi_raw = molt_abi or extension_meta.get("molt_c_api_version") or default_abi
    if not isinstance(abi_raw, str):
        errors.append("molt ABI must be a string")
        abi_raw = default_abi
    abi_version = abi_raw.strip()
    if _MOLT_C_API_VERSION_RE.match(abi_version) is None:
        errors.append(
            "Invalid molt ABI version. Expected MAJOR[.MINOR[.PATCH]] "
            f"(got {abi_version!r})."
        )
    abi_major = abi_version.split(".", 1)[0] if abi_version else "0"
    abi_tag = f"molt_abi{abi_major}"

    if errors:
        return _fail(
            "Extension build configuration errors: " + "; ".join(errors),
            json_output,
            command="extension-build",
        )

    output_root = Path(out_dir).expanduser() if out_dir else Path("dist")
    if not output_root.is_absolute():
        output_root = (project_root / output_root).absolute()
    output_root.mkdir(parents=True, exist_ok=True)

    cargo_timeout, timeout_err = _resolve_timeout_env("MOLT_CARGO_TIMEOUT")
    if timeout_err:
        return _fail(timeout_err, json_output, command="extension-build")
    runtime_cargo_profile, runtime_profile_err = _resolve_cargo_profile_name("release")
    if runtime_profile_err:
        return _fail(runtime_profile_err, json_output, command="extension-build")
    if runtime_target_triple:
        _ensure_rustup_target(runtime_target_triple, warnings)
    runtime_lib = _runtime_lib_path(
        molt_root,
        runtime_cargo_profile,
        runtime_target_triple,
    )
    if not _ensure_runtime_lib(
        runtime_lib,
        runtime_target_triple,
        json_output,
        runtime_cargo_profile,
        molt_root,
        cargo_timeout,
    ):
        return _fail("Runtime build failed", json_output, command="extension-build")

    include_root = molt_root / "include"
    if not include_root.exists():
        return _fail(
            f"Missing Molt header root: {include_root}",
            json_output,
            command="extension-build",
        )

    cc = os.environ.get("CC", "clang")
    cc_cmd = shlex.split(cc)
    if not cc_cmd:
        return _fail(
            "Compiler command is empty. Set CC or install clang.",
            json_output,
            command="extension-build",
        )
    if runtime_target_triple:
        cross_cc = os.environ.get("MOLT_CROSS_CC")
        target_arg = runtime_target_triple
        if cross_cc:
            cc_cmd = shlex.split(cross_cc)
        elif shutil.which("zig"):
            cc_cmd = ["zig", "cc"]
            normalized = _zig_target_query(runtime_target_triple)
            if normalized != runtime_target_triple:
                warnings.append(
                    f"Zig target normalized to {normalized} from {runtime_target_triple}."
                )
            target_arg = normalized
        else:
            return _fail(
                "Cross-target extension build requires zig or MOLT_CROSS_CC "
                f"(missing for {runtime_target_triple}).",
                json_output,
                command="extension-build",
            )
        if not cc_cmd:
            return _fail(
                "Compiler command is empty. Set MOLT_CROSS_CC or install zig.",
                json_output,
                command="extension-build",
            )
        cc_cmd.extend(["-target", target_arg])

    dist_name = _normalize_name(str(project_name)).replace("-", "_")
    wheel_version = _wheel_version_token(str(project_version))
    target_triple = manifest_target_triple
    platform_tag = _wheel_token(target_triple)
    python_tag = "py3"
    wheel_name = (
        f"{dist_name}-{wheel_version}-{python_tag}-{abi_tag}-{platform_tag}.whl"
    )
    wheel_path = output_root / wheel_name

    build_env = os.environ.copy()
    # Supply-chain: always set SOURCE_DATE_EPOCH for release builds for reproducibility
    if deterministic or profile == "release":
        build_env.setdefault("SOURCE_DATE_EPOCH", "315532800")

    module_rel = Path(
        *module_parts[:-1],
        module_parts[-1] + _extension_binary_suffix(runtime_target_triple),
    )
    compile_commands: list[list[str]] = []
    link_command: list[str] = []

    with tempfile.TemporaryDirectory(prefix="molt_ext_build_", dir=output_root) as td:
        build_tmp = Path(td)
        object_paths: list[Path] = []
        for idx, source_path in enumerate(source_paths):
            object_path = build_tmp / f"{idx}_{source_path.stem}.o"
            cmd = [*cc_cmd, "-c", str(source_path), "-o", str(object_path)]
            cmd.extend(["-I", str(include_root), "-I", str(project_root)])
            for include_path in include_paths:
                cmd.extend(["-I", str(include_path)])
            if os.name != "nt":
                cmd.append("-fPIC")
            if deterministic:
                prefix = str(project_root)
                cmd.append(f"-ffile-prefix-map={prefix}=.")
                cmd.append(f"-fdebug-prefix-map={prefix}=.")
            cmd.extend(compile_args)
            result = subprocess.run(
                cmd,
                cwd=project_root,
                env=build_env,
                capture_output=True,
                text=True,
                check=False,
            )
            if result.returncode != 0:
                detail = result.stderr.strip() or result.stdout.strip()
                if not detail:
                    detail = f"compiler exited with code {result.returncode}"
                return _fail(
                    f"Failed compiling {source_path.name}: {detail}",
                    json_output,
                    command="extension-build",
                )
            compile_commands.append(cmd)
            object_paths.append(object_path)

        built_extension = build_tmp / module_rel
        built_extension.parent.mkdir(parents=True, exist_ok=True)
        link_command = [*cc_cmd, "-shared"]
        link_command.extend(str(path) for path in object_paths)
        link_command.append(str(runtime_lib))
        link_command.extend(["-o", str(built_extension)])
        link_command.extend(link_args)
        link_result = subprocess.run(
            link_command,
            cwd=project_root,
            env=build_env,
            capture_output=True,
            text=True,
            check=False,
        )
        if link_result.returncode != 0:
            detail = link_result.stderr.strip() or link_result.stdout.strip()
            if not detail:
                detail = f"linker exited with code {link_result.returncode}"
            return _fail(
                f"Failed linking extension module: {detail}",
                json_output,
                command="extension-build",
            )

        if not built_extension.exists():
            return _fail(
                "Link succeeded but extension artifact is missing.",
                json_output,
                command="extension-build",
            )

        extension_bytes = built_extension.read_bytes()
        extension_archive_path = module_rel.as_posix()
        manifest_payload: dict[str, Any] = {
            "schema_version": 1,
            "name": str(project_name),
            "version": str(project_version),
            "module": ".".join(module_parts),
            "sources": [str(path) for path in source_paths],
            "molt_c_api_version": abi_version,
            "abi_tag": abi_tag,
            "python_tag": python_tag,
            "target_triple": target_triple,
            "platform_tag": platform_tag,
            "capabilities": capabilities_list,
            "capability_profiles": capability_profiles,
            "deterministic": deterministic,
            "determinism": determinism_mode,
            "effects": effects,
            "wheel": wheel_name,
            "extension": extension_archive_path,
            "build": {
                "compiler": cc_cmd,
                "compiler_target": runtime_target_triple or "native",
                "runtime_lib": str(runtime_lib),
                "include_dirs": [str(include_root), str(project_root)]
                + [str(path) for path in include_paths],
                "extra_compile_args": compile_args,
                "extra_link_args": link_args,
            },
        }
        manifest_bytes = (
            json.dumps(manifest_payload, sort_keys=True, indent=2).encode("utf-8")
            + b"\n"
        )

        dist_info = f"{dist_name}-{wheel_version}.dist-info"
        wheel_metadata = "\n".join(
            [
                "Wheel-Version: 1.0",
                "Generator: molt extension build",
                "Root-Is-Purelib: false",
                f"Tag: {python_tag}-{abi_tag}-{platform_tag}",
                "",
            ]
        ).encode("utf-8")
        package_metadata = "\n".join(
            [
                "Metadata-Version: 2.1",
                f"Name: {project_name}",
                f"Version: {project_version}",
                "Summary: Molt C extension package",
                "",
            ]
        ).encode("utf-8")

        wheel_entries: list[tuple[str, bytes]] = [
            (extension_archive_path, extension_bytes),
            ("extension_manifest.json", manifest_bytes),
            (f"{dist_info}/WHEEL", wheel_metadata),
            (f"{dist_info}/METADATA", package_metadata),
        ]
        record_path = f"{dist_info}/RECORD"
        record_lines = [_wheel_record_line(path, data) for path, data in wheel_entries]
        record_lines.append(f"{record_path},,")
        record_bytes = ("\n".join(record_lines) + "\n").encode("utf-8")

        with zipfile.ZipFile(wheel_path, "w") as zf:
            for path, data in wheel_entries:
                _write_zip_member(zf, path, data)
            _write_zip_member(zf, record_path, record_bytes)

    wheel_sha = _sha256_file(wheel_path)
    extension_sha = hashlib.sha256(extension_bytes).hexdigest()
    sidecar_payload = dict(manifest_payload)
    sidecar_payload["wheel_sha256"] = wheel_sha
    sidecar_payload["extension_sha256"] = extension_sha
    if deterministic:
        sidecar_payload["generated_at_utc"] = "1970-01-01T00:00:00Z"
    else:
        sidecar_payload["generated_at_utc"] = (
            dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat()
        )
    manifest_path = output_root / "extension_manifest.json"
    manifest_path.write_text(
        json.dumps(sidecar_payload, sort_keys=True, indent=2) + "\n"
    )

    if json_output:
        payload = _json_payload(
            "extension-build",
            "ok",
            data={
                "project": str(project_root),
                "wheel": str(wheel_path),
                "manifest": str(manifest_path),
                "module": ".".join(module_parts),
                "molt_c_api_version": abi_version,
                "abi_tag": abi_tag,
                "target_triple": target_triple,
                "build_target": runtime_target_triple or "native",
                "platform_tag": platform_tag,
                "deterministic": deterministic,
                "determinism": determinism_mode,
                "capabilities": capabilities_list,
                "capability_profiles": capability_profiles,
                "wheel_sha256": wheel_sha,
                "extension_sha256": extension_sha,
            },
            warnings=warnings,
        )
        _emit_json(payload, json_output=True)
    else:
        print(f"Built extension wheel: {wheel_path}")
        print(f"Wrote extension manifest: {manifest_path}")
        if verbose:
            print(f"Target triple: {target_triple}")
            print(f"Build target: {runtime_target_triple or 'native'}")
            print(f"Molt C API version: {abi_version}")
            print(f"Capabilities: {json.dumps(capabilities_list)}")
            print(f"Compile steps: {len(compile_commands)}")
    return 0


def extension_audit(
    path: str,
    require_capabilities: bool = False,
    require_abi: str | None = None,
    require_checksum: bool = False,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    target = Path(path).expanduser()
    if not target.is_absolute():
        target = (Path.cwd() / target).absolute()

    if (
        require_abi is not None
        and _MOLT_C_API_VERSION_RE.match(require_abi.strip()) is None
    ):
        return _fail(
            "Invalid --require-abi value. Expected MAJOR[.MINOR[.PATCH]].",
            json_output,
            command="extension-audit",
        )
    required_abi = require_abi.strip() if require_abi is not None else None

    errors: list[str] = []
    warnings: list[str] = []
    manifest: dict[str, Any] | None = None
    manifest_source = ""
    manifest_dir = target.parent if target.is_file() else target
    wheel_path: Path | None = None

    def load_manifest_json(source_path: Path) -> dict[str, Any] | None:
        try:
            loaded = json.loads(source_path.read_text())
        except (OSError, json.JSONDecodeError) as exc:
            errors.append(f"Failed to read extension manifest {source_path}: {exc}")
            return None
        if not isinstance(loaded, dict):
            errors.append(f"Extension manifest must be a JSON object: {source_path}")
            return None
        return loaded

    if target.is_dir():
        manifest_path = target / "extension_manifest.json"
        if not manifest_path.exists():
            return _fail(
                f"Missing extension manifest: {manifest_path}",
                json_output,
                command="extension-audit",
            )
        manifest = load_manifest_json(manifest_path)
        manifest_source = str(manifest_path)
        manifest_dir = manifest_path.parent
    elif target.is_file() and target.suffix == ".whl":
        wheel_path = target
        sibling_manifest = target.parent / "extension_manifest.json"
        if sibling_manifest.exists():
            manifest = load_manifest_json(sibling_manifest)
            manifest_source = str(sibling_manifest)
            manifest_dir = sibling_manifest.parent
        else:
            try:
                with zipfile.ZipFile(target) as zf:
                    manifest_bytes = zf.read("extension_manifest.json")
            except KeyError:
                return _fail(
                    "extension_manifest.json not found next to wheel or inside wheel.",
                    json_output,
                    command="extension-audit",
                )
            except (OSError, zipfile.BadZipFile) as exc:
                return _fail(
                    f"Failed to inspect wheel {target}: {exc}",
                    json_output,
                    command="extension-audit",
                )
            try:
                decoded = json.loads(manifest_bytes.decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError) as exc:
                return _fail(
                    f"Invalid embedded extension_manifest.json: {exc}",
                    json_output,
                    command="extension-audit",
                )
            if not isinstance(decoded, dict):
                return _fail(
                    "Embedded extension_manifest.json must be a JSON object.",
                    json_output,
                    command="extension-audit",
                )
            manifest = decoded
            manifest_source = f"{target}!/extension_manifest.json"
            manifest_dir = target.parent
    elif target.is_file() and target.suffix == ".json":
        manifest = load_manifest_json(target)
        manifest_source = str(target)
        manifest_dir = target.parent
    else:
        return _fail(
            f"Unsupported audit path: {target}",
            json_output,
            command="extension-audit",
        )

    if manifest is None:
        return _fail(
            "Failed to load extension manifest.",
            json_output,
            command="extension-audit",
        )

    validation = _validate_extension_manifest(
        manifest,
        manifest_dir=manifest_dir,
        wheel_path=wheel_path,
        require_capabilities=require_capabilities,
        required_abi=required_abi,
        require_checksum=require_checksum,
        warn_missing_checksum=not require_checksum,
    )
    errors.extend(validation.errors)
    warnings.extend(validation.warnings)
    wheel_path = validation.wheel_path
    manifest_abi = validation.abi_version
    manifest_abi_tag = validation.abi_tag
    manifest_capabilities = validation.capabilities
    wheel_tags = validation.wheel_tags

    status = "ok" if not errors else "error"
    if json_output:
        payload = _json_payload(
            "extension-audit",
            status,
            data={
                "path": str(target),
                "manifest_source": manifest_source,
                "wheel": str(wheel_path) if wheel_path is not None else None,
                "molt_c_api_version": manifest_abi,
                "abi_tag": manifest_abi_tag,
                "capabilities": manifest_capabilities,
                "require_capabilities": require_capabilities,
                "require_abi": required_abi,
                "require_checksum": require_checksum,
                "wheel_tags": {
                    "python": wheel_tags[0],
                    "abi": wheel_tags[1],
                    "platform": wheel_tags[2],
                }
                if wheel_tags is not None
                else None,
            },
            warnings=warnings,
            errors=errors,
        )
        _emit_json(payload, json_output=True)
    else:
        if errors:
            for err in errors:
                print(f"ERROR: {err}")
        else:
            print(f"Extension audit passed: {target}")
        if verbose:
            print(f"Manifest source: {manifest_source}")
            if wheel_path is not None:
                print(f"Wheel: {wheel_path}")
            for warning in warnings:
                print(f"WARN: {warning}")
    return 0 if not errors else 1


def _resolve_extension_manifest_for_verify(
    wheel_path: Path,
) -> tuple[Path | None, tempfile.TemporaryDirectory[str] | None, str | None]:
    sibling_manifest = wheel_path.parent / "extension_manifest.json"
    if sibling_manifest.exists():
        return sibling_manifest, None, None
    try:
        with zipfile.ZipFile(wheel_path) as zf:
            manifest_bytes = zf.read("extension_manifest.json")
    except KeyError:
        return (
            None,
            None,
            "extension_manifest.json not found next to wheel or inside wheel.",
        )
    except (OSError, zipfile.BadZipFile) as exc:
        return None, None, f"Failed to inspect wheel {wheel_path}: {exc}"
    try:
        decoded = json.loads(manifest_bytes.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        return None, None, f"Invalid embedded extension_manifest.json: {exc}"
    if not isinstance(decoded, dict):
        return None, None, "Embedded extension_manifest.json must be a JSON object."
    tmpdir = tempfile.TemporaryDirectory(prefix="molt_ext_manifest_")
    manifest_path = Path(tmpdir.name) / "extension_manifest.json"
    manifest_path.write_text(json.dumps(decoded, sort_keys=True, indent=2) + "\n")
    return manifest_path, tmpdir, None


def _resolve_sidecar_path(output_path: Path, override: str | None, suffix: str) -> Path:
    if override:
        path = Path(override).expanduser()
        if not path.is_absolute():
            path = (output_path.parent / path).absolute()
        return path
    return output_path.with_name(output_path.stem + suffix)


def _is_remote_registry(registry: str) -> bool:
    scheme = urllib.parse.urlparse(registry).scheme.lower()
    return scheme in REMOTE_REGISTRY_SCHEMES


def _validate_registry_url(registry: str) -> str | None:
    parsed = urllib.parse.urlparse(registry)
    if parsed.scheme.lower() not in REMOTE_REGISTRY_SCHEMES:
        return f"Unsupported registry scheme: {parsed.scheme or 'none'}"
    if not parsed.netloc:
        return "Registry URL is missing a host"
    if parsed.username or parsed.password:
        return (
            "Registry URL must not include credentials "
            "(use --registry-token or --registry-user/--registry-password)"
        )
    return None


def _read_secret_value(
    value: str | None, *, env_name: str, label: str, use_env: bool = True
) -> tuple[str | None, str | None]:
    source = None
    if value is None and use_env:
        env_val = os.environ.get(env_name)
        if env_val is not None:
            value = env_val
            source = "env"
    else:
        source = "arg"
    if value is None:
        return None, None
    if value.startswith("@"):
        secret_path = Path(value[1:]).expanduser()
        if not secret_path.exists():
            raise RuntimeError(f"{label} file not found: {secret_path}")
        value = secret_path.read_text()
        source = "file"
    value = value.strip()
    if not value:
        raise RuntimeError(f"{label} is empty")
    return value, source


def _resolve_registry_auth(
    registry_token: str | None,
    registry_user: str | None,
    registry_password: str | None,
) -> tuple[dict[str, str], dict[str, str]]:
    explicit_token = registry_token is not None
    explicit_user = registry_user is not None or registry_password is not None
    if explicit_token and explicit_user:
        raise RuntimeError(
            "Use --registry-token or --registry-user/--registry-password, not both."
        )
    token: str | None = None
    token_source: str | None = None
    if explicit_token:
        token, token_source = _read_secret_value(
            registry_token,
            env_name="MOLT_REGISTRY_TOKEN",
            label="Registry token",
            use_env=False,
        )
    elif not explicit_user:
        token, token_source = _read_secret_value(
            None,
            env_name="MOLT_REGISTRY_TOKEN",
            label="Registry token",
            use_env=True,
        )
    user = None
    user_source = None
    password = None
    password_source = None
    if token is None:
        user = registry_user
        user_source = "arg" if registry_user is not None else None
        if user is None:
            env_user = os.environ.get("MOLT_REGISTRY_USER")
            if env_user is not None:
                user = env_user
                user_source = "env"
        password, password_source = _read_secret_value(
            registry_password,
            env_name="MOLT_REGISTRY_PASSWORD",
            label="Registry password",
            use_env=registry_password is None,
        )
    if user and not password:
        raise RuntimeError("Registry password is required when using --registry-user.")
    if password and not user:
        raise RuntimeError("Registry user is required when using --registry-password.")
    headers: dict[str, str] = {}
    auth_info = {"mode": "none", "source": "none"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
        auth_info["mode"] = "bearer"
        auth_info["source"] = token_source or "unknown"
    elif user:
        credential = f"{user}:{password}"
        encoded = base64.b64encode(credential.encode("utf-8")).decode("ascii")
        headers["Authorization"] = f"Basic {encoded}"
        auth_info["mode"] = "basic"
        sources = {
            source for source in (user_source, password_source) if source is not None
        }
        if len(sources) == 1:
            auth_info["source"] = sources.pop()
        elif len(sources) > 1:
            auth_info["source"] = "mixed"
        else:
            auth_info["source"] = "unknown"
    return headers, auth_info


def _resolve_registry_timeout(value: float | None) -> float:
    timeout = value
    if timeout is None:
        env_val = os.environ.get("MOLT_REGISTRY_TIMEOUT")
        if env_val:
            try:
                timeout = float(env_val)
            except ValueError as exc:
                raise RuntimeError(
                    f"Invalid MOLT_REGISTRY_TIMEOUT value: {env_val}"
                ) from exc
    if timeout is None:
        timeout = 30.0
    if timeout <= 0:
        raise RuntimeError("Registry timeout must be greater than zero.")
    return timeout


def _remote_registry_destination(registry_url: str, filename: str) -> str:
    parsed = urllib.parse.urlparse(registry_url)
    path = parsed.path or ""
    if not path or path.endswith("/"):
        base_path = path or "/"
        if not base_path.endswith("/"):
            base_path += "/"
        dest_path = posixpath.join(base_path, filename)
    else:
        dest_path = path
    return urllib.parse.urlunparse(parsed._replace(path=dest_path))


def _remote_sidecar_url(dest_url: str, suffix: str) -> str:
    parsed = urllib.parse.urlparse(dest_url)
    path = parsed.path
    if not path:
        raise RuntimeError("Remote destination URL is missing a path")
    dir_name, file_name = posixpath.split(path)
    stem = Path(file_name).stem
    sidecar_name = f"{stem}{suffix}"
    if dir_name and not dir_name.endswith("/"):
        sidecar_path = posixpath.join(dir_name, sidecar_name)
    elif dir_name:
        sidecar_path = f"{dir_name}{sidecar_name}"
    else:
        sidecar_path = f"/{sidecar_name}"
    return urllib.parse.urlunparse(parsed._replace(path=sidecar_path))


def _registry_content_type(path: Path) -> str:
    suffix = path.suffix.lower()
    if suffix in {".moltpkg", ".whl"}:
        return "application/zip"
    if suffix == ".json":
        return "application/json"
    return "application/octet-stream"


def _upload_registry_file(
    source: Path,
    dest_url: str,
    headers: dict[str, str],
    timeout: float,
) -> dict[str, Any]:
    parsed = urllib.parse.urlparse(dest_url)
    scheme = parsed.scheme.lower()
    host = parsed.hostname
    if not host:
        raise RuntimeError(f"Invalid registry URL: {dest_url}")
    if scheme not in REMOTE_REGISTRY_SCHEMES:
        raise RuntimeError(f"Unsupported registry scheme: {scheme}")
    port = parsed.port
    path = parsed.path or "/"
    if parsed.params:
        path = f"{path};{parsed.params}"
    if parsed.query:
        path = f"{path}?{parsed.query}"
    conn_cls: type[http.client.HTTPConnection]
    if scheme == "https":
        conn_cls = http.client.HTTPSConnection
    else:
        conn_cls = http.client.HTTPConnection
    content_length = source.stat().st_size
    upload_headers = {
        "Content-Type": _registry_content_type(source),
        "Content-Length": str(content_length),
        "User-Agent": f"molt/{_compiler_metadata()[0] or 'unknown'}",
        "X-Molt-Upload-Id": str(uuid.uuid4()),
    }
    upload_headers.update(headers)
    conn = conn_cls(host, port, timeout=timeout)
    try:
        conn.putrequest("PUT", path)
        for key, value in upload_headers.items():
            conn.putheader(key, value)
        conn.endheaders()
        with source.open("rb") as handle:
            while True:
                chunk = handle.read(1024 * 64)
                if not chunk:
                    break
                conn.send(chunk)
        response = conn.getresponse()
        body = response.read()
    finally:
        conn.close()
    status = response.status
    if status < 200 or status >= 300:
        detail = body.decode("utf-8", errors="replace").strip()
        if detail:
            detail = f" {detail}"
        raise RuntimeError(
            f"Registry upload failed ({status} {response.reason}).{detail}"
        )
    return {
        "status": status,
        "reason": response.reason,
        "bytes": content_length,
        "etag": response.getheader("ETag"),
    }


def package(
    artifact: str,
    manifest_path: str,
    output: str | None,
    json_output: bool = False,
    verbose: bool = False,
    deterministic: bool = True,
    deterministic_warn: bool = False,
    capabilities: CapabilityInput | None = None,
    sbom: bool = True,
    sbom_output: str | None = None,
    sbom_format: str = "cyclonedx",
    signature: str | None = None,
    signature_output: str | None = None,
    sign: bool = False,
    signer: str | None = None,
    signing_key: str | None = None,
    signing_identity: str | None = None,
) -> int:
    artifact_path = Path(artifact)
    if not artifact_path.exists():
        return _fail(
            f"Artifact not found: {artifact_path}",
            json_output,
            command="package",
        )
    manifest_file = Path(manifest_path)
    manifest = _load_manifest(manifest_file)
    if manifest is None:
        return _fail(
            f"Failed to load manifest: {manifest_file}",
            json_output,
            command="package",
        )
    errors = _manifest_errors(manifest)
    if errors:
        return _fail(
            "Manifest errors: " + ", ".join(errors),
            json_output,
            command="package",
        )
    if deterministic and manifest.get("deterministic") is not True:
        return _fail(
            "Manifest is not deterministic.",
            json_output,
            command="package",
        )

    warnings: list[str] = []
    project_root = _find_project_root(manifest_file.resolve())
    lock_error = _check_lockfiles(
        project_root,
        json_output,
        warnings,
        deterministic,
        deterministic_warn,
        "package",
    )
    if lock_error is not None:
        return lock_error
    capabilities_list = None
    capability_profiles: list[str] = []
    capability_manifest: CapabilityManifest | None = None
    if capabilities is not None:
        spec = _parse_capabilities_spec(capabilities)
        if spec.errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(spec.errors),
                json_output,
                command="package",
            )
        capabilities_list = spec.capabilities
        capability_profiles = spec.profiles
        capability_manifest = spec.manifest
    if capabilities_list is not None:
        required = manifest.get("capabilities", [])
        pkg_name = manifest.get("name")
        allowlist = _allowed_capabilities_for_package(
            capabilities_list, capability_manifest, pkg_name
        )
        missing = [cap for cap in required if cap not in allowlist]
        if missing:
            return _fail(
                "Capabilities missing from allowlist: " + ", ".join(missing),
                json_output,
                command="package",
            )
        required_effects = _normalize_effects(manifest.get("effects"))
        allowed_effects = _allowed_effects_for_package(capability_manifest, pkg_name)
        if allowed_effects is not None:
            missing_effects = [
                effect for effect in required_effects if effect not in allowed_effects
            ]
            if missing_effects:
                return _fail(
                    "Effects missing from allowlist: " + ", ".join(missing_effects),
                    json_output,
                    command="package",
                )

    if signature and sign:
        return _fail(
            "Use --signature or --sign, not both.",
            json_output,
            command="package",
        )
    if sign and manifest.get("deterministic") is True:
        warnings.append("Signing may introduce non-determinism in packaged outputs.")

    tlog_upload = os.environ.get("MOLT_COSIGN_TLOG", "").lower() in {"1", "true", "yes"}
    signer_meta: dict[str, Any] | None = None
    signer_selected: str | None = None
    if sign:
        try:
            signer_meta, signer_selected = _sign_artifact(
                artifact_path=artifact_path,
                sign=sign,
                signer=signer,
                signing_key=signing_key,
                signing_identity=signing_identity,
                tlog_upload=tlog_upload,
            )
        except RuntimeError as exc:
            return _fail(str(exc), json_output, command="package")

    checksum = _sha256_file(artifact_path)
    manifest = dict(manifest)
    manifest["checksum"] = checksum
    name = manifest.get("name", "molt_pkg")
    version = manifest.get("version", "0.0.0")
    target = manifest.get("target", "unknown")

    if output:
        output_path = Path(output)
    else:
        output_path = Path("dist") / f"{name}-{version}-{target}.moltpkg"
    output_path.parent.mkdir(parents=True, exist_ok=True)

    signature_source = Path(signature).expanduser() if signature else None
    signature_bytes: bytes | None = None
    signature_checksum: str | None = None
    signature_path: Path | None = None
    if signature_source is not None:
        if not signature_source.exists():
            return _fail(
                f"Signature not found: {signature_source}",
                json_output,
                command="package",
            )
        signature_bytes = signature_source.read_bytes()
        signature_checksum = hashlib.sha256(signature_bytes).hexdigest()
        signature_path = _resolve_sidecar_path(output_path, signature_output, ".sig")
    elif signer_meta is not None:
        sig_value = (
            signer_meta.get("signature", {}).get("value")
            if isinstance(signer_meta.get("signature"), dict)
            else None
        )
        if isinstance(sig_value, str) and sig_value:
            signature_bytes = sig_value.encode("utf-8")
            signature_checksum = hashlib.sha256(signature_bytes).hexdigest()
            signature_path = _resolve_sidecar_path(
                output_path, signature_output, ".sig"
            )

    sbom_bytes: bytes | None = None
    sbom_path: Path | None = None
    if sbom:
        project_root = _find_project_root(manifest_file.resolve())
        sbom_path = _resolve_sidecar_path(output_path, sbom_output, ".sbom.json")
        sbom_data, sbom_warnings = _build_sbom(
            manifest=manifest,
            artifact_path=artifact_path,
            checksum=checksum,
            project_root=project_root,
            format_name=sbom_format,
        )
        warnings.extend(sbom_warnings)
        sbom_bytes = (
            json.dumps(sbom_data, sort_keys=True, indent=2).encode("utf-8") + b"\n"
        )

    signature_meta_path = _resolve_sidecar_path(output_path, None, ".sig.json")
    signature_meta = _signature_metadata(
        artifact_path=artifact_path,
        checksum=checksum,
        signer_meta=signer_meta,
        signer=signer_selected,
        signature_name=signature_path.name if signature_path is not None else None,
        signature_checksum=signature_checksum,
    )
    signature_meta_bytes = (
        json.dumps(signature_meta, sort_keys=True, indent=2).encode("utf-8") + b"\n"
    )

    artifact_bytes = artifact_path.read_bytes()
    manifest_bytes = (
        json.dumps(manifest, sort_keys=True, indent=2).encode("utf-8") + b"\n"
    )
    with zipfile.ZipFile(output_path, "w") as zf:
        _write_zip_member(zf, "manifest.json", manifest_bytes)
        _write_zip_member(zf, f"artifact/{artifact_path.name}", artifact_bytes)
        if sbom_bytes is not None:
            _write_zip_member(zf, "sbom.json", sbom_bytes)
        _write_zip_member(zf, "signature.json", signature_meta_bytes)
        if signature_bytes is not None and signature_path is not None:
            _write_zip_member(zf, f"signature/{signature_path.name}", signature_bytes)

    if sbom_bytes is not None and sbom_path is not None:
        sbom_path.parent.mkdir(parents=True, exist_ok=True)
        sbom_path.write_bytes(sbom_bytes)
    signature_meta_path.parent.mkdir(parents=True, exist_ok=True)
    signature_meta_path.write_bytes(signature_meta_bytes)
    if signature_bytes is not None and signature_path is not None:
        signature_path.parent.mkdir(parents=True, exist_ok=True)
        signature_path.write_bytes(signature_bytes)

    if json_output:
        payload = _json_payload(
            "package",
            "ok",
            data={
                "output": str(output_path),
                "checksum": checksum,
                "deterministic": deterministic,
                "capabilities": capabilities_list,
                "capability_profiles": capability_profiles,
                "sbom": str(sbom_path) if sbom_path is not None else None,
                "sbom_format": sbom_format if sbom else None,
                "signature_metadata": str(signature_meta_path),
                "signature": str(signature_path)
                if signature_path is not None
                else None,
                "signed": signer_meta is not None or signature_path is not None,
                "signer": signer_selected,
            },
            warnings=warnings,
        )
        _emit_json(payload, json_output=True)
    else:
        print(f"Packaged {output_path}")
        if verbose:
            print(f"Checksum: {checksum}")
            if sbom_path is not None:
                print(f"SBOM: {sbom_path}")
            print(f"Signature metadata: {signature_meta_path}")
            if signature_path is not None:
                print(f"Signature: {signature_path}")
            if signer_meta is not None:
                print(f"Signed with: {signer_selected}")
            for warning in warnings:
                print(f"WARN: {warning}")
    return 0


def publish(
    package_path: str,
    registry: str,
    dry_run: bool,
    json_output: bool = False,
    verbose: bool = False,
    deterministic: bool = True,
    deterministic_warn: bool = False,
    capabilities: CapabilityInput | None = None,
    require_signature: bool = False,
    verify_signature: bool = False,
    trusted_signers: str | None = None,
    signer: str | None = None,
    signing_key: str | None = None,
    registry_token: str | None = None,
    registry_user: str | None = None,
    registry_password: str | None = None,
    registry_timeout: float | None = None,
) -> int:
    source = Path(package_path)
    if not source.exists():
        return _fail(
            f"Package not found: {source}",
            json_output,
            command="publish",
        )
    warnings: list[str] = []
    project_root = _find_project_root(source.resolve())
    lock_error = _check_lockfiles(
        project_root,
        json_output,
        warnings,
        deterministic,
        deterministic_warn,
        "publish",
    )
    if lock_error is not None:
        return lock_error
    extension_manifest_tmpdir: tempfile.TemporaryDirectory[str] | None = None
    is_extension_wheel = source.suffix.lower() == ".whl"
    if verify_signature:
        require_signature = True
    should_verify = (
        deterministic
        or require_signature
        or verify_signature
        or trusted_signers is not None
    )

    def run_publish_verify(*verify_args: Any) -> tuple[int, str]:
        if not json_output:
            return verify(*verify_args), ""
        captured = io.StringIO()
        with redirect_stdout(captured), redirect_stderr(captured):
            code = verify(*verify_args)
        return code, captured.getvalue()

    if is_extension_wheel:
        manifest_path, extension_manifest_tmpdir, manifest_error = (
            _resolve_extension_manifest_for_verify(source)
        )
        if manifest_error is not None:
            if extension_manifest_tmpdir is not None:
                extension_manifest_tmpdir.cleanup()
            return _fail(manifest_error, json_output, command="publish")
        if manifest_path is None:
            if extension_manifest_tmpdir is not None:
                extension_manifest_tmpdir.cleanup()
            return _fail(
                "Failed to resolve extension manifest for wheel verification.",
                json_output,
                command="publish",
            )
        verify_code, verify_output = run_publish_verify(
            None,  # package_path
            str(manifest_path),  # manifest_path
            str(source),  # artifact_path
            True,  # require_checksum
            False,  # json_output
            verbose,
            deterministic,
            capabilities,
            require_signature,
            verify_signature,
            trusted_signers,
            signer,
            signing_key,
            True,  # require_extension_capabilities
            None,  # require_extension_abi
            True,  # extension_metadata
        )
        if verify_code != 0:
            if extension_manifest_tmpdir is not None:
                extension_manifest_tmpdir.cleanup()
            if json_output:
                verify_msg = (
                    verify_output.strip().splitlines()[-1]
                    if verify_output.strip()
                    else "extension publish verification failed"
                )
                return _fail(
                    f"Extension publish verification failed: {verify_msg}",
                    json_output,
                    command="publish",
                )
            return verify_code
    elif should_verify:
        verify_code, verify_output = run_publish_verify(
            package_path,
            None,
            None,
            True,
            False,
            verbose,
            deterministic,
            capabilities,
            require_signature,
            verify_signature,
            trusted_signers,
            signer,
            signing_key,
        )
        if verify_code != 0:
            if extension_manifest_tmpdir is not None:
                extension_manifest_tmpdir.cleanup()
            if json_output:
                verify_msg = (
                    verify_output.strip().splitlines()[-1]
                    if verify_output.strip()
                    else "publish verification failed"
                )
                return _fail(
                    f"Publish verification failed: {verify_msg}",
                    json_output,
                    command="publish",
                )
            return verify_code
    is_remote = _is_remote_registry(registry)
    sidecars: list[dict[str, str]] = []
    uploads: list[dict[str, Any]] = []
    auth_info = {"mode": "none", "source": "none"}
    if is_remote:
        url_error = _validate_registry_url(registry)
        if url_error:
            if extension_manifest_tmpdir is not None:
                extension_manifest_tmpdir.cleanup()
            return _fail(url_error, json_output, command="publish")
        try:
            headers, auth_info = _resolve_registry_auth(
                registry_token, registry_user, registry_password
            )
            timeout = _resolve_registry_timeout(registry_timeout)
        except RuntimeError as exc:
            if extension_manifest_tmpdir is not None:
                extension_manifest_tmpdir.cleanup()
            return _fail(str(exc), json_output, command="publish")
        dest = _remote_registry_destination(registry, source.name)
        upload_plan: list[tuple[Path, str]] = [(source, dest)]
        for suffix in (".sbom.json", ".sig.json", ".sig"):
            sidecar_src = source.with_name(source.stem + suffix)
            if not sidecar_src.exists():
                continue
            sidecar_dest = _remote_sidecar_url(dest, suffix)
            sidecars.append({"source": str(sidecar_src), "dest": sidecar_dest})
            upload_plan.append((sidecar_src, sidecar_dest))
        if not dry_run:
            for upload_src, upload_dest in upload_plan:
                try:
                    result = _upload_registry_file(
                        upload_src, upload_dest, headers, timeout
                    )
                except RuntimeError as exc:
                    if extension_manifest_tmpdir is not None:
                        extension_manifest_tmpdir.cleanup()
                    return _fail(str(exc), json_output, command="publish")
                uploads.append(
                    {
                        "source": str(upload_src),
                        "dest": upload_dest,
                        **result,
                    }
                )
    else:
        registry_path = Path(registry)
        if registry_path.exists() and registry_path.is_dir():
            dest = registry_path / source.name
        elif registry.endswith(os.sep):
            dest = registry_path / source.name
        else:
            dest = registry_path
        if not dry_run:
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, dest)
        for suffix in (".sbom.json", ".sig.json", ".sig"):
            sidecar_src = source.with_name(source.stem + suffix)
            if not sidecar_src.exists():
                continue
            sidecar_dest = dest.with_name(dest.stem + suffix)
            sidecars.append({"source": str(sidecar_src), "dest": str(sidecar_dest)})
            if not dry_run:
                sidecar_dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(sidecar_src, sidecar_dest)
    if json_output:
        payload = _json_payload(
            "publish",
            "ok",
            data={
                "source": str(source),
                "dest": str(dest),
                "dry_run": dry_run,
                "deterministic": deterministic,
                "sidecars": sidecars,
                "remote": is_remote,
                "extension_wheel": is_extension_wheel,
                "auth": auth_info,
                "uploads": uploads,
            },
            warnings=warnings,
        )
        _emit_json(payload, json_output=True)
    else:
        action = "Would publish" if dry_run else "Published"
        print(f"{action} {source} -> {dest}")
        if sidecars and verbose:
            for entry in sidecars:
                print(f"{action} {entry['source']} -> {entry['dest']}")
        if verbose:
            registry_label = registry
            if is_remote:
                print(f"Registry: {registry_label} (remote)")
                print(f"Auth: {auth_info['mode']}")
            else:
                print(f"Registry: {registry_label}")
    if extension_manifest_tmpdir is not None:
        extension_manifest_tmpdir.cleanup()
    return 0


def verify(
    package_path: str | None,
    manifest_path: str | None,
    artifact_path: str | None,
    require_checksum: bool,
    json_output: bool = False,
    verbose: bool = False,
    require_deterministic: bool = False,
    capabilities: CapabilityInput | None = None,
    require_signature: bool = False,
    verify_signature: bool = False,
    trusted_signers: str | None = None,
    signer: str | None = None,
    signing_key: str | None = None,
    require_extension_capabilities: bool = False,
    require_extension_abi: str | None = None,
    extension_metadata: bool | None = None,
) -> int:
    errors: list[str] = []
    warnings: list[str] = []
    manifest: dict[str, Any] | None = None
    manifest_file: Path | None = None
    artifact_name = None
    artifact_bytes = None
    artifact_file: Path | None = None
    checksum: str | None = None
    extension_mode = False
    extension_validation: ExtensionManifestValidation | None = None
    capabilities_list = None
    capability_profiles: list[str] = []
    capability_manifest: CapabilityManifest | None = None
    signature_meta: dict[str, Any] | None = None
    signature_bytes: bytes | None = None
    signature_name: str | None = None
    trust_policy: TrustPolicy | None = None

    if capabilities is not None:
        spec = _parse_capabilities_spec(capabilities)
        if spec.errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(spec.errors),
                json_output,
                command="verify",
            )
        capabilities_list = spec.capabilities
        capability_profiles = spec.profiles
        capability_manifest = spec.manifest
    if trusted_signers:
        try:
            trust_policy = _load_trust_policy(Path(trusted_signers))
        except (OSError, json.JSONDecodeError, tomllib.TOMLDecodeError) as exc:
            return _fail(
                f"Failed to load trust policy: {exc}",
                json_output,
                command="verify",
            )
    required_extension_abi: str | None = None
    if require_extension_abi is not None:
        normalized = require_extension_abi.strip()
        if _MOLT_C_API_VERSION_RE.match(normalized) is None:
            return _fail(
                "Invalid --require-extension-abi value. Expected MAJOR[.MINOR[.PATCH]].",
                json_output,
                command="verify",
            )
        required_extension_abi = normalized

    if package_path:
        pkg_path = Path(package_path)
        if not pkg_path.exists():
            return _fail(
                f"Package not found: {pkg_path}",
                json_output,
                command="verify",
            )
        try:
            with zipfile.ZipFile(pkg_path) as zf:
                try:
                    manifest_bytes = zf.read("manifest.json")
                except KeyError:
                    errors.append("manifest.json not found in package")
                else:
                    manifest = json.loads(manifest_bytes.decode("utf-8"))
                try:
                    sig_meta_bytes = zf.read("signature.json")
                except KeyError:
                    signature_meta = None
                else:
                    signature_meta = json.loads(sig_meta_bytes.decode("utf-8"))
                artifact_entries = [
                    name for name in zf.namelist() if name.startswith("artifact/")
                ]
                if len(artifact_entries) == 1:
                    artifact_name = artifact_entries[0]
                    artifact_bytes = zf.read(artifact_name)
                elif not artifact_entries:
                    errors.append("artifact/* not found in package")
                else:
                    errors.append("multiple artifact entries in package")
                signature_entries = [
                    name for name in zf.namelist() if name.startswith("signature/")
                ]
                if len(signature_entries) == 1:
                    signature_name = signature_entries[0].split("/", 1)[1]
                    signature_bytes = zf.read(signature_entries[0])
                elif len(signature_entries) > 1:
                    errors.append("multiple signature entries in package")
        except (OSError, zipfile.BadZipFile) as exc:
            return _fail(
                f"Failed to read package: {exc}",
                json_output,
                command="verify",
            )
        if signature_meta is None:
            sidecar = pkg_path.with_name(pkg_path.stem + ".sig.json")
            if sidecar.exists():
                signature_meta = json.loads(sidecar.read_text())
        if signature_bytes is None:
            sidecar_sig = pkg_path.with_name(pkg_path.stem + ".sig")
            if sidecar_sig.exists():
                signature_bytes = sidecar_sig.read_bytes()
                signature_name = sidecar_sig.name
    else:
        if not manifest_path or not artifact_path:
            return _fail(
                "Provide --package or both --manifest and --artifact.",
                json_output,
                command="verify",
            )
        manifest_file = Path(manifest_path)
        manifest = _load_manifest(manifest_file)
        if manifest is None:
            errors.append("failed to load manifest")
        artifact_file = Path(artifact_path)
        if not artifact_file.exists():
            errors.append("artifact not found")
        else:
            artifact_name = artifact_file.name
            artifact_bytes = artifact_file.read_bytes()
        sidecar = artifact_file.with_name(artifact_file.stem + ".sig.json")
        if sidecar.exists():
            signature_meta = json.loads(sidecar.read_text())
        sidecar_sig = artifact_file.with_name(artifact_file.stem + ".sig")
        if sidecar_sig.exists():
            signature_bytes = sidecar_sig.read_bytes()
            signature_name = sidecar_sig.name

    if manifest is not None:
        extension_mode = (
            extension_metadata
            if extension_metadata is not None
            else _is_extension_manifest(manifest)
        )
        if extension_mode:
            if not _is_extension_manifest(manifest):
                errors.append(
                    "Manifest does not match extension metadata schema "
                    "(disable with --no-extension-metadata)."
                )
            else:
                manifest_dir = (
                    manifest_file.parent
                    if manifest_file is not None
                    else (
                        Path(package_path).parent
                        if package_path is not None
                        else Path.cwd()
                    )
                )
                extension_wheel: Path | None = None
                if artifact_file is not None and artifact_file.suffix == ".whl":
                    extension_wheel = artifact_file
                extension_validation = _validate_extension_manifest(
                    manifest,
                    manifest_dir=manifest_dir,
                    wheel_path=extension_wheel,
                    require_capabilities=require_extension_capabilities,
                    required_abi=required_extension_abi,
                    require_checksum=require_checksum,
                    warn_missing_checksum=not require_checksum,
                )
                errors.extend(extension_validation.errors)
                warnings.extend(extension_validation.warnings)
                wheel_checksum = manifest.get("wheel_sha256")
                checksum = wheel_checksum if isinstance(wheel_checksum, str) else None
                if require_deterministic and manifest.get("deterministic") is not True:
                    errors.append("manifest is not deterministic")
                required_caps = extension_validation.capabilities
                if capabilities_list is None and required_caps:
                    errors.append(
                        "capabilities allowlist required; pass --capabilities or set "
                        "tool.molt.capabilities in config"
                    )
                if capabilities_list is not None:
                    pkg_name = manifest.get("name")
                    allowlist = _allowed_capabilities_for_package(
                        capabilities_list, capability_manifest, pkg_name
                    )
                    missing = [cap for cap in required_caps if cap not in allowlist]
                    if missing:
                        errors.append(
                            "capabilities missing from allowlist: " + ", ".join(missing)
                        )
        else:
            errors.extend(_manifest_errors(manifest))
            checksum = manifest.get("checksum")
            if checksum and artifact_bytes is not None:
                actual = hashlib.sha256(artifact_bytes).hexdigest()
                if actual != checksum:
                    errors.append("checksum mismatch")
            elif require_checksum:
                errors.append("checksum missing")
            elif not checksum:
                warnings.append("checksum missing")
            if require_deterministic and manifest.get("deterministic") is not True:
                errors.append("manifest is not deterministic")
            required_caps = manifest.get("capabilities", [])
            if not isinstance(required_caps, list):
                required_caps = []
            required_effects = _normalize_effects(manifest.get("effects"))
            if capabilities_list is None and (required_caps or required_effects):
                errors.append(
                    "capabilities allowlist required; pass --capabilities or set "
                    "tool.molt.capabilities in config"
                )
            if capabilities_list is not None:
                pkg_name = manifest.get("name")
                allowlist = _allowed_capabilities_for_package(
                    capabilities_list, capability_manifest, pkg_name
                )
                missing = [cap for cap in required_caps if cap not in allowlist]
                if missing:
                    errors.append(
                        "capabilities missing from allowlist: " + ", ".join(missing)
                    )
                allowed_effects = _allowed_effects_for_package(
                    capability_manifest, pkg_name
                )
                if allowed_effects is not None:
                    missing_effects = [
                        effect
                        for effect in required_effects
                        if effect not in allowed_effects
                    ]
                    if missing_effects:
                        errors.append(
                            "effects missing from allowlist: "
                            + ", ".join(missing_effects)
                        )

    signature_status = None
    signer_meta: dict[str, Any] | None = None
    if signature_meta and isinstance(signature_meta, dict):
        signature_status = signature_meta.get("status")
        signer_meta_val = signature_meta.get("signer")
        if isinstance(signer_meta_val, dict):
            signer_meta = signer_meta_val
        artifact_meta = signature_meta.get("artifact")
        if isinstance(artifact_meta, dict):
            meta_sha = _normalize_sha256(artifact_meta.get("sha256"))
            if meta_sha and checksum:
                if _normalize_sha256(checksum) != meta_sha:
                    errors.append("signature metadata artifact checksum mismatch")
        signature_file = signature_meta.get("signature_file")
        if isinstance(signature_file, dict) and signature_bytes is not None:
            expected_sig = _normalize_sha256(signature_file.get("sha256"))
            actual_sig = hashlib.sha256(signature_bytes).hexdigest()
            if expected_sig and _normalize_sha256(actual_sig) != expected_sig:
                errors.append("signature file checksum mismatch")

    if verify_signature:
        require_signature = True

    signed = False
    if signature_status == "signed":
        signed = True
    elif signature_status == "unsigned":
        signed = False
    elif signature_name or signature_bytes or signer_meta is not None:
        signed = True

    if require_signature or trust_policy is not None:
        if not signed:
            errors.append("signature required but not present")

    trust_status = None
    if trust_policy is not None and signed:
        signer_name = None
        if signer_meta is not None:
            selected = signer_meta.get("selected")
            if isinstance(selected, str) and selected:
                signer_name = selected
            else:
                tool = signer_meta.get("tool")
                if isinstance(tool, dict):
                    name = tool.get("name")
                    if isinstance(name, str) and name:
                        signer_name = name
        allowed, reason = _trust_policy_allows(signer_name, signer_meta, trust_policy)
        trust_status = "trusted" if allowed else "untrusted"
        if not allowed:
            errors.append(f"signature trust policy failed: {reason}")

    signature_verified = None
    if verify_signature and signed and artifact_bytes is not None:
        key = signing_key or os.environ.get("COSIGN_KEY")
        with tempfile.TemporaryDirectory(prefix="molt_verify_") as tmpdir:
            temp_dir = Path(tmpdir)
            if artifact_file is None:
                filename = Path(artifact_name).name if artifact_name else "artifact.bin"
                artifact_fs_path = temp_dir / filename
                artifact_fs_path.write_bytes(artifact_bytes)
            else:
                artifact_fs_path = artifact_file
            tool = _resolve_signature_tool(
                signer, signer_meta, artifact_fs_path, signature_bytes
            )
            try:
                if tool == "cosign":
                    if signature_bytes is None:
                        raise RuntimeError("cosign signature file is missing")
                    if not key:
                        raise RuntimeError(
                            "cosign verification requires --signing-key or COSIGN_KEY"
                        )
                    _verify_cosign_signature(artifact_fs_path, signature_bytes, key)
                elif tool == "codesign":
                    if not _is_macho(artifact_fs_path):
                        raise RuntimeError(
                            "codesign verification requires a Mach-O artifact"
                        )
                    _verify_codesign_signature(artifact_fs_path)
                else:
                    raise RuntimeError(
                        "unable to resolve signing tool for verification"
                    )
            except RuntimeError as exc:
                signature_verified = False
                errors.append(str(exc))
            else:
                signature_verified = True

    status = "ok" if not errors else "error"
    if json_output:
        data: dict[str, Any] = {
            "artifact": artifact_name,
            "deterministic": require_deterministic,
            "capability_profiles": capability_profiles,
            "signature_status": signature_status
            or ("signed" if signed else "unsigned"),
            "signature_verified": signature_verified,
            "trust_status": trust_status,
        }
        if extension_mode:
            data["extension_metadata"] = True
            data["extension_require_capabilities"] = require_extension_capabilities
            data["extension_require_abi"] = required_extension_abi
            if extension_validation is not None:
                data["extension_wheel"] = (
                    str(extension_validation.wheel_path)
                    if extension_validation.wheel_path is not None
                    else None
                )
                data["extension_abi"] = extension_validation.abi_version
                data["extension_abi_tag"] = extension_validation.abi_tag
                data["extension_capabilities"] = extension_validation.capabilities
                data["extension_wheel_tags"] = (
                    {
                        "python": extension_validation.wheel_tags[0],
                        "abi": extension_validation.wheel_tags[1],
                        "platform": extension_validation.wheel_tags[2],
                    }
                    if extension_validation.wheel_tags is not None
                    else None
                )
        payload = _json_payload(
            "verify",
            status,
            data=data,
            warnings=warnings,
            errors=errors,
        )
        _emit_json(payload, json_output=True)
    else:
        for err in errors:
            print(f"ERROR: {err}")
        for warn in warnings:
            print(f"WARN: {warn}")
        if not errors and verbose:
            print("Verification passed")
    return 0 if not errors else 1


def _summarize_tiers(rows: list[dict[str, Any]]) -> dict[str, int]:
    summary: dict[str, int] = {"Tier A": 0, "Tier B": 0, "Tier C": 0}
    for row in rows:
        tier = row.get("tier")
        if tier in summary:
            summary[tier] += 1
    return summary


def _git_ref_from_source(source: dict[str, Any]) -> tuple[str | None, str | None]:
    for key in ("rev", "revision", "commit", "reference"):
        value = source.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip(), key
    for key in ("tag", "branch"):
        value = source.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip(), key
    return None, None


def _resolve_git_ref(url: str, ref: str) -> tuple[str | None, str | None]:
    try:
        result = subprocess.run(
            ["git", "ls-remote", url, ref],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        return None, f"Failed to resolve git ref {ref}: {exc}"
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip() or "unknown error"
        return None, f"Failed to resolve git ref {ref}: {detail}"
    line = result.stdout.strip().splitlines()[0] if result.stdout.strip() else ""
    if not line:
        return None, f"Failed to resolve git ref {ref}: empty response"
    commit = line.split()[0]
    if not commit:
        return None, f"Failed to resolve git ref {ref}: empty commit"
    return commit, None


def _clone_git_source(
    url: str,
    ref: str,
    dest: Path,
    *,
    subdirectory: str | None = None,
) -> tuple[str, str]:
    tmp_root = dest.parent
    with tempfile.TemporaryDirectory(dir=tmp_root, prefix="git_vendor_") as tmpdir:
        repo_dir = Path(tmpdir) / "repo"
        try:
            clone = subprocess.run(
                [
                    "git",
                    "clone",
                    "--filter=blob:none",
                    "--no-checkout",
                    url,
                    str(repo_dir),
                ],
                capture_output=True,
                text=True,
                check=False,
            )
        except OSError as exc:
            raise RuntimeError(f"Failed to clone git repo {url}: {exc}") from exc
        if clone.returncode != 0:
            detail = (clone.stderr or clone.stdout).strip() or "unknown error"
            raise RuntimeError(f"Failed to clone git repo {url}: {detail}")
        fetch = subprocess.run(
            ["git", "-C", str(repo_dir), "fetch", "--depth", "1", "origin", ref],
            capture_output=True,
            text=True,
            check=False,
        )
        if fetch.returncode != 0:
            detail = (fetch.stderr or fetch.stdout).strip() or "unknown error"
            raise RuntimeError(f"Failed to fetch git ref {ref}: {detail}")
        checkout = subprocess.run(
            ["git", "-C", str(repo_dir), "checkout", "--detach", ref],
            capture_output=True,
            text=True,
            check=False,
        )
        if checkout.returncode != 0:
            detail = (checkout.stderr or checkout.stdout).strip() or "unknown error"
            raise RuntimeError(f"Failed to checkout git ref {ref}: {detail}")
        rev = subprocess.run(
            ["git", "-C", str(repo_dir), "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            check=False,
        )
        if rev.returncode != 0 or not rev.stdout.strip():
            detail = (rev.stderr or rev.stdout).strip() or "unknown error"
            raise RuntimeError(f"Failed to resolve git revision for {ref}: {detail}")
        resolved_commit = rev.stdout.strip()
        tree = subprocess.run(
            ["git", "-C", str(repo_dir), "rev-parse", "HEAD^{tree}"],
            capture_output=True,
            text=True,
            check=False,
        )
        if tree.returncode != 0 or not tree.stdout.strip():
            detail = (tree.stderr or tree.stdout).strip() or "unknown error"
            raise RuntimeError(f"Failed to resolve git tree hash: {detail}")
        tree_hash = tree.stdout.strip()
        source_dir = repo_dir
        if subdirectory:
            source_dir = repo_dir / subdirectory
            if not source_dir.exists():
                raise RuntimeError(f"Git subdirectory not found: {subdirectory}")
        if dest.exists():
            shutil.rmtree(dest)
        if source_dir.is_dir():
            shutil.copytree(source_dir, dest, ignore=shutil.ignore_patterns(".git"))
        else:
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source_dir, dest)
        return resolved_commit, tree_hash


def deps(include_dev: bool, json_output: bool = False, verbose: bool = False) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "deps")
    if root_error is not None:
        return root_error
    pyproject = _load_toml(root / "pyproject.toml")
    lock = _load_toml(root / "uv.lock")
    deps = _collect_deps(pyproject, include_dev=include_dev)
    packages = _lock_packages(lock)
    allow = _dep_allowlists(pyproject)

    rows: list[dict[str, Any]] = []
    for dep in deps:
        key = _normalize_name(dep)
        pkg = packages.get(key)
        version = pkg.get("version") if pkg else None
        tier, reason = _classify_tier(dep, pkg, allow)
        rows.append({"name": dep, "version": version, "tier": tier, "reason": reason})

    if json_output:
        data: dict[str, Any] = {"dependencies": rows}
        if verbose:
            data["summary"] = _summarize_tiers(rows)
        payload = _json_payload("deps", "ok", data=data)
        _emit_json(payload, json_output)
        return 0

    for row in rows:
        version = row["version"] or "missing"
        print(f"{row['name']} {version} {row['tier']} {row['reason']}")
    if verbose:
        summary = _summarize_tiers(rows)
        print(
            "Summary: "
            + ", ".join(f"{tier}={count}" for tier, count in summary.items())
        )
    return 0


def vendor(
    include_dev: bool,
    json_output: bool = False,
    verbose: bool = False,
    output: str | None = None,
    dry_run: bool = False,
    allow_non_tier_a: bool = False,
    extras: list[str] | None = None,
    deterministic: bool = True,
    deterministic_warn: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "vendor")
    if root_error is not None:
        return root_error
    warnings: list[str] = []
    lock_error = _check_lockfiles(
        root,
        json_output,
        warnings,
        deterministic,
        deterministic_warn,
        "vendor",
    )
    if lock_error is not None:
        return lock_error
    pyproject = _load_toml(root / "pyproject.toml")
    lock = _load_toml(root / "uv.lock")
    extras_set: set[str] = set()
    for extra in extras or []:
        for token in re.split(r"[,\s]+", extra):
            if token:
                extras_set.add(token)
    deps, root_extras, skipped_root = _collect_dep_specs(
        pyproject,
        include_dev=include_dev,
        extras=extras_set,
    )
    env = _marker_environment()
    packages, deps_graph, skipped = _lock_package_graph(
        lock,
        env=env,
        selected_extras=root_extras,
    )
    allow = _dep_allowlists(pyproject)

    root_names = deps
    closure, missing = _resolve_dependency_closure(root_names, deps_graph)
    vendor_list: list[dict[str, Any]] = []
    blockers: list[dict[str, Any]] = []
    for name in closure:
        pkg = packages.get(name)
        display = pkg.get("name", name) if pkg else name
        tier, reason = _classify_tier(display, pkg, allow)
        version = pkg.get("version") if pkg else None
        entry = {
            "name": display,
            "version": version,
            "tier": tier,
            "reason": reason,
        }
        if tier == "Tier A":
            vendor_list.append(entry)
        else:
            blockers.append(entry)

    if missing:
        blockers.append(
            {
                "name": ",".join(missing),
                "version": None,
                "tier": "Unknown",
                "reason": "missing from uv.lock",
            }
        )

    if blockers and not allow_non_tier_a:
        if json_output:
            payload = _json_payload(
                "vendor",
                "error",
                data={
                    "vendor": vendor_list,
                    "blockers": blockers,
                    "missing": missing,
                    "extras": sorted(extras_set),
                    "skipped": skipped,
                    "skipped_root": skipped_root,
                },
                errors=["vendoring blocked by non-Tier A dependencies"],
                warnings=warnings,
            )
            _emit_json(payload, json_output=True)
            return 2
        print("Vendoring blocked by non-Tier A dependencies:")
        for entry in blockers:
            version = entry["version"] or "missing"
            print(f"- {entry['name']} {version} {entry['tier']} {entry['reason']}")
        return 2

    output_dir = Path(output) if output else Path("vendor")
    package_dir = output_dir / "packages"
    local_dir = output_dir / "local"
    manifest: dict[str, Any] = {
        "created_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "root": str(root),
        "include_dev": include_dev,
        "extras": sorted(extras_set),
        "packages": [],
        "blockers": blockers,
        "missing": missing,
        "skipped": skipped,
        "skipped_root": skipped_root,
    }

    if not dry_run:
        package_dir.mkdir(parents=True, exist_ok=True)
        local_dir.mkdir(parents=True, exist_ok=True)

    for entry in vendor_list:
        pkg = packages.get(_normalize_name(entry["name"]))
        if not pkg:
            continue
        source = pkg.get("source", {})
        if source.get("path"):
            src_path = Path(source["path"])
            if not src_path.is_absolute():
                src_path = (root / src_path).resolve()
            dest = local_dir / entry["name"]
            if not dry_run:
                if dest.exists():
                    shutil.rmtree(dest)
                if src_path.is_dir():
                    shutil.copytree(src_path, dest)
                else:
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(src_path, dest)
            manifest["packages"].append(
                {
                    **entry,
                    "source": "path",
                    "path": str(src_path),
                }
            )
            continue
        if source.get("git"):
            url = source.get("git")
            if not isinstance(url, str) or not url.strip():
                blockers.append(
                    {**entry, "tier": "Tier A", "reason": "git source missing url"}
                )
                continue
            if shutil.which("git") is None:
                return _fail(
                    "git is required to vendor git sources",
                    json_output,
                    command="vendor",
                )
            ref, ref_kind = _git_ref_from_source(source)
            if ref is None:
                blockers.append(
                    {
                        **entry,
                        "tier": "Tier A",
                        "reason": "git source missing pinned revision",
                    }
                )
                continue
            resolved_ref = ref
            resolved_error = None
            if ref_kind in {"tag", "branch"}:
                resolved_ref, resolved_error = _resolve_git_ref(url, ref)
            if resolved_error:
                return _fail(
                    resolved_error,
                    json_output,
                    command="vendor",
                )
            if resolved_ref is None:
                return _fail(
                    "unable to resolve git ref",
                    json_output,
                    command="vendor",
                )
            subdir = source.get("subdirectory") or source.get("subdir")
            if subdir is not None and not isinstance(subdir, str):
                blockers.append(
                    {
                        **entry,
                        "tier": "Tier A",
                        "reason": "git source subdirectory must be a string",
                    }
                )
                continue
            dest = local_dir / entry["name"]
            resolved_commit = resolved_ref
            tree_hash = None
            if not dry_run:
                try:
                    resolved_commit, tree_hash = _clone_git_source(
                        url, resolved_ref, dest, subdirectory=subdir
                    )
                except RuntimeError as exc:
                    return _fail(
                        str(exc),
                        json_output,
                        command="vendor",
                    )
            manifest["packages"].append(
                {
                    **entry,
                    "source": "git",
                    "git": url,
                    "ref": ref,
                    "ref_kind": ref_kind,
                    "resolved": resolved_commit,
                    "tree": tree_hash,
                    "subdirectory": subdir,
                    "path": str(dest),
                }
            )
            continue
        picked = _pick_vendor_artifact(pkg)
        if picked is None:
            blockers.append(
                {**entry, "tier": "Tier A", "reason": "no artifact in uv.lock"}
            )
            continue
        kind, artifact = picked
        url = artifact.get("url", "")
        hash_value = artifact.get("hash", "")
        filename = Path(url).name if url else f"{entry['name']}-{entry['version']}"
        dest = package_dir / filename
        if not dry_run:
            try:
                data = _download_artifact(url, hash_value)
            except Exception as exc:
                return _fail(
                    f"Failed to download {url}: {exc}",
                    json_output,
                    command="vendor",
                )
            dest.write_bytes(data)
        manifest["packages"].append(
            {
                **entry,
                "source": kind,
                "url": url,
                "hash": hash_value,
                "file": str(dest),
            }
        )

    if not dry_run:
        manifest_path = output_dir / "manifest.json"
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")

    if json_output:
        data: dict[str, Any] = {
            "vendor": vendor_list,
            "blockers": blockers,
            "missing": missing,
            "output": str(output_dir),
            "dry_run": dry_run,
            "extras": sorted(extras_set),
            "skipped": skipped,
            "skipped_root": skipped_root,
            "deterministic": deterministic,
        }
        if verbose:
            data["count"] = len(vendor_list)
        payload = _json_payload("vendor", "ok", data=data, warnings=warnings)
        _emit_json(payload, json_output=True)
        return 0

    banner = "Vendoring plan (Tier A)" if dry_run else "Vendoring Tier A packages"
    print(f"{banner}:")
    for entry in vendor_list:
        version = entry["version"] or "missing"
        print(f"- {entry['name']} {version}")
    if blockers:
        print("Blockers:")
        for entry in blockers:
            version = entry["version"] or "missing"
            print(f"- {entry['name']} {version} {entry['tier']} {entry['reason']}")
    if verbose:
        print(f"Total Tier A packages: {len(vendor_list)}")
        print(f"Output: {output_dir}")
    return 0


def clean(
    json_output: bool = False,
    verbose: bool = False,
    cache: bool = True,
    artifacts: bool = True,
    bins: bool = False,
    repo_artifacts: bool = False,
    cargo_target: bool = False,
    clean_all: bool = False,
    include_venvs: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "clean")
    if root_error is not None:
        return root_error
    removed: list[str] = []
    missing: list[str] = []
    failures: list[str] = []

    if clean_all:
        cache = True
        artifacts = True
        bins = True
        repo_artifacts = True
        cargo_target = True
        include_venvs = True

    def _remove_path(path: Path) -> None:
        try:
            if path.is_symlink():
                path.unlink()
                removed.append(str(path))
                return
            if path.exists():
                if path.is_dir():
                    shutil.rmtree(path)
                else:
                    path.unlink()
                removed.append(str(path))
            else:
                missing.append(str(path))
        except OSError as exc:
            failures.append(f"{path}: {exc}")

    def _is_virtualenv_path(path: Path) -> bool:
        for part in path.parts:
            if part in {"venv", ".env", "env"}:
                return True
            if part.startswith(".venv"):
                return True
        return False

    def _iter_pycache_dirs(root_dir: Path) -> list[Path]:
        pycache_dirs: list[Path] = []
        for dirpath, dirnames, _filenames in os.walk(root_dir, followlinks=False):
            current = Path(dirpath)
            if not include_venvs and _is_virtualenv_path(current):
                dirnames[:] = []
                continue
            pruned: list[str] = []
            for name in dirnames:
                candidate = Path(dirpath, name)
                if candidate.is_symlink():
                    continue
                pruned.append(name)
            dirnames[:] = pruned
            if current.name == "__pycache__":
                pycache_dirs.append(current)
                dirnames[:] = []
        return pycache_dirs

    if cache:
        cache_root = _default_molt_cache()
        _remove_path(cache_root)
    if artifacts:
        build_root = _default_molt_home() / "build"
        _remove_path(build_root)
    if bins:
        bin_root = _default_molt_bin()
        _remove_path(bin_root)
    if repo_artifacts:
        repo_dirs = [
            root / "vendor",
            root / "logs",
            root / "dist",
            root / "build",
            root / ".pytest_cache",
            root / ".ruff_cache",
            root / ".mypy_cache",
            root / "__pycache__",
        ]
        for path in repo_dirs:
            _remove_path(path)
        for path in _iter_pycache_dirs(root):
            _remove_path(path)
        repo_files = [
            root / "output.wasm",
            root / "output_linked.wasm",
            root / "output.o",
            root / "main_stub.c",
        ]
        for path in repo_files:
            _remove_path(path)
    if cargo_target:
        cargo_root = root / "target"
        _remove_path(cargo_root)
    if json_output:
        data: dict[str, Any] = {"removed": removed}
        if verbose:
            data["missing"] = missing
        status = "error" if failures else "ok"
        payload = _json_payload(
            "clean",
            status,
            data=data,
            errors=failures if failures else None,
        )
        _emit_json(payload, json_output=True)
    else:
        if removed:
            print("Removed:")
            for path in removed:
                print(f"- {path}")
        if failures:
            print("Failed:")
            for entry in failures:
                print(f"- {entry}")
        if verbose and missing:
            print("Missing:")
            for path in missing:
                print(f"- {path}")
    return 1 if failures else 0


def show_config(
    config_root: Path,
    config: dict[str, Any],
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    molt_toml = config_root / "molt.toml"
    pyproject = config_root / "pyproject.toml"
    build_cfg = _resolve_build_config(config)
    run_cfg = _resolve_command_config(config, "run")
    compare_cfg = _resolve_command_config(config, "compare")
    test_cfg = _resolve_command_config(config, "test")
    diff_cfg = _resolve_command_config(config, "diff")
    extension_cfg = _resolve_command_config(config, "extension")
    publish_cfg = _resolve_command_config(config, "publish")
    caps_cfg = _resolve_capabilities_config(config)
    data: dict[str, Any] = {
        "root": str(config_root),
        "sources": {
            "molt_toml": str(molt_toml) if molt_toml.exists() else None,
            "pyproject": str(pyproject) if pyproject.exists() else None,
        },
        "build": build_cfg,
        "run": run_cfg,
        "compare": compare_cfg,
        "test": test_cfg,
        "diff": diff_cfg,
        "extension": extension_cfg,
        "publish": publish_cfg,
        "capabilities": caps_cfg,
        "paths": {
            "molt_home": str(_default_molt_home()),
            "molt_bin": str(_default_molt_bin()),
            "molt_cache": str(_default_molt_cache()),
            "build_root": str(_default_molt_home() / "build"),
        },
    }
    if json_output:
        data["config"] = config
        payload = _json_payload("config", "ok", data=data)
        _emit_json(payload, json_output=True)
        return 0
    print(f"Config root: {config_root}")
    if data["sources"]["molt_toml"] or data["sources"]["pyproject"]:
        print("Sources:")
        if data["sources"]["molt_toml"]:
            print(f"- {data['sources']['molt_toml']}")
        if data["sources"]["pyproject"]:
            print(f"- {data['sources']['pyproject']}")
    print("Paths:")
    for key, value in data["paths"].items():
        print(f"- {key}: {value}")
    if build_cfg:
        print("Build defaults:")
        for key in sorted(build_cfg):
            print(f"- {key}: {build_cfg[key]}")
    else:
        print("Build defaults: none")
    if run_cfg:
        print("Run defaults:")
        for key in sorted(run_cfg):
            print(f"- {key}: {run_cfg[key]}")
    else:
        print("Run defaults: none")
    if compare_cfg:
        print("Compare defaults:")
        for key in sorted(compare_cfg):
            print(f"- {key}: {compare_cfg[key]}")
    else:
        print("Compare defaults: none")
    if test_cfg:
        print("Test defaults:")
        for key in sorted(test_cfg):
            print(f"- {key}: {test_cfg[key]}")
    else:
        print("Test defaults: none")
    if diff_cfg:
        print("Diff defaults:")
        for key in sorted(diff_cfg):
            print(f"- {key}: {diff_cfg[key]}")
    else:
        print("Diff defaults: none")
    if extension_cfg:
        print("Extension defaults:")
        for key in sorted(extension_cfg):
            print(f"- {key}: {extension_cfg[key]}")
    else:
        print("Extension defaults: none")
    if publish_cfg:
        print("Publish defaults:")
        for key in sorted(publish_cfg):
            print(f"- {key}: {publish_cfg[key]}")
    else:
        print("Publish defaults: none")
    if caps_cfg is not None:
        print(f"Capabilities: {_format_capabilities_input(caps_cfg)}")
    else:
        print("Capabilities: none")
    if verbose:
        print("Merged config:")
        print(json.dumps(config, indent=2))
    return 0


def _completion_script(shell: str) -> str:
    commands = [
        "build",
        "extension",
        "check",
        "run",
        "compare",
        "parity-run",
        "test",
        "diff",
        "bench",
        "profile",
        "lint",
        "doctor",
        "package",
        "publish",
        "verify",
        "deps",
        "vendor",
        "clean",
        "config",
        "completion",
    ]
    extension_subcommands = ["build", "audit", "scan"]
    extension_options = {
        "build": [
            "--project",
            "--out-dir",
            "--molt-abi",
            "--target",
            "--capabilities",
            "--deterministic",
            "--no-deterministic",
            "--json",
            "--verbose",
        ],
        "audit": [
            "--path",
            "--require-capabilities",
            "--require-abi",
            "--require-checksum",
            "--json",
            "--verbose",
        ],
    }
    options = {
        "build": [
            "--module",
            "--target",
            "--codec",
            "--type-hints",
            "--fallback",
            "--type-facts",
            "--pgo-profile",
            "--output",
            "--out-dir",
            "--sysroot",
            "--emit",
            "--emit-ir",
            "--profile",
            "--build-profile",
            "--deterministic",
            "--no-deterministic",
            "--deterministic-warn",
            "--no-deterministic-warn",
            "--portable",
            "--trusted",
            "--no-trusted",
            "--capabilities",
            "--cache",
            "--no-cache",
            "--cache-dir",
            "--cache-report",
            "--rebuild",
            "--respect-pythonpath",
            "--no-respect-pythonpath",
            "--json",
            "--verbose",
        ],
        "check": [
            "--output",
            "--strict",
            "--deterministic",
            "--no-deterministic",
            "--deterministic-warn",
            "--no-deterministic-warn",
            "--json",
            "--verbose",
        ],
        "run": [
            "--module",
            "--build-arg",
            "--profile",
            "--build-profile",
            "--rebuild",
            "--timing",
            "--capabilities",
            "--trusted",
            "--no-trusted",
            "--json",
            "--verbose",
        ],
        "compare": [
            "--python",
            "--python-version",
            "--module",
            "--build-arg",
            "--profile",
            "--build-profile",
            "--rebuild",
            "--capabilities",
            "--trusted",
            "--no-trusted",
            "--json",
            "--verbose",
        ],
        "parity-run": [
            "--python",
            "--python-version",
            "--module",
            "--timing",
            "--json",
            "--verbose",
        ],
        "test": [
            "--suite",
            "--python-version",
            "--profile",
            "--build-profile",
            "--trusted",
            "--no-trusted",
            "--json",
            "--verbose",
        ],
        "diff": [
            "--python-version",
            "--profile",
            "--build-profile",
            "--trusted",
            "--no-trusted",
            "--json",
            "--verbose",
        ],
        "bench": ["--wasm", "--script", "--json", "--verbose"],
        "profile": ["--json", "--verbose"],
        "lint": ["--json", "--verbose"],
        "doctor": ["--strict", "--json", "--verbose"],
        "package": [
            "--output",
            "--deterministic",
            "--no-deterministic",
            "--deterministic-warn",
            "--no-deterministic-warn",
            "--capabilities",
            "--sbom",
            "--no-sbom",
            "--sbom-output",
            "--sbom-format",
            "--signature",
            "--signature-output",
            "--sign",
            "--no-sign",
            "--signer",
            "--signing-key",
            "--signing-identity",
            "--json",
            "--verbose",
        ],
        "publish": [
            "--registry",
            "--registry-token",
            "--registry-user",
            "--registry-password",
            "--registry-timeout",
            "--dry-run",
            "--deterministic",
            "--no-deterministic",
            "--deterministic-warn",
            "--no-deterministic-warn",
            "--capabilities",
            "--require-signature",
            "--no-require-signature",
            "--verify-signature",
            "--no-verify-signature",
            "--trusted-signers",
            "--signer",
            "--signing-key",
            "--json",
            "--verbose",
        ],
        "verify": [
            "--package",
            "--manifest",
            "--artifact",
            "--require-checksum",
            "--extension-metadata",
            "--no-extension-metadata",
            "--require-extension-capabilities",
            "--require-extension-abi",
            "--require-deterministic",
            "--require-signature",
            "--no-require-signature",
            "--verify-signature",
            "--no-verify-signature",
            "--trusted-signers",
            "--signer",
            "--signing-key",
            "--capabilities",
            "--json",
            "--verbose",
        ],
        "deps": ["--include-dev", "--json", "--verbose"],
        "vendor": [
            "--include-dev",
            "--output",
            "--dry-run",
            "--allow-non-tier-a",
            "--extras",
            "--deterministic",
            "--no-deterministic",
            "--deterministic-warn",
            "--no-deterministic-warn",
            "--json",
            "--verbose",
        ],
        "clean": [
            "--all",
            "--cache",
            "--no-cache",
            "--artifacts",
            "--no-artifacts",
            "--bins",
            "--no-bins",
            "--repo-artifacts",
            "--no-repo-artifacts",
            "--include-venvs",
            "--cargo-target",
            "--no-cargo-target",
            "--json",
            "--verbose",
        ],
        "config": ["--file", "--json", "--verbose"],
        "completion": ["--shell", "--json", "--verbose"],
    }
    if shell == "bash":
        lines = [
            "_molt_complete() {",
            "  local cur prev",
            "  COMPREPLY=()",
            '  cur="${COMP_WORDS[COMP_CWORD]}"',
            '  prev="${COMP_WORDS[COMP_CWORD-1]}"',
            "  if [[ ${COMP_CWORD} -eq 1 ]]; then",
            f'    COMPREPLY=( $(compgen -W "{" ".join(commands)}" -- "$cur") )',
            "    return 0",
            "  fi",
            '  if [[ "${COMP_WORDS[1]}" == "extension" ]]; then',
            "    if [[ ${COMP_CWORD} -eq 2 ]]; then",
            '      COMPREPLY=( $(compgen -W "build audit" -- "$cur") )',
            "      return 0",
            "    fi",
            '    case "${COMP_WORDS[2]}" in',
        ]
        for sub in extension_subcommands:
            opts = " ".join(extension_options.get(sub, []))
            lines.append(f'      {sub}) opts="{opts}" ;;')
        lines.extend(
            [
                '      *) opts="" ;;',
                "    esac",
                '    COMPREPLY=( $(compgen -W "$opts" -- "$cur") )',
                "    return 0",
                "  fi",
                '  case "${COMP_WORDS[1]}" in',
            ]
        )
        for cmd in commands:
            opts = " ".join(options.get(cmd, []))
            lines.append(f'    {cmd}) opts="{opts}" ;;')
        lines.extend(
            [
                '    *) opts="" ;;',
                "  esac",
                '  COMPREPLY=( $(compgen -W "$opts" -- "$cur") )',
                "}",
                "complete -F _molt_complete molt",
            ]
        )
        return "\n".join(lines) + "\n"
    if shell == "zsh":
        lines = [
            "#compdef molt",
            "_molt() {",
            "  local -a commands",
            f"  commands=({' '.join(commands)})",
            "  if (( CURRENT == 2 )); then",
            "    compadd $commands",
            "    return",
            "  fi",
            "  if [[ $words[2] == extension ]]; then",
            "    if (( CURRENT == 3 )); then",
            "      compadd build audit",
            "      return",
            "    fi",
            "    local -a extension_opts",
            "    case $words[3] in",
        ]
        for sub in extension_subcommands:
            opts = " ".join(extension_options.get(sub, []))
            lines.append(f"      {sub}) extension_opts=({opts}) ;;")
        lines.extend(
            [
                "      *) extension_opts=() ;;",
                "    esac",
                "    compadd $extension_opts",
                "    return",
                "  fi",
                "  local -a opts",
                "  case $words[2] in",
            ]
        )
        for cmd in commands:
            opts = " ".join(options.get(cmd, []))
            lines.append(f"    {cmd}) opts=({opts}) ;;")
        lines.extend(
            [
                "    *) opts=() ;;",
                "  esac",
                "  compadd $opts",
                "}",
                "compdef _molt molt",
            ]
        )
        return "\n".join(lines) + "\n"
    if shell == "fish":
        lines = [
            f"complete -c molt -f -n '__fish_use_subcommand' -a \"{' '.join(commands)}\"",
            "complete -c molt -f -n '__fish_seen_subcommand_from extension; and not __fish_seen_subcommand_from build audit' -a \"build audit\"",
        ]
        for cmd in commands:
            for opt in options.get(cmd, []):
                opt_name = opt.lstrip("-")
                lines.append(
                    f"complete -c molt -n '__fish_seen_subcommand_from {cmd}' -l {opt_name}"
                )
        for sub in extension_subcommands:
            for opt in extension_options.get(sub, []):
                opt_name = opt.lstrip("-")
                lines.append(
                    "complete -c molt "
                    "-n '__fish_seen_subcommand_from extension; and "
                    f"__fish_seen_subcommand_from {sub}' -l {opt_name}"
                )
        return "\n".join(lines) + "\n"
    raise ValueError(f"Unsupported shell: {shell}")


def completion(shell: str, json_output: bool = False, verbose: bool = False) -> int:
    try:
        script = _completion_script(shell)
    except ValueError as exc:
        return _fail(str(exc), json_output, command="completion")
    if json_output:
        payload = _json_payload(
            "completion",
            "ok",
            data={"shell": shell, "script": script},
        )
        _emit_json(payload, json_output=True)
    else:
        print(script, end="")
    return 0


def _strip_leading_double_dash(args: list[str]) -> list[str]:
    if args and args[0] == "--":
        return args[1:]
    return args


def _extract_output_arg(args: list[str]) -> Path | None:
    for idx, arg in enumerate(args):
        if arg == "--output" and idx + 1 < len(args):
            return Path(args[idx + 1])
        if arg.startswith("--output="):
            return Path(arg.split("=", 1)[1])
    return None


def _extract_out_dir_arg(args: list[str]) -> Path | None:
    for idx, arg in enumerate(args):
        if arg == "--out-dir" and idx + 1 < len(args):
            return Path(args[idx + 1])
        if arg.startswith("--out-dir="):
            return Path(arg.split("=", 1)[1])
    return None


def _extract_emit_arg(args: list[str]) -> str | None:
    for idx, arg in enumerate(args):
        if arg == "--emit" and idx + 1 < len(args):
            return args[idx + 1]
        if arg.startswith("--emit="):
            return arg.split("=", 1)[1]
    return None


def _build_args_has_cache_flag(args: list[str]) -> bool:
    for arg in args:
        if arg in {"--cache", "--no-cache", "--rebuild"}:
            return True
    return False


def _resolve_binary_output(path_str: str) -> Path | None:
    path = Path(path_str)
    if path.exists():
        return path
    fallback = path.with_suffix(".exe")
    if fallback.exists():
        return fallback
    return None


def _build_args_has_trusted_flag(args: list[str]) -> bool:
    for arg in args:
        if arg in {"--trusted", "--no-trusted"}:
            return True
    return False


def _build_args_has_capabilities_flag(args: list[str]) -> bool:
    for arg in args:
        if arg == "--capabilities" or arg.startswith("--capabilities="):
            return True
    return False


def _build_args_has_profile_flag(args: list[str]) -> bool:
    for arg in args:
        if (
            arg == "--profile"
            or arg.startswith("--profile=")
            or arg == "--build-profile"
            or arg.startswith("--build-profile=")
        ):
            return True
    return False


def _ensure_cli_hash_seed() -> None:
    desired = os.environ.get(_HASH_SEED_OVERRIDE_ENV, "0").strip()
    if not desired:
        desired = "0"
    if desired.lower() in {"off", "disable", "random"}:
        return
    if os.environ.get("PYTHONHASHSEED") == desired:
        return
    if os.environ.get(_HASH_SEED_SENTINEL_ENV) == "1":
        return
    env = os.environ.copy()
    env["PYTHONHASHSEED"] = desired
    env[_HASH_SEED_SENTINEL_ENV] = "1"
    os.execvpe(sys.executable, [sys.executable, *sys.argv], env)


def main() -> int:
    _ensure_cli_hash_seed()
    parser = argparse.ArgumentParser(prog="molt")
    subparsers = parser.add_subparsers(dest="command", required=True)

    build_parser = subparsers.add_parser("build", help="Compile a Python file")
    build_parser.add_argument("file", nargs="?", help="Path to Python source")
    build_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    build_parser.add_argument(
        "--target",
        default=None,
        help="Target backend: native, wasm, or a target triple.",
    )
    build_parser.add_argument(
        "--codec",
        choices=["msgpack", "cbor", "json"],
        default=None,
        help="Structured codec for parse calls (default from config or msgpack).",
    )
    build_parser.add_argument(
        "--type-hints",
        choices=["ignore", "trust", "check"],
        default=None,
        help="Apply type annotations to guide lowering and specialization.",
    )
    build_parser.add_argument(
        "--fallback",
        choices=["error", "bridge"],
        default=None,
        help="Fallback policy for unsupported constructs.",
    )
    build_parser.add_argument(
        "--type-facts",
        help="Path to type facts JSON from `molt check`.",
    )
    build_parser.add_argument(
        "--pgo-profile",
        help="Path to a Molt profile artifact (molt_profile.json) for PGO hints.",
    )
    build_parser.add_argument(
        "--runtime-feedback",
        help=(
            "Path to a Molt runtime feedback artifact "
            "(molt_runtime_feedback.json) for measured hot-function promotion hints."
        ),
    )
    build_parser.add_argument(
        "--output",
        help=(
            "Output path for the native binary or wasm artifact "
            "(relative to --out-dir when set, otherwise project root). "
            "If the path is a directory (or ends with a path separator), "
            "the default filename is used within that directory."
        ),
    )
    build_parser.add_argument(
        "--out-dir",
        help=(
            "Output directory for final artifacts (binary/wasm/object). "
            "Intermediates stay under MOLT_HOME/build/<entry> by default. "
            "Native binaries otherwise default to MOLT_BIN."
        ),
    )
    build_parser.add_argument(
        "--sysroot",
        help=(
            "Sysroot path for native linking (relative paths resolve under the project "
            "root; defaults to MOLT_SYSROOT or MOLT_CROSS_SYSROOT when set)."
        ),
    )
    build_parser.add_argument(
        "--emit",
        choices=["bin", "obj", "wasm"],
        default=None,
        help="Select which artifact to emit (native: bin/obj, wasm: wasm).",
    )
    build_parser.add_argument(
        "--linked",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Emit a linked wasm artifact (output_linked.wasm) alongside output.wasm.",
    )
    build_parser.add_argument(
        "--linked-output",
        help=(
            "Output path for the linked wasm artifact "
            "(relative to --out-dir when set, otherwise project root)."
        ),
    )
    build_parser.add_argument(
        "--require-linked",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require linked wasm output for wasm targets (fails if linking is unavailable).",
    )
    build_parser.add_argument(
        "--emit-ir",
        help="Write the lowered IR JSON to a file path.",
    )
    build_parser.add_argument(
        "--profile",
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile for backend/runtime (default: release).",
    )
    build_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic inputs (lockfiles).",
    )
    build_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    build_parser.add_argument(
        "--portable",
        action="store_true",
        default=False,
        help="Use baseline ISA (no host-specific CPU features). Ensures cross-machine reproducible codegen at ~5-15%% runtime cost.",
    )
    build_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments (native only).",
    )
    build_parser.add_argument(
        "--cache",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Enable build cache under MOLT_CACHE (defaults to the OS cache).",
    )
    build_parser.add_argument(
        "--cache-dir",
        help="Override the build cache directory (default: MOLT_CACHE).",
    )
    build_parser.add_argument(
        "--cache-report",
        action="store_true",
        help="Print cache hit/miss details even without --verbose.",
    )
    build_parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Disable the build cache (alias for --no-cache).",
    )
    build_parser.add_argument(
        "--respect-pythonpath",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Include PYTHONPATH entries as module roots during compilation.",
    )
    build_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    build_parser.add_argument(
        "--diagnostics",
        action=argparse.BooleanOptionalAction,
        default=None,
        help=(
            "Enable compile diagnostics payloads (phase timings, module reasons, "
            "frontend/midend summaries)."
        ),
    )
    build_parser.add_argument(
        "--diagnostics-file",
        help=(
            "Optional path for compile diagnostics JSON (relative paths resolve "
            "under the build artifacts root). Implies --diagnostics."
        ),
    )
    build_parser.add_argument(
        "--diagnostics-verbosity",
        choices=["summary", "default", "full"],
        default=None,
        help=(
            "Select stderr build diagnostics detail level. "
            "JSON/file diagnostics remain complete."
        ),
    )
    build_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    build_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    extension_parser = subparsers.add_parser(
        "extension",
        help="Build and audit C extensions compiled against libmolt.",
    )
    extension_subparsers = extension_parser.add_subparsers(
        dest="extension_command", required=True
    )
    extension_build_parser = extension_subparsers.add_parser(
        "build",
        help="Compile a C extension against libmolt and emit a wheel + sidecar.",
    )
    extension_build_parser.add_argument(
        "--project",
        help="Project directory containing pyproject.toml (default: cwd).",
    )
    extension_build_parser.add_argument(
        "--out-dir",
        help="Output directory for wheel + extension_manifest.json (default: dist/).",
    )
    extension_build_parser.add_argument(
        "--molt-abi",
        help=(
            "Molt C-API ABI version override "
            "(default: tool.molt.extension.molt_c_api_version or MOLT_C_API_VERSION)."
        ),
    )
    extension_build_parser.add_argument(
        "--target",
        help="Target triple for extension build (default: native host target).",
    )
    extension_build_parser.add_argument(
        "--capabilities",
        help=(
            "Capabilities allowlist/profiles override "
            "(default: tool.molt.extension.capabilities)."
        ),
    )
    extension_build_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic lockfile and reproducible wheel checks.",
    )
    extension_build_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    extension_build_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    extension_audit_parser = extension_subparsers.add_parser(
        "audit",
        help="Audit an extension manifest and wheel for ABI/capability compatibility.",
    )
    extension_audit_parser.add_argument(
        "--path",
        required=True,
        help="Path to a wheel, extension_manifest.json, or directory containing it.",
    )
    extension_audit_parser.add_argument(
        "--require-capabilities",
        action="store_true",
        help="Fail when the manifest capability list is empty.",
    )
    extension_audit_parser.add_argument(
        "--require-abi",
        help="Require an exact molt_c_api_version match.",
    )
    extension_audit_parser.add_argument(
        "--require-checksum",
        action="store_true",
        help="Require wheel and extension checksums in the manifest.",
    )
    extension_audit_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    extension_audit_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    extension_scan_parser = extension_subparsers.add_parser(
        "scan",
        help=(
            "Scan extension sources for unsupported Py* C-API usage "
            "against include/molt/Python.h."
        ),
    )
    extension_scan_parser.add_argument(
        "--project",
        help="Project directory containing pyproject.toml (default: cwd).",
    )
    extension_scan_parser.add_argument(
        "--source",
        action="append",
        help=(
            "Source path to scan (repeatable). If omitted, uses "
            "tool.molt.extension.sources from pyproject.toml."
        ),
    )
    extension_scan_parser.add_argument(
        "--fail-on-missing",
        action="store_true",
        help="Return non-zero if unsupported Py* C-API symbols are detected.",
    )
    extension_scan_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    extension_scan_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    internal_batch_parser = subparsers.add_parser(
        "internal-batch-build-server",
        help=argparse.SUPPRESS,
    )
    internal_batch_parser.add_argument(
        "--json", action="store_true", help=argparse.SUPPRESS
    )
    internal_batch_parser.add_argument(
        "--verbose", action="store_true", help=argparse.SUPPRESS
    )

    check_parser = subparsers.add_parser(
        "check", help="Generate a type facts artifact (ty-backed when available)"
    )
    check_parser.add_argument("path", help="Python file or package directory")
    check_parser.add_argument(
        "--output",
        default="type_facts.json",
        help="Output path for type facts JSON.",
    )
    check_parser.add_argument(
        "--strict",
        action="store_true",
        help="Mark facts as trusted (strict tier).",
    )
    check_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic inputs (lockfiles).",
    )
    check_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    check_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    check_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    run_parser = subparsers.add_parser(
        "run", help="Compile with Molt and run the native binary"
    )
    run_parser.add_argument("file", nargs="?", help="Path to Python source")
    run_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    run_parser.add_argument(
        "--build-arg",
        action="append",
        default=[],
        help="Extra args passed to `molt build`.",
    )
    run_parser.add_argument(
        "--profile",
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile passed to `molt build` (default: dev).",
    )
    run_parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Disable build cache for `molt build`.",
    )
    run_parser.add_argument(
        "--timing",
        action="store_true",
        help="Emit timing summary (compile + run).",
    )
    run_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    run_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    run_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    run_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    run_parser.add_argument(
        "script_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the script (use -- to separate).",
    )

    compare_parser = subparsers.add_parser(
        "compare", help="Compare CPython vs Molt outputs and timing"
    )
    compare_parser.add_argument("file", nargs="?", help="Path to Python source")
    compare_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    compare_parser.add_argument(
        "--python",
        help="Python interpreter (path) or version (e.g. 3.12).",
    )
    compare_parser.add_argument(
        "--python-version",
        help="Python version alias (e.g. 3.12).",
    )
    compare_parser.add_argument(
        "--build-arg",
        action="append",
        default=[],
        help="Extra args passed to `molt build` for the Molt side.",
    )
    compare_parser.add_argument(
        "--profile",
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile passed to `molt build` (default: dev).",
    )
    compare_parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Disable build cache for the Molt build.",
    )
    compare_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    compare_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    compare_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    compare_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    compare_parser.add_argument(
        "script_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the script (use -- to separate).",
    )

    parity_run_parser = subparsers.add_parser(
        "parity-run", help="Run the entrypoint with CPython (no Molt compilation)"
    )
    parity_run_parser.add_argument("file", nargs="?", help="Path to Python source")
    parity_run_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    parity_run_parser.add_argument(
        "--python",
        help="Python interpreter (path) or version (e.g. 3.12).",
    )
    parity_run_parser.add_argument(
        "--python-version",
        help="Python version alias (e.g. 3.12).",
    )
    parity_run_parser.add_argument(
        "--timing",
        action="store_true",
        help="Emit timing summary for the CPython run.",
    )
    parity_run_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    parity_run_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    parity_run_parser.add_argument(
        "script_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the script (use -- to separate).",
    )

    test_parser = subparsers.add_parser("test", help="Run Molt test suites")
    test_parser.add_argument(
        "--suite",
        choices=["dev", "diff", "pytest"],
        default="dev",
        help="Test suite to run.",
    )
    test_parser.add_argument(
        "--python-version",
        help="Python version for diff suite (e.g. 3.13).",
    )
    test_parser.add_argument(
        "--profile",
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile for Molt builds in suite=diff (default: dev).",
    )
    test_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    test_parser.add_argument("path", nargs="?", help="Optional test path.")
    test_parser.add_argument(
        "pytest_args",
        nargs=argparse.REMAINDER,
        help="Extra pytest args when --suite pytest (use -- to separate).",
    )
    test_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    test_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    diff_parser = subparsers.add_parser(
        "diff", help="Run differential tests against CPython"
    )
    diff_parser.add_argument("path", nargs="?", help="File or directory to test.")
    diff_parser.add_argument(
        "--python-version", help="Python version to test against (e.g. 3.13)."
    )
    diff_parser.add_argument(
        "--profile",
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile for Molt builds in the diff harness (default: dev).",
    )
    diff_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    diff_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    diff_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    bench_parser = subparsers.add_parser("bench", help="Run benchmark suites")
    bench_parser.add_argument(
        "--wasm", action="store_true", help="Use the WASM bench harness."
    )
    bench_parser.add_argument(
        "--script",
        action="append",
        dest="bench_script",
        default=[],
        help="Benchmark a custom script path (repeatable).",
    )
    bench_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    bench_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    bench_parser.add_argument(
        "bench_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the bench tool (use -- to separate).",
    )

    profile_parser = subparsers.add_parser("profile", help="Profile benchmarks")
    profile_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    profile_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    profile_parser.add_argument(
        "profile_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the profile tool (use -- to separate).",
    )

    lint_parser = subparsers.add_parser("lint", help="Run linting checks")
    lint_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    lint_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    doctor_parser = subparsers.add_parser("doctor", help="Check toolchain setup")
    doctor_parser.add_argument(
        "--strict",
        action="store_true",
        help="Return non-zero exit on missing requirements.",
    )
    doctor_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    doctor_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    package_parser = subparsers.add_parser(
        "package", help="Bundle a Molt package artifact"
    )
    package_parser.add_argument("artifact", help="Path to the package artifact.")
    package_parser.add_argument(
        "manifest",
        help=(
            "Path to manifest JSON (fields per "
            "docs/spec/areas/compat/contracts/package_abi_contract.md)."
        ),
    )
    package_parser.add_argument(
        "--output",
        help="Output .moltpkg path (default dist/<name>-<version>-<target>.moltpkg).",
    )
    package_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic package metadata.",
    )
    package_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    package_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    package_parser.add_argument(
        "--sbom",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Emit a CycloneDX SBOM sidecar (default: enabled).",
    )
    package_parser.add_argument(
        "--sbom-output",
        help="Override the SBOM output path (defaults next to the package).",
    )
    package_parser.add_argument(
        "--sbom-format",
        choices=["cyclonedx", "spdx"],
        default="cyclonedx",
        help="SBOM format to emit (default: cyclonedx).",
    )
    package_parser.add_argument(
        "--signature",
        help="Path to a signature file to attach and record in metadata.",
    )
    package_parser.add_argument(
        "--signature-output",
        help="Override the signature sidecar output path (defaults next to the package).",
    )
    package_parser.add_argument(
        "--sign",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Sign the artifact with cosign or codesign.",
    )
    package_parser.add_argument(
        "--signer",
        choices=["auto", "cosign", "codesign"],
        default="auto",
        help="Select the signing tool (default: auto).",
    )
    package_parser.add_argument(
        "--signing-key",
        help="Signing key path for cosign (or set COSIGN_KEY).",
    )
    package_parser.add_argument(
        "--signing-identity",
        help="Signing identity for codesign (or set MOLT_CODESIGN_IDENTITY).",
    )
    package_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    package_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    publish_parser = subparsers.add_parser(
        "publish", help="Publish a Molt package to a registry path or URL"
    )
    publish_parser.add_argument("package", help="Path to the .moltpkg file.")
    publish_parser.add_argument(
        "--registry",
        default="dist/registry",
        help="Registry directory, file path, or HTTP(S) URL.",
    )
    publish_parser.add_argument(
        "--registry-token",
        help=(
            "Bearer token for remote registry auth (or MOLT_REGISTRY_TOKEN; "
            "prefix @ for file)."
        ),
    )
    publish_parser.add_argument(
        "--registry-user",
        help="Username for basic auth (or MOLT_REGISTRY_USER).",
    )
    publish_parser.add_argument(
        "--registry-password",
        help=(
            "Password for basic auth (or MOLT_REGISTRY_PASSWORD; prefix @ for file)."
        ),
    )
    publish_parser.add_argument(
        "--registry-timeout",
        type=float,
        help="Registry request timeout in seconds (or MOLT_REGISTRY_TIMEOUT).",
    )
    publish_parser.add_argument(
        "--dry-run", action="store_true", help="Print the publish plan only."
    )
    publish_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Verify package determinism before publishing.",
    )
    publish_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    publish_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    publish_parser.add_argument(
        "--require-signature",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require a package signature when publishing.",
    )
    publish_parser.add_argument(
        "--verify-signature",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Verify package signatures when publishing.",
    )
    publish_parser.add_argument(
        "--trusted-signers",
        help="Path to a trust policy for allowed signers.",
    )
    publish_parser.add_argument(
        "--signer",
        choices=["auto", "cosign", "codesign"],
        default="auto",
        help="Select the verification tool (default: auto).",
    )
    publish_parser.add_argument(
        "--signing-key",
        help="Verification key path for cosign (or set COSIGN_KEY).",
    )
    publish_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    publish_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    verify_parser = subparsers.add_parser(
        "verify", help="Verify a Molt package manifest and checksum"
    )
    verify_parser.add_argument(
        "--package",
        help="Path to the .moltpkg archive (alternative to --manifest/--artifact).",
    )
    verify_parser.add_argument("--manifest", help="Manifest JSON path.")
    verify_parser.add_argument("--artifact", help="Artifact path.")
    verify_parser.add_argument(
        "--require-checksum",
        action="store_true",
        help="Fail when checksum is missing.",
    )
    verify_parser.add_argument(
        "--extension-metadata",
        action=argparse.BooleanOptionalAction,
        default=None,
        help=(
            "Treat manifest as extension metadata and enforce extension ABI/wheel "
            "checks (default: auto-detect from manifest keys)."
        ),
    )
    verify_parser.add_argument(
        "--require-extension-capabilities",
        action="store_true",
        help="Fail when extension manifest capability list is empty.",
    )
    verify_parser.add_argument(
        "--require-extension-abi",
        help="Require an exact extension molt_c_api_version match.",
    )
    verify_parser.add_argument(
        "--require-deterministic",
        action="store_true",
        help="Fail when manifest is not deterministic.",
    )
    verify_parser.add_argument(
        "--require-signature",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require a package signature.",
    )
    verify_parser.add_argument(
        "--verify-signature",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Verify package signatures when present.",
    )
    verify_parser.add_argument(
        "--trusted-signers",
        help="Path to a trust policy for allowed signers.",
    )
    verify_parser.add_argument(
        "--signer",
        choices=["auto", "cosign", "codesign"],
        default="auto",
        help="Select the verification tool (default: auto).",
    )
    verify_parser.add_argument(
        "--signing-key",
        help="Verification key path for cosign (or set COSIGN_KEY).",
    )
    verify_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    verify_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    verify_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    deps_parser = subparsers.add_parser(
        "deps", help="Show dependency compatibility info"
    )
    deps_parser.add_argument(
        "--include-dev", action="store_true", help="Include dev dependencies"
    )
    deps_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    deps_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    vendor_parser = subparsers.add_parser(
        "vendor", help="Vendor pure Python dependencies"
    )
    vendor_parser.add_argument(
        "--include-dev", action="store_true", help="Include dev dependencies"
    )
    vendor_parser.add_argument(
        "--output",
        help="Output directory for vendored artifacts (default: vendor).",
    )
    vendor_parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show vendoring plan without downloading artifacts.",
    )
    vendor_parser.add_argument(
        "--allow-non-tier-a",
        action="store_true",
        help="Proceed even if non-Tier A dependencies are present.",
    )
    vendor_parser.add_argument(
        "--extras",
        action="append",
        help="Extras to include from project optional-dependencies.",
    )
    vendor_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic inputs (lockfiles).",
    )
    vendor_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    vendor_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    vendor_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    clean_parser = subparsers.add_parser(
        "clean", help="Remove Molt caches, build artifacts, and repo outputs"
    )
    clean_parser.add_argument(
        "--all",
        action="store_true",
        help="Remove all caches, build artifacts, repo outputs, and cargo targets.",
    )
    clean_parser.add_argument(
        "--cache",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Remove build caches under MOLT_CACHE.",
    )
    clean_parser.add_argument(
        "--artifacts",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Remove build artifacts under MOLT_HOME/build.",
    )
    clean_parser.add_argument(
        "--bins",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Remove Molt binaries under MOLT_BIN.",
    )
    clean_parser.add_argument(
        "--repo-artifacts",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Remove repo-local artifacts (vendor/, logs/, caches, output*.wasm).",
    )
    clean_parser.add_argument(
        "--include-venvs",
        action="store_true",
        help="Also clean virtualenv caches when removing repo artifacts.",
    )
    clean_parser.add_argument(
        "--cargo-target",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Remove Cargo target/ build artifacts in the repo root.",
    )
    clean_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    clean_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    config_parser = subparsers.add_parser(
        "config", help="Show Molt configuration defaults"
    )
    config_parser.add_argument(
        "--file",
        help="Resolve project root from a source file path.",
    )
    config_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    config_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    completion_parser = subparsers.add_parser(
        "completion", help="Generate shell completion scripts"
    )
    completion_parser.add_argument(
        "--shell",
        choices=["bash", "zsh", "fish"],
        default="bash",
        help="Shell type to emit.",
    )
    completion_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    completion_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    args = parser.parse_args()

    config_root = _find_project_root(Path.cwd())
    if getattr(args, "file", None):
        try:
            config_root = _find_project_root(Path(args.file).resolve())
        except OSError:
            config_root = _find_project_root(Path.cwd())
    config = _load_molt_config(config_root)
    build_cfg = _resolve_build_config(config)
    run_cfg = _resolve_command_config(config, "run")
    compare_cfg = _resolve_command_config(config, "compare")
    test_cfg = _resolve_command_config(config, "test")
    diff_cfg = _resolve_command_config(config, "diff")
    extension_cfg = _resolve_command_config(config, "extension")
    publish_cfg = _resolve_command_config(config, "publish")
    cfg_capabilities = _resolve_capabilities_config(config)

    if args.command == "internal-batch-build-server":
        return _internal_batch_build_server(
            json_output=args.json,
            verbose=args.verbose,
        )

    if args.command == "build":
        target = args.target or build_cfg.get("target") or "native"
        codec = args.codec or build_cfg.get("codec") or "msgpack"
        type_hints = args.type_hints or build_cfg.get("type_hints") or "ignore"
        fallback = args.fallback or build_cfg.get("fallback") or "error"
        output = args.output or build_cfg.get("output")
        out_dir = args.out_dir or build_cfg.get("out_dir") or build_cfg.get("out-dir")
        sysroot = (
            args.sysroot
            or build_cfg.get("sysroot")
            or build_cfg.get("sysroot_path")
            or build_cfg.get("sysroot-path")
        )
        emit = args.emit or build_cfg.get("emit")
        emit_ir = args.emit_ir or build_cfg.get("emit_ir") or build_cfg.get("emit-ir")
        pgo_profile = (
            args.pgo_profile
            or build_cfg.get("pgo_profile")
            or build_cfg.get("pgo-profile")
        )
        runtime_feedback = (
            args.runtime_feedback
            or build_cfg.get("runtime_feedback")
            or build_cfg.get("runtime-feedback")
        )
        build_profile = (
            args.profile
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "release"
        )
        linked_output_path = (
            args.linked_output
            or build_cfg.get("linked_output")
            or build_cfg.get("linked-output")
        )
        require_linked = args.require_linked
        if require_linked is None:
            require_linked = _coerce_bool(
                build_cfg.get("require_linked") or build_cfg.get("require-linked"),
                False,
            )
        type_facts = args.type_facts or build_cfg.get("type_facts")
        deterministic = (
            args.deterministic
            if args.deterministic is not None
            else _coerce_bool(build_cfg.get("deterministic"), True)
        )
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(build_cfg.get("trusted"), False)
        linked = args.linked
        if linked is None:
            linked = _coerce_bool(build_cfg.get("linked"), False)
        cache = (
            args.cache
            if args.cache is not None
            else _coerce_bool(build_cfg.get("cache"), True)
        )
        if args.rebuild:
            cache = False
        cache_dir = (
            args.cache_dir or build_cfg.get("cache_dir") or build_cfg.get("cache-dir")
        )
        cache_report = args.cache_report or _coerce_bool(
            build_cfg.get("cache_report") or build_cfg.get("cache-report"), False
        )
        respect_pythonpath = args.respect_pythonpath
        if respect_pythonpath is None:
            respect_pythonpath = _coerce_bool(
                build_cfg.get("respect_pythonpath")
                or build_cfg.get("respect-pythonpath"),
                False,
            )
        diagnostics = args.diagnostics
        if diagnostics is None:
            diagnostics_cfg = build_cfg.get("diagnostics")
            if diagnostics_cfg is None:
                diagnostics_cfg = build_cfg.get("build_diagnostics")
            if diagnostics_cfg is None:
                diagnostics_cfg = build_cfg.get("build-diagnostics")
            if diagnostics_cfg is not None:
                diagnostics = _coerce_bool(diagnostics_cfg, False)
        diagnostics_file_raw = (
            args.diagnostics_file
            or build_cfg.get("diagnostics_file")
            or build_cfg.get("diagnostics-file")
            or build_cfg.get("build_diagnostics_file")
            or build_cfg.get("build-diagnostics-file")
        )
        diagnostics_file = (
            diagnostics_file_raw.strip()
            if isinstance(diagnostics_file_raw, str)
            else None
        )
        if diagnostics_file == "":
            diagnostics_file = None
        diagnostics_verbosity = (
            args.diagnostics_verbosity
            or build_cfg.get("diagnostics_verbosity")
            or build_cfg.get("diagnostics-verbosity")
            or build_cfg.get("build_diagnostics_verbosity")
            or build_cfg.get("build-diagnostics-verbosity")
        )
        capabilities = (
            args.capabilities or build_cfg.get("capabilities") or cfg_capabilities
        )
        if args.file and args.module:
            return _fail(
                "Use a file path or --module, not both.", args.json, command="build"
            )
        if not args.file and not args.module:
            return _fail("Missing entry file or module.", args.json, command="build")
        return build(
            args.file,
            target,
            codec,
            type_hints,
            fallback,
            type_facts,
            pgo_profile,
            runtime_feedback,
            output,
            args.json,
            args.verbose,
            deterministic,
            deterministic_warn,
            trusted,
            capabilities,
            cache,
            cache_dir,
            cache_report,
            sysroot,
            emit_ir,
            emit,
            out_dir,
            build_profile,
            linked,
            linked_output_path,
            require_linked,
            respect_pythonpath,
            args.module,
            diagnostics,
            diagnostics_file,
            diagnostics_verbosity,
            portable=getattr(args, "portable", False),
        )
    if args.command == "extension":
        if args.extension_command == "build":
            deterministic = (
                args.deterministic
                if args.deterministic is not None
                else _coerce_bool(extension_cfg.get("deterministic"), True)
            )
            capabilities = (
                args.capabilities
                or extension_cfg.get("capabilities")
                or cfg_capabilities
            )
            molt_abi = (
                args.molt_abi
                or extension_cfg.get("molt_abi")
                or extension_cfg.get("molt-abi")
            )
            return extension_build(
                project=args.project or extension_cfg.get("project"),
                out_dir=args.out_dir
                or extension_cfg.get("out_dir")
                or extension_cfg.get("out-dir"),
                molt_abi=molt_abi,
                capabilities=capabilities,
                deterministic=deterministic,
                target=args.target or extension_cfg.get("target"),
                json_output=args.json,
                verbose=args.verbose,
            )
        if args.extension_command == "audit":
            require_abi = (
                args.require_abi
                or extension_cfg.get("require_abi")
                or extension_cfg.get("require-abi")
            )
            require_capabilities = args.require_capabilities
            if not require_capabilities:
                require_capabilities = _coerce_bool(
                    extension_cfg.get("require_capabilities")
                    or extension_cfg.get("require-capabilities"),
                    False,
                )
            require_checksum = args.require_checksum
            if not require_checksum:
                require_checksum = _coerce_bool(
                    extension_cfg.get("require_checksum")
                    or extension_cfg.get("require-checksum"),
                    False,
                )
            return extension_audit(
                path=args.path,
                require_capabilities=require_capabilities,
                require_abi=require_abi,
                require_checksum=require_checksum,
                json_output=args.json,
                verbose=args.verbose,
            )
        if args.extension_command == "scan":
            fail_on_missing = args.fail_on_missing
            if not fail_on_missing:
                fail_on_missing = _coerce_bool(
                    extension_cfg.get("scan_fail_on_missing")
                    or extension_cfg.get("scan-fail-on-missing"),
                    False,
                )
            return extension_scan(
                project=args.project or extension_cfg.get("project"),
                sources=args.source,
                fail_on_missing=fail_on_missing,
                json_output=args.json,
                verbose=args.verbose,
            )
        return _fail(
            "Missing extension subcommand (build|audit|scan).",
            args.json,
            command="extension",
        )
    if args.command == "check":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(build_cfg.get("deterministic"), True)
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        return check(
            args.path,
            args.output,
            args.strict,
            args.json,
            args.verbose,
            deterministic,
            deterministic_warn,
        )
    if args.command == "run":
        build_args = _strip_leading_double_dash(args.build_arg)
        if args.rebuild and not _build_args_has_cache_flag(build_args):
            build_args.append("--no-cache")
        run_profile = (
            args.profile
            or run_cfg.get("profile")
            or run_cfg.get("build_profile")
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "dev"
        )
        if run_profile is not None and run_profile not in {"dev", "release"}:
            return _fail(
                f"Invalid run profile: {run_profile}",
                args.json,
                command="run",
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(run_cfg.get("trusted"), False)
        capabilities = (
            args.capabilities or run_cfg.get("capabilities") or cfg_capabilities
        )
        return run_script(
            args.file,
            args.module,
            _strip_leading_double_dash(args.script_args),
            args.json,
            args.verbose,
            args.timing,
            trusted,
            capabilities,
            build_args,
            cast(BuildProfile | None, run_profile),
        )
    if args.command == "compare":
        python_exe = args.python or args.python_version
        build_args = _strip_leading_double_dash(args.build_arg)
        compare_profile = (
            args.profile
            or compare_cfg.get("profile")
            or compare_cfg.get("build_profile")
            or run_cfg.get("profile")
            or run_cfg.get("build_profile")
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "dev"
        )
        if compare_profile is not None and compare_profile not in {"dev", "release"}:
            return _fail(
                f"Invalid compare profile: {compare_profile}",
                args.json,
                command="compare",
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(
                compare_cfg.get("trusted", run_cfg.get("trusted")),
                False,
            )
        capabilities = (
            args.capabilities
            or compare_cfg.get("capabilities")
            or run_cfg.get("capabilities")
            or cfg_capabilities
        )
        return compare(
            args.file,
            args.module,
            python_exe,
            _strip_leading_double_dash(args.script_args),
            args.json,
            args.verbose,
            trusted,
            capabilities,
            build_args,
            args.rebuild,
            cast(BuildProfile | None, compare_profile),
        )
    if args.command == "parity-run":
        python_exe = args.python or args.python_version
        return parity_run(
            args.file,
            args.module,
            python_exe,
            _strip_leading_double_dash(args.script_args),
            args.json,
            args.verbose,
            args.timing,
        )
    if args.command == "test":
        pytest_args = _strip_leading_double_dash(args.pytest_args)
        if args.suite == "dev" and (args.path or pytest_args) and args.verbose:
            print("Ignoring extra args for suite=dev.")
        test_profile = (
            args.profile
            or test_cfg.get("profile")
            or test_cfg.get("build_profile")
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "dev"
        )
        if test_profile is not None and test_profile not in {"dev", "release"}:
            return _fail(
                f"Invalid test profile: {test_profile}",
                args.json,
                command="test",
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(test_cfg.get("trusted"), False)
        return test(
            args.suite,
            args.path,
            args.python_version,
            pytest_args,
            cast(BuildProfile | None, test_profile),
            trusted,
            args.json,
            args.verbose,
        )
    if args.command == "diff":
        diff_profile = (
            args.profile
            or diff_cfg.get("profile")
            or diff_cfg.get("build_profile")
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "dev"
        )
        if diff_profile is not None and diff_profile not in {"dev", "release"}:
            return _fail(
                f"Invalid diff profile: {diff_profile}",
                args.json,
                command="diff",
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(diff_cfg.get("trusted"), False)
        return diff(
            args.path,
            args.python_version,
            cast(BuildProfile | None, diff_profile),
            trusted,
            args.json,
            args.verbose,
        )
    if args.command == "bench":
        return bench(
            args.wasm,
            _strip_leading_double_dash(args.bench_args),
            args.bench_script,
            args.json,
            args.verbose,
        )
    if args.command == "profile":
        return profile(
            _strip_leading_double_dash(args.profile_args),
            args.json,
            args.verbose,
        )
    if args.command == "lint":
        return lint(args.json, args.verbose)
    if args.command == "doctor":
        return doctor(args.json, args.verbose, args.strict)
    if args.command == "package":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(build_cfg.get("deterministic"), True)
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        capabilities = args.capabilities or cfg_capabilities
        sbom_enabled = args.sbom
        if sbom_enabled is None:
            sbom_enabled = True
        return package(
            args.artifact,
            args.manifest,
            args.output,
            json_output=args.json,
            verbose=args.verbose,
            deterministic=deterministic,
            deterministic_warn=deterministic_warn,
            capabilities=capabilities,
            sbom=sbom_enabled,
            sbom_output=args.sbom_output,
            sbom_format=args.sbom_format,
            signature=args.signature,
            signature_output=args.signature_output,
            sign=args.sign,
            signer=args.signer,
            signing_key=args.signing_key,
            signing_identity=args.signing_identity,
        )
    if args.command == "publish":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(
                publish_cfg.get("deterministic") or build_cfg.get("deterministic"),
                True,
            )
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                publish_cfg.get("deterministic_warn")
                or publish_cfg.get("deterministic-warn")
                or build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        explicit_require = args.require_signature is not None
        explicit_verify = args.verify_signature is not None
        require_signature = args.require_signature
        if require_signature is None:
            require_signature = _coerce_bool(
                publish_cfg.get("require_signature")
                or publish_cfg.get("require-signature")
                or os.environ.get("MOLT_REQUIRE_SIGNATURE"),
                False,
            )
        verify_signature = args.verify_signature
        if verify_signature is None:
            verify_signature = _coerce_bool(
                publish_cfg.get("verify_signature")
                or publish_cfg.get("verify-signature")
                or os.environ.get("MOLT_VERIFY_SIGNATURE"),
                False,
            )
        if explicit_require and not require_signature and not explicit_verify:
            verify_signature = False
        trusted_signers = (
            args.trusted_signers
            or publish_cfg.get("trusted_signers")
            or publish_cfg.get("trusted-signers")
            or os.environ.get("MOLT_TRUSTED_SIGNERS")
        )
        if _is_remote_registry(args.registry):
            if not explicit_require:
                require_signature = True
            if not explicit_verify and require_signature:
                verify_signature = True
            if trusted_signers is None and (require_signature or verify_signature):
                return _fail(
                    "Remote publish requires --trusted-signers or MOLT_TRUSTED_SIGNERS "
                    "(disable with --no-require-signature/--no-verify-signature).",
                    args.json,
                    command="publish",
                )
        capabilities = (
            args.capabilities or publish_cfg.get("capabilities") or cfg_capabilities
        )
        return publish(
            args.package,
            args.registry,
            args.dry_run,
            args.json,
            args.verbose,
            deterministic,
            deterministic_warn,
            capabilities,
            require_signature,
            verify_signature,
            trusted_signers,
            args.signer,
            args.signing_key,
            args.registry_token,
            args.registry_user,
            args.registry_password,
            args.registry_timeout,
        )
    if args.command == "verify":
        require_signature = args.require_signature
        if require_signature is None:
            require_signature = False
        verify_signature = args.verify_signature
        if verify_signature is None:
            verify_signature = False
        return verify(
            args.package,
            args.manifest,
            args.artifact,
            args.require_checksum,
            args.json,
            args.verbose,
            args.require_deterministic,
            args.capabilities or cfg_capabilities,
            require_signature,
            verify_signature,
            args.trusted_signers,
            args.signer,
            args.signing_key,
            args.require_extension_capabilities,
            args.require_extension_abi,
            args.extension_metadata,
        )
    if args.command == "deps":
        return deps(args.include_dev, args.json, args.verbose)
    if args.command == "vendor":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(build_cfg.get("deterministic"), True)
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        return vendor(
            args.include_dev,
            args.json,
            args.verbose,
            args.output,
            args.dry_run,
            args.allow_non_tier_a,
            args.extras,
            deterministic,
            deterministic_warn,
        )
    if args.command == "clean":
        return clean(
            args.json,
            args.verbose,
            args.cache,
            args.artifacts,
            args.bins,
            args.repo_artifacts,
            args.cargo_target,
            args.all,
            args.include_venvs,
        )
    if args.command == "config":
        return show_config(config_root, config, args.json, args.verbose)
    if args.command == "completion":
        return completion(args.shell, args.json, args.verbose)

    return 2


def _load_toml(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {}
    return tomllib.loads(path.read_text())


def _normalize_name(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def _marker_environment() -> dict[str, str]:
    version = sys.version_info
    return {
        "python_version": f"{version.major}.{version.minor}",
        "python_full_version": f"{version.major}.{version.minor}.{version.micro}",
        "os_name": os.name,
        "sys_platform": sys.platform,
        "platform_python_implementation": platform.python_implementation(),
        "platform_system": platform.system(),
        "platform_machine": platform.machine(),
        "platform_release": platform.release(),
        "platform_version": platform.version(),
        "implementation_name": sys.implementation.name,
        "implementation_version": sys.implementation.version.__str__(),
    }


def _parse_requirement(spec: str) -> tuple[str, set[str], str | None]:
    try:
        req = Requirement(spec)
    except InvalidRequirement:
        return "", set(), None
    marker = str(req.marker) if req.marker else None
    return req.name, set(req.extras), marker


def _marker_satisfied(
    marker: str,
    env: dict[str, str],
    extras: set[str],
) -> bool:
    try:
        parsed = Marker(marker)
    except InvalidMarker:
        return False
    base_env = dict(env)
    base_env.setdefault("extra", "")
    if "extra" in marker:
        if extras:
            return any(
                parsed.evaluate({**base_env, "extra": extra}) for extra in extras
            )
        return parsed.evaluate(base_env)
    return parsed.evaluate(base_env)


def _collect_dep_specs(
    pyproject: dict[str, Any],
    include_dev: bool,
    extras: set[str] | None = None,
) -> tuple[list[str], dict[str, set[str]], list[str]]:
    deps: list[str] = []
    root_extras: dict[str, set[str]] = {}
    skipped: list[str] = []
    entries: list[str] = []
    entries.extend(pyproject.get("project", {}).get("dependencies", []))
    if include_dev:
        entries.extend(pyproject.get("dependency-groups", {}).get("dev", []))
    extras = extras or set()
    optional = pyproject.get("project", {}).get("optional-dependencies", {})
    for extra in extras:
        entries.extend(optional.get(extra, []))
    env = _marker_environment()
    for entry in entries:
        name, req_extras, marker = _parse_requirement(entry)
        if not name:
            continue
        if marker and not _marker_satisfied(marker, env, extras):
            skipped.append(entry)
            continue
        norm = _normalize_name(name)
        deps.append(norm)
        if req_extras:
            root_extras.setdefault(norm, set()).update(req_extras)
    return deps, root_extras, skipped


def _collect_deps(pyproject: dict[str, Any], include_dev: bool) -> list[str]:
    deps: list[str] = []
    deps.extend(pyproject.get("project", {}).get("dependencies", []))
    if include_dev:
        deps.extend(pyproject.get("dependency-groups", {}).get("dev", []))
    return [re.split(r"[<=>\\[\\s;]", dep, 1)[0] for dep in deps]


def _lock_packages(lock: dict[str, Any]) -> dict[str, dict[str, Any]]:
    packages: dict[str, dict[str, Any]] = {}
    for pkg in lock.get("package", []):
        name = _normalize_name(pkg.get("name", ""))
        if name:
            packages[name] = pkg
    return packages


def _lock_package_graph(
    lock: dict[str, Any],
    env: dict[str, str] | None = None,
    selected_extras: dict[str, set[str]] | None = None,
) -> tuple[dict[str, dict[str, Any]], dict[str, list[str]], list[dict[str, Any]]]:
    packages: dict[str, dict[str, Any]] = {}
    deps: dict[str, list[str]] = {}
    skipped: list[dict[str, Any]] = []
    env = env or _marker_environment()
    selected_extras = selected_extras or {}
    for pkg in lock.get("package", []):
        name = _normalize_name(pkg.get("name", ""))
        if not name:
            continue
        packages[name] = pkg
        dep_names: list[str] = []
        extras = selected_extras.get(name, set())
        if isinstance(extras, list):
            extras = set(extras)
        for dep in pkg.get("dependencies", []):
            dep_name = _normalize_name(dep.get("name", ""))
            marker = dep.get("marker")
            extra = dep.get("extra")
            extra_tokens: list[str] = []
            if isinstance(extra, str):
                if extra:
                    extra_tokens = [extra]
            elif isinstance(extra, list):
                extra_tokens = [
                    item for item in extra if isinstance(item, str) and item
                ]
            if extra_tokens and extras.isdisjoint(extra_tokens):
                skipped.append(
                    {
                        "name": dep.get("name"),
                        "from": pkg.get("name"),
                        "marker": marker,
                        "extra": extra,
                    }
                )
                continue
            if marker and not _marker_satisfied(marker, env, extras):
                skipped.append(
                    {
                        "name": dep.get("name"),
                        "from": pkg.get("name"),
                        "marker": marker,
                        "extra": extra,
                    }
                )
                continue
            if dep_name:
                dep_names.append(dep_name)
        deps[name] = dep_names
    return packages, deps, skipped


def _resolve_dependency_closure(
    roots: list[str],
    deps: dict[str, list[str]],
) -> tuple[list[str], list[str]]:
    seen: set[str] = set()
    missing: list[str] = []
    queue = list(roots)
    while queue:
        name = queue.pop(0)
        if name in seen:
            continue
        seen.add(name)
        if name not in deps:
            missing.append(name)
            continue
        for child in deps.get(name, []):
            if child not in seen:
                queue.append(child)
    return sorted(seen), sorted(set(missing))


def _pick_vendor_artifact(pkg: dict[str, Any]) -> tuple[str, dict[str, Any]] | None:
    for wheel in pkg.get("wheels", []):
        url = wheel.get("url", "")
        if "py3-none-any" in url:
            return "wheel", wheel
    sdist = pkg.get("sdist")
    if sdist:
        return "sdist", sdist
    wheels = pkg.get("wheels", [])
    if wheels:
        return "wheel", wheels[0]
    return None


def _vendor_cache_path(url: str, expected_hash: str) -> Path | None:
    if not expected_hash:
        return None
    algo = "sha256"
    digest = expected_hash
    if ":" in expected_hash:
        algo, digest = expected_hash.split(":", 1)
    if not digest:
        return None
    suffixes = Path(urllib.parse.urlparse(url).path).suffixes
    suffix = "".join(suffixes) if suffixes else ""
    cache_root = _default_molt_cache() / "vendor"
    try:
        cache_root.mkdir(parents=True, exist_ok=True)
    except OSError:
        return None
    return cache_root / f"{algo}-{digest}{suffix}"


def _read_cached_artifact(cache_path: Path, expected_digest: str) -> bytes | None:
    try:
        data = cache_path.read_bytes()
    except OSError:
        return None
    digest = hashlib.sha256(data).hexdigest()
    if digest != expected_digest:
        try:
            cache_path.unlink()
        except OSError:
            pass
        return None
    return data


def _write_cached_artifact(cache_path: Path, data: bytes) -> None:
    tmp_path = cache_path.with_name(f"{cache_path.name}.tmp")
    try:
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        tmp_path.write_bytes(data)
        tmp_path.replace(cache_path)
    except OSError:
        try:
            if tmp_path.exists():
                tmp_path.unlink()
        except OSError:
            pass


def _download_artifact(url: str, expected_hash: str) -> bytes:
    if not url or not expected_hash:
        raise ValueError("missing url or hash")
    cache_path = _vendor_cache_path(url, expected_hash)
    expected = expected_hash.split(":", 1)[-1]
    if cache_path is not None:
        cached = _read_cached_artifact(cache_path, expected)
        if cached is not None:
            return cached
    with urllib.request.urlopen(url) as response:
        data = response.read()
    digest = hashlib.sha256(data).hexdigest()
    if digest != expected:
        raise ValueError("hash mismatch")
    if cache_path is not None:
        _write_cached_artifact(cache_path, data)
    return data


def _classify_tier(
    name: str,
    pkg: dict[str, Any] | None,
    allow: dict[str, set[str]],
) -> tuple[str, str]:
    norm = _normalize_name(name)
    if norm in allow["tier_a"]:
        return "Tier A", _append_feature_notes("allowlisted", pkg)
    if norm in allow["tier_b"]:
        return "Tier B", _append_feature_notes("allowlisted", pkg)
    if norm in allow["tier_c"]:
        return "Tier C", _append_feature_notes("allowlisted", pkg)
    if norm in allow["native_wheels"]:
        return "Tier B", _append_feature_notes("allowlisted native wheels", pkg)

    molt_packages = {"molt_json", "molt_msgpack", "molt_cbor"}
    if norm in molt_packages:
        return "Tier B", _append_feature_notes("molt package", pkg)
    if pkg is None:
        return "Tier A", _append_feature_notes("unresolved (assumed pure python)", pkg)
    source = pkg.get("source", {})
    if source.get("git") or source.get("path"):
        return "Tier A", _append_feature_notes("local/git source", pkg)
    wheels = pkg.get("wheels", [])
    has_universal = any("py3-none-any" in wheel.get("url", "") for wheel in wheels)
    has_abi3 = any("abi3" in wheel.get("url", "") for wheel in wheels)
    if wheels and not has_universal and not has_abi3:
        return "Tier C", _append_feature_notes("platform wheels only", pkg)
    if has_abi3 and not has_universal:
        return "Tier B", _append_feature_notes("abi3 wheels", pkg)
    if wheels:
        return "Tier A", _append_feature_notes("universal wheels", pkg)
    if pkg.get("sdist"):
        return "Tier A", _append_feature_notes("sdist only", pkg)
    return "Tier A", _append_feature_notes("assumed pure python", pkg)


def _dep_allowlists(pyproject: dict[str, Any]) -> dict[str, set[str]]:
    tool_cfg = pyproject.get("tool", {}).get("molt", {}).get("deps", {})
    return {
        "tier_a": {_normalize_name(name) for name in tool_cfg.get("tier_a", [])},
        "tier_b": {_normalize_name(name) for name in tool_cfg.get("tier_b", [])},
        "tier_c": {_normalize_name(name) for name in tool_cfg.get("tier_c", [])},
        "native_wheels": {
            _normalize_name(name) for name in tool_cfg.get("native_wheels", [])
        },
    }


def _append_feature_notes(reason: str, pkg: dict[str, Any] | None) -> str:
    if not pkg:
        return reason
    metadata = pkg.get("metadata", {})
    requires = metadata.get("requires-dist", [])
    markers = any("marker" in dep for dep in requires)
    extras = any("extra" in dep for dep in requires)
    notes: list[str] = []
    if markers:
        notes.append("markers")
    if extras:
        notes.append("extras")
    if notes:
        return f"{reason}; {', '.join(notes)}"
    return reason


def _collect_py_files(target: Path) -> list[Path]:
    if target.is_file():
        return [target]
    return sorted(path for path in target.rglob("*.py") if path.is_file())


def _run_ty_check(path: Path) -> tuple[bool, str]:
    commands = [
        ["uv", "run", "ty", "check", str(path), "--output-format", "concise"],
        ["ty", "check", str(path), "--output-format", "concise"],
    ]
    for cmd in commands:
        try:
            result = subprocess.run(cmd, capture_output=True, text=True, check=False)
        except FileNotFoundError:
            continue
        if result.returncode == 0:
            return True, result.stdout.strip()
        combined = (result.stdout + result.stderr).strip()
        return False, combined
    return False, "ty is not available; install it with `uv add ty`."


def _collect_type_facts_for_build(
    paths: list[Path], type_hint_policy: TypeHintPolicy, ty_target: Path
) -> tuple[Any | None, bool]:
    trust = "trusted" if type_hint_policy == "trust" else "guarded"
    ty_ok, _ = _run_ty_check(ty_target)
    facts = collect_type_facts_from_paths(paths, trust, infer=ty_ok)
    if ty_ok:
        facts.tool = "molt-check+ty+infer"
    return facts, ty_ok


def check(
    path: str,
    output: str,
    strict: bool,
    json_output: bool = False,
    verbose: bool = False,
    deterministic: bool = True,
    deterministic_warn: bool = False,
) -> int:
    target = Path(path)
    if not target.exists():
        return _fail(f"Path not found: {target}", json_output, command="check")
    project_root = _find_project_root(target.resolve())
    warnings: list[str] = []
    lock_error = _check_lockfiles(
        project_root,
        json_output,
        warnings,
        deterministic,
        deterministic_warn,
        "check",
    )
    if lock_error is not None:
        return lock_error
    files = _collect_py_files(target)
    if not files:
        return _fail(
            f"No Python files found under: {target}",
            json_output,
            command="check",
        )
    trust = "trusted" if strict else "guarded"
    ty_ok, ty_output = _run_ty_check(target)
    if ty_ok:
        facts = collect_type_facts_from_paths(files, trust, infer=True)
        facts.tool = "molt-check+ty+infer"
        if verbose and not json_output:
            print("ty check passed; trusting inferred hints.")
    elif ty_output:
        warnings.append(ty_output)
        if not json_output:
            print(ty_output, file=sys.stderr)
        if strict:
            return _fail(
                "ty check failed; refusing strict type facts.",
                json_output,
                command="check",
            )
        facts = collect_type_facts_from_paths(files, trust, infer=False)
    else:
        facts = collect_type_facts_from_paths(files, trust, infer=False)
    output_path = Path(output)
    write_type_facts(output_path, facts)
    if json_output:
        payload = _json_payload(
            "check",
            "ok",
            data={
                "output": str(output_path),
                "strict": strict,
                "ty_ok": ty_ok,
                "deterministic": deterministic,
            },
            warnings=warnings,
        )
        _emit_json(payload, json_output)
    else:
        print(f"Wrote type facts to {output_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
