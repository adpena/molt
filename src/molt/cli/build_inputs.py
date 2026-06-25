from __future__ import annotations

from collections.abc import Mapping, MutableMapping, Sequence
import functools
import hashlib
import json
import os
from pathlib import Path
import time
import tomllib
import tracemalloc
from typing import Any

from molt.capability_manifest import load_manifest
from molt.cli.build_diagnostics import (
    _build_allocation_diagnostics_enabled,
    _build_diagnostics_enabled,
    _resolve_build_diagnostics_verbosity,
)
from molt.cli.build_output_layout import _resolve_sysroot
from molt.cli.capability_spec import CapabilityInput, _parse_capabilities
from molt.cli.cargo_profiles import (
    _resolve_backend_cargo_profile_name,
    _resolve_backend_profile,
    _resolve_cargo_profile_name,
)
from molt.cli.command_runtime import _resolve_timeout_env
from molt.cli.config_resolution import _coerce_bool, _resolve_build_config
from molt.cli.lockfiles import _check_lockfiles
from molt.cli.models import (
    BinaryImageKind,
    BuildProfile,
    PgoProfileSummary,
    RuntimeFeedbackSummary,
    Target,
    _BinaryImageScope,
    _ModuleRootResolution,
    _PreparedBuildConfig,
    _PreparedBuildPreamble,
    _PreparedBuildRoots,
    _ResolvedBuildEntry,
)
from molt.cli.module_graph_discovery import _record_module_reason
from molt.cli.module_import_scanner import _infer_module_overrides, _spec_parent
from molt.cli.module_resolution import (
    _entry_module_root_for_path,
    _is_stdlib_resolved_path,
    _module_name_from_path,
    _resolve_module_path,
    _stdlib_root_path,
)
from molt.cli.module_source import _read_module_source
from molt.cli.output import CliFailure as _CliFailure, fail as _fail
from molt.cli.profile_feedback import _load_pgo_profile, _load_runtime_feedback
from molt.cli.project_roots import (
    _find_molt_root,
    _find_project_root,
    _has_project_markers,
    _require_molt_root,
)
from molt.cli.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
    _parse_source_for_target,
    _resolve_target_python_version,
)


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

def _build_config_entry_value(
    build_config: Mapping[str, Any] | None,
    keys: Sequence[str],
) -> tuple[str | None, str | None]:
    if not build_config:
        return None, None
    for key in keys:
        raw = build_config.get(key)
        if raw is None:
            continue
        if not isinstance(raw, str):
            return None, f"[tool.molt.build] {key} must be a string"
        value = raw.strip()
        if not value:
            return None, f"[tool.molt.build] {key} must not be empty"
        return value, key
    return None, None

def _entry_value_looks_like_path(value: str) -> bool:
    path_markers = ("/", "\\")
    return (
        value.endswith(".py")
        or value.startswith(".")
        or any(marker in value for marker in path_markers)
    )

def _configured_build_entry_selector(
    build_config: Mapping[str, Any] | None,
) -> tuple[str | None, str | None, str | None, str | None]:
    entry_file, entry_file_key = _build_config_entry_value(
        build_config,
        (
            "entry-file",
            "entry_file",
            "entry-script",
            "entry_script",
            "script",
            "file",
        ),
    )
    if entry_file_key is not None and entry_file is None:
        return None, None, None, entry_file_key
    entry_module, entry_module_key = _build_config_entry_value(
        build_config,
        ("entry-module", "entry_module", "module"),
    )
    if entry_module_key is not None and entry_module is None:
        return None, None, None, entry_module_key
    generic_entry, generic_entry_key = _build_config_entry_value(
        build_config,
        ("entry", "main"),
    )
    if generic_entry_key is not None and generic_entry is None:
        return None, None, None, generic_entry_key
    selectors = [
        key
        for key, value in (
            (entry_file_key, entry_file),
            (entry_module_key, entry_module),
            (generic_entry_key, generic_entry),
        )
        if key is not None and value is not None
    ]
    if len(selectors) > 1:
        return (
            None,
            None,
            None,
            "Build config has multiple entry selectors; use exactly one of "
            "entry-file, entry-module, or entry.",
        )
    if entry_file is not None:
        return entry_file, None, f"config:{entry_file_key}", None
    if entry_module is not None:
        return None, entry_module, f"config:{entry_module_key}", None
    if generic_entry is not None and generic_entry_key is not None:
        if _entry_value_looks_like_path(generic_entry):
            return generic_entry, None, f"config:{generic_entry_key}:file", None
        return None, generic_entry, f"config:{generic_entry_key}:module", None
    return None, None, None, None

def _resolve_build_entry_selector(
    *,
    file_path: str | None,
    module: str | None,
    project_root: Path,
    build_config: Mapping[str, Any] | None,
) -> tuple[str | None, str | None, str | None, str | None]:
    if file_path and module:
        return None, None, None, "Use a file path or --module, not both."
    if file_path:
        return file_path, None, "cli:file", None
    if module:
        return None, module, "cli:module", None
    configured_file, configured_module, source, error = _configured_build_entry_selector(
        build_config
    )
    if error is not None:
        return None, None, None, error
    if configured_file is not None:
        path = Path(configured_file).expanduser()
        if not path.is_absolute():
            path = project_root / path
        return os.fspath(path), None, source, None
    if configured_module is not None:
        return None, configured_module, source, None
    return (
        None,
        None,
        None,
        "Missing entry file or module. Provide a Python file, --module, or "
        "[tool.molt.build] entry-file/entry-module.",
    )

def _binary_image_kind(
    *,
    selector_source: str,
    source_path: Path,
) -> BinaryImageKind:
    config_selected = selector_source.startswith("config:")
    module_selected = selector_source.endswith(":module") or selector_source in {
        "config:entry-module",
        "config:entry_module",
        "config:module",
    }
    if module_selected and source_path.name == "__main__.py":
        return "project_entry_package" if config_selected else "entry_package"
    if module_selected:
        return "project_entry_module" if config_selected else "entry_module"
    return "project_entry_script" if config_selected else "entry_script"

def _resolve_build_entry(
    *,
    file_path: str | None,
    module: str | None,
    project_root: Path,
    cwd_root: Path,
    stdlib_root: Path,
    respect_pythonpath: bool,
    json_output: bool,
    command: str = "build",
    lib_paths: list[str] | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    build_config: Mapping[str, Any] | None = None,
) -> tuple[_ResolvedBuildEntry | None, _CliFailure | None]:
    file_path, module, selector_source, selector_error = _resolve_build_entry_selector(
        file_path=file_path,
        module=module,
        project_root=project_root,
        build_config=build_config,
    )
    if selector_error is not None:
        return None, _fail(selector_error, json_output, command=command)
    assert selector_source is not None
    module_root_resolution = _resolve_module_root_resolution(
        project_root,
        cwd_root,
        respect_pythonpath=respect_pythonpath,
        lib_paths=lib_paths or [],
    )
    module_roots = list(module_root_resolution.roots)
    external_module_roots = tuple(module_root_resolution.external_roots)
    source_path: Path | None = None
    entry_module: str | None = None
    if file_path:
        source_path = Path(file_path).resolve()
        if not source_path.exists():
            return None, _fail(
                f"File not found: {source_path}", json_output, command=command
            )
        module_roots.append(_entry_module_root_for_path(source_path))
        module_roots = list(dict.fromkeys(root.resolve() for root in module_roots))
    if module:
        resolved = _resolve_entry_module(module, module_roots)
        if resolved is None:
            return None, _fail(
                f"Entry module not found: {module}",
                json_output,
                command=command,
            )
        entry_module, source_path = resolved
        module_roots.append(source_path.parent.resolve())
        module_roots = list(dict.fromkeys(module_roots))
    elif source_path is not None:
        entry_module = _module_name_from_path(source_path, module_roots, stdlib_root)
    if source_path is None or entry_module is None:
        return None, _fail(
            "No entry point found. Provide a Python file path or use --module to specify a package entry point.",
            json_output,
            command=command,
        )
    try:
        entry_source = _read_module_source(source_path)
    except (SyntaxError, UnicodeDecodeError) as exc:
        return None, _fail(
            f"Syntax error in {source_path}: {exc}",
            json_output,
            command=command,
        )
    except OSError as exc:
        return None, _fail(
            f"Failed to read entry module {source_path}: {exc}",
            json_output,
            command=command,
        )
    try:
        entry_tree = _parse_source_for_target(
            entry_source,
            filename=str(source_path),
            target_python=target_python,
        )
    except SyntaxError as exc:
        return None, _fail(
            f"Syntax error in {source_path}: {exc}",
            json_output,
            command=command,
        )
    (
        entry_pkg_override_set,
        entry_pkg_override,
        entry_spec_override_set,
        entry_spec_override,
        entry_spec_override_is_package,
    ) = _infer_module_overrides(entry_tree)
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
    return _ResolvedBuildEntry(
        source_path=source_path,
        entry_module=entry_module,
        module_roots=list(dict.fromkeys(root.resolve() for root in module_roots)),
        entry_source=entry_source,
        entry_tree=entry_tree,
        target_python=target_python,
        external_module_roots=external_module_roots,
        image_scope=_BinaryImageScope.from_entry(
            kind=_binary_image_kind(
                selector_source=selector_source,
                source_path=source_path,
            ),
            selector_source=selector_source,
            entry_module=entry_module,
            source_path=source_path,
            project_root=project_root,
            module_roots=module_roots,
        ),
    ), None

def _prepare_build_config(
    *,
    project_root: Path,
    warnings: list[str],
    json_output: bool,
    profile: BuildProfile,
    pgo_profile: str | None,
    runtime_feedback: str | None,
    capabilities: CapabilityInput | None,
    capability_manifest: str | None = None,
    require_signed_manifest: bool = False,
    python_version: str | None = None,
    build_config: Mapping[str, Any] | None = None,
) -> tuple[_PreparedBuildConfig | None, _CliFailure | None]:
    try:
        target_python = _resolve_target_python_version(
            explicit=python_version,
            build_config=build_config,
            project_root=project_root,
        )
    except ValueError as exc:
        return None, _fail(str(exc), json_output, command="build")
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
            return None, err
        pgo_profile_summary = summary
        pgo_profile_path = resolved
    if pgo_profile_summary is not None:
        pgo_hot_function_names = {
            symbol.strip()
            for symbol in pgo_profile_summary.hot_functions
            if isinstance(symbol, str) and symbol.strip()
        }
    if runtime_feedback:
        summary, resolved, err = _load_runtime_feedback(
            project_root,
            runtime_feedback,
            warnings,
            json_output,
            command="build",
        )
        if err is not None:
            return None, err
        runtime_feedback_summary = summary
        runtime_feedback_path = resolved
    if runtime_feedback_summary is not None:
        pgo_hot_function_names.update(
            symbol.strip()
            for symbol in runtime_feedback_summary.hot_functions
            if isinstance(symbol, str) and symbol.strip()
        )
    pgo_hot_function_names_sorted = tuple(sorted(pgo_hot_function_names))
    pgo_profile_payload: dict[str, Any] | None = None
    if pgo_profile_summary is not None and pgo_profile_path is not None:
        pgo_profile_payload = {
            "path": str(pgo_profile_path),
            "version": pgo_profile_summary.version,
            "hash": pgo_profile_summary.hash,
            "hot_functions": pgo_profile_summary.hot_functions,
        }
        if pgo_profile_summary.branch_counts:
            pgo_profile_payload["branch_counts"] = pgo_profile_summary.branch_counts
        if pgo_profile_summary.call_counts:
            pgo_profile_payload["call_counts"] = pgo_profile_summary.call_counts
        if pgo_profile_summary.loop_counts:
            pgo_profile_payload["loop_counts"] = pgo_profile_summary.loop_counts
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
        return None, _fail(timeout_err, json_output, command="build")
    backend_timeout, timeout_err = _resolve_timeout_env("MOLT_BACKEND_TIMEOUT")
    if timeout_err:
        return None, _fail(timeout_err, json_output, command="build")
    link_timeout, timeout_err = _resolve_timeout_env("MOLT_LINK_TIMEOUT")
    if timeout_err:
        return None, _fail(timeout_err, json_output, command="build")
    frontend_phase_timeout, timeout_err = _resolve_timeout_env(
        "MOLT_FRONTEND_PHASE_TIMEOUT"
    )
    if timeout_err:
        return None, _fail(timeout_err, json_output, command="build")

    backend_profile, profile_err = _resolve_backend_profile(profile)
    if profile_err:
        return None, _fail(profile_err, json_output, command="build")
    runtime_cargo_profile, runtime_profile_err = _resolve_cargo_profile_name(profile)
    if runtime_profile_err:
        return None, _fail(runtime_profile_err, json_output, command="build")
    backend_cargo_profile, backend_profile_err = _resolve_backend_cargo_profile_name(
        backend_profile
    )
    if backend_profile_err:
        return None, _fail(backend_profile_err, json_output, command="build")

    capabilities_list: list[str] | None = None
    capabilities_source = None
    capability_profiles: list[str] = []
    if capabilities is not None:
        parsed, profiles, source, errors = _parse_capabilities(capabilities)
        if errors:
            return None, _fail(
                "Invalid capabilities: " + ", ".join(errors),
                json_output,
                command="build",
            )
        capabilities_list = parsed
        capability_profiles = profiles
        capabilities_source = source

    # Load capability manifest if --capability-manifest was provided
    manifest_env_vars: dict[str, str] = {}
    if capability_manifest is not None:
        try:
            manifest = load_manifest(
                capability_manifest, require_signed=require_signed_manifest
            )
            manifest_env_vars = manifest.to_env_vars()
            # Merge manifest capabilities with --capabilities flag
            if capabilities_list is None:
                capabilities_list = sorted(manifest.effective_capabilities())
                capabilities_source = str(capability_manifest)
            else:
                # CLI --capabilities takes precedence; manifest adds
                manifest_caps = manifest.effective_capabilities()
                merged = sorted(set(capabilities_list) | manifest_caps)
                capabilities_list = merged
        except Exception as e:
            return None, _fail(
                f"Invalid capability manifest: {e}",
                json_output,
                command="build",
            )
    capability_config_cache_digest = _capability_config_cache_digest(
        capabilities_list=capabilities_list,
        capability_profiles=capability_profiles,
        manifest_env_vars=manifest_env_vars,
    )

    return _PreparedBuildConfig(
        pgo_profile_summary=pgo_profile_summary,
        pgo_profile_path=pgo_profile_path,
        runtime_feedback_summary=runtime_feedback_summary,
        runtime_feedback_path=runtime_feedback_path,
        pgo_hot_function_names=pgo_hot_function_names,
        pgo_hot_function_names_sorted=pgo_hot_function_names_sorted,
        pgo_profile_payload=pgo_profile_payload,
        runtime_feedback_payload=runtime_feedback_payload,
        cargo_timeout=cargo_timeout,
        backend_timeout=backend_timeout,
        link_timeout=link_timeout,
        frontend_phase_timeout=frontend_phase_timeout,
        backend_profile=backend_profile,
        runtime_cargo_profile=runtime_cargo_profile,
        backend_cargo_profile=backend_cargo_profile,
        capabilities_list=capabilities_list,
        capability_profiles=capability_profiles,
        capabilities_source=capabilities_source,
        manifest_env_vars=manifest_env_vars,
        capability_config_cache_digest=capability_config_cache_digest,
        target_python=target_python,
    ), None

def _prepare_build_preamble(
    *,
    diagnostics: bool | None,
    diagnostics_file: str | None,
    diagnostics_verbosity: str | None,
    json_output: bool,
    target: Target,
) -> tuple[_PreparedBuildPreamble | None, _CliFailure | None]:
    diagnostics_path_spec = (
        diagnostics_file.strip() if isinstance(diagnostics_file, str) else ""
    )
    diagnostics_enabled = (
        _build_diagnostics_enabled() if diagnostics is None else diagnostics
    )
    if diagnostics is False and diagnostics_path_spec:
        return None, _fail(
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
    return _PreparedBuildPreamble(
        diagnostics_path_spec=diagnostics_path_spec,
        diagnostics_enabled=diagnostics_enabled,
        resolved_diagnostics_verbosity=resolved_diagnostics_verbosity,
        allocation_diagnostics_enabled=allocation_diagnostics_enabled,
        frontend_timing_raw=frontend_timing_raw,
        frontend_timing_enabled=frontend_timing_enabled,
        frontend_timing_threshold=frontend_timing_threshold,
        frontend_module_timings=frontend_module_timings,
        midend_policy_outcomes_by_function=midend_policy_outcomes_by_function,
        midend_pass_stats_by_function=midend_pass_stats_by_function,
        frontend_parallel_details=frontend_parallel_details,
        diagnostics_start=diagnostics_start,
        phase_starts=phase_starts,
        backend_daemon_health=backend_daemon_health,
        backend_daemon_cached=backend_daemon_cached,
        backend_daemon_cache_tier=backend_daemon_cache_tier,
        backend_daemon_config_digest=backend_daemon_config_digest,
        module_reasons=module_reasons,
        stdlib_root=stdlib_root,
        warnings=warnings,
        native_arch_perf_enabled=native_arch_perf_enabled,
    ), None

def _prepare_build_roots(
    *,
    file_path: str | None,
    json_output: bool,
    warnings: list[str],
    deterministic: bool,
    deterministic_warn: bool,
    sysroot: str | None,
) -> tuple[_PreparedBuildRoots | None, _CliFailure | None]:
    cwd_root = _find_project_root(Path.cwd())
    project_root = (
        _find_project_root(Path(file_path).resolve()) if file_path else cwd_root
    )
    if not _has_project_markers(project_root) and _has_project_markers(cwd_root):
        project_root = cwd_root
    molt_root = _find_molt_root(project_root, cwd_root)
    root_error = _require_molt_root(molt_root, json_output, "build")
    if root_error is not None:
        return None, root_error
    lock_error = _check_lockfiles(
        molt_root,
        json_output,
        warnings,
        deterministic,
        deterministic_warn,
        "build",
    )
    if lock_error is not None:
        return None, lock_error
    sysroot_path = _resolve_sysroot(project_root, sysroot)
    if sysroot_path is not None and not sysroot_path.exists():
        return None, _fail(
            f"Sysroot not found: {sysroot_path}",
            json_output,
            command="build",
        )
    return _PreparedBuildRoots(
        cwd_root=cwd_root,
        project_root=project_root,
        molt_root=molt_root,
        sysroot_path=sysroot_path,
    ), None

def _prepare_build_inputs(
    *,
    file_path: str | None,
    module: str | None,
    diagnostics: bool | None,
    diagnostics_file: str | None,
    diagnostics_verbosity: str | None,
    json_output: bool,
    target: Target,
    deterministic: bool,
    deterministic_warn: bool,
    sysroot: str | None,
    profile: BuildProfile,
    pgo_profile: str | None,
    runtime_feedback: str | None,
    capabilities: CapabilityInput | None,
    capability_manifest: str | None = None,
    require_signed_manifest: bool = False,
    respect_pythonpath: bool = False,
    lib_paths: list[str] | None = None,
    python_version: str | None = None,
    build_config: Mapping[str, Any] | None = None,
) -> tuple[
    tuple[
        _PreparedBuildPreamble,
        _PreparedBuildRoots,
        _PreparedBuildConfig,
        _ResolvedBuildEntry,
    ]
    | None,
    _CliFailure | None,
]:
    prepared_build_preamble, prepared_build_preamble_error = _prepare_build_preamble(
        diagnostics=diagnostics,
        diagnostics_file=diagnostics_file,
        diagnostics_verbosity=diagnostics_verbosity,
        json_output=json_output,
        target=target,
    )
    if prepared_build_preamble_error is not None:
        return None, prepared_build_preamble_error
    assert prepared_build_preamble is not None

    prepared_build_roots, prepared_build_roots_error = _prepare_build_roots(
        file_path=file_path,
        json_output=json_output,
        warnings=prepared_build_preamble.warnings,
        deterministic=deterministic,
        deterministic_warn=deterministic_warn,
        sysroot=sysroot,
    )
    if prepared_build_roots_error is not None:
        return None, prepared_build_roots_error
    assert prepared_build_roots is not None

    prepared_build_config, prepared_build_config_error = _prepare_build_config(
        project_root=prepared_build_roots.project_root,
        warnings=prepared_build_preamble.warnings,
        json_output=json_output,
        profile=profile,
        pgo_profile=pgo_profile,
        runtime_feedback=runtime_feedback,
        capabilities=capabilities,
        capability_manifest=capability_manifest,
        require_signed_manifest=require_signed_manifest,
        python_version=python_version,
        build_config=build_config,
    )
    if prepared_build_config_error is not None:
        return None, prepared_build_config_error
    assert prepared_build_config is not None

    resolved_build_entry, resolved_build_entry_error = _resolve_build_entry(
        file_path=file_path,
        module=module,
        project_root=prepared_build_roots.project_root,
        cwd_root=prepared_build_roots.cwd_root,
        stdlib_root=prepared_build_preamble.stdlib_root,
        respect_pythonpath=respect_pythonpath,
        json_output=json_output,
        lib_paths=lib_paths or [],
        target_python=prepared_build_config.target_python,
        build_config=build_config,
    )
    if resolved_build_entry_error is not None:
        return None, resolved_build_entry_error
    assert resolved_build_entry is not None

    return (
        prepared_build_preamble,
        prepared_build_roots,
        prepared_build_config,
        resolved_build_entry,
    ), None

def _resolve_module_root_resolution(
    project_root: Path,
    cwd_root: Path,
    *,
    respect_pythonpath: bool,
    lib_paths: list[str] | None = None,
) -> _ModuleRootResolution:
    module_roots: list[Path] = []
    external_roots: list[Path] = []
    internal_roots: set[Path] = set()

    def add_root(path: Path, *, external: bool) -> None:
        resolved = path.resolve()
        module_roots.append(resolved)
        if external and resolved not in internal_roots:
            external_roots.append(resolved)
        if not external:
            internal_roots.add(resolved)

    hermetic_module_roots = os.environ.get(
        "MOLT_HERMETIC_MODULE_ROOTS", ""
    ).lower() in {
        "1",
        "true",
        "yes",
        "on",
    }
    extra_roots = os.environ.get("MOLT_MODULE_ROOTS", "")
    if extra_roots:
        for entry in extra_roots.split(os.pathsep):
            if not entry:
                continue
            entry_path = Path(entry).expanduser()
            if entry_path.exists():
                add_root(entry_path, external=True)
    # Deferred import: env_paths and build_inputs are both reachable during
    # molt.cli package initialization in an order where env_paths is still
    # partially initialized when build_inputs is first imported (a true import
    # cycle only in the molt-build order; the isolated import is fine). These
    # leaf path-utilities are needed only at call time, so importing them here
    # rather than at module top keeps build_inputs import-order-independent.
    from molt.cli.env_paths import _molt_venv_site_packages, _vendor_roots

    for root in (project_root, cwd_root):
        if root.exists():
            add_root(root, external=False)
        src_root = root / "src"
        if src_root.exists():
            add_root(src_root, external=False)
        for vendor_root in _vendor_roots(root):
            add_root(vendor_root, external=False)
    if respect_pythonpath:
        pythonpath = os.environ.get("PYTHONPATH", "")
        if pythonpath:
            for entry in pythonpath.split(os.pathsep):
                if not entry:
                    continue
                entry_path = Path(entry).expanduser()
                if entry_path.exists():
                    add_root(entry_path, external=True)
    # --lib-path / [tool.molt] lib-paths: explicit third-party package roots
    for lp in lib_paths or []:
        lp_path = Path(lp).expanduser()
        if lp_path.exists():
            add_root(lp_path, external=True)
    # Auto-detect active venv site-packages when no explicit lib paths given
    if not lib_paths and not hermetic_module_roots:
        venv_path = project_root / ".venv"
        if venv_path.exists():
            for sp in sorted(venv_path.glob("lib/python*/site-packages")):
                if sp.is_dir():
                    add_root(sp, external=True)
    # Auto-detect .molt-venv site-packages (UV-managed venv)
    if not hermetic_module_roots:
        for sp in _molt_venv_site_packages(project_root):
            add_root(sp, external=True)
    roots = tuple(dict.fromkeys(module_roots))
    external = tuple(
        root
        for root in dict.fromkeys(external_roots)
        if root in roots and root not in internal_roots
    )
    return _ModuleRootResolution(roots=roots, external_roots=external)

def _resolve_module_roots(
    project_root: Path,
    cwd_root: Path,
    *,
    respect_pythonpath: bool,
    lib_paths: list[str] | None = None,
) -> list[Path]:
    return list(
        _resolve_module_root_resolution(
            project_root,
            cwd_root,
            respect_pythonpath=respect_pythonpath,
            lib_paths=lib_paths,
        ).roots
    )

def _build_args_respect_pythonpath(args: list[str]) -> bool:
    if any(arg == "--no-respect-pythonpath" for arg in args):
        return False
    return any(arg == "--respect-pythonpath" for arg in args)

def _build_args_lib_paths(args: Sequence[str]) -> list[str]:
    lib_paths: list[str] = []
    idx = 0
    while idx < len(args):
        arg = args[idx]
        if arg == "--lib-path":
            next_idx = idx + 1
            if next_idx < len(args):
                lib_paths.append(args[next_idx])
                idx = next_idx + 1
                continue
        elif arg.startswith("--lib-path="):
            value = arg.split("=", 1)[1].strip()
            if value:
                lib_paths.append(value)
        idx += 1
    return lib_paths

def _resolve_wrapper_build_entry(
    *,
    file_path: str | None,
    module: str | None,
    project_root: Path,
    json_output: bool,
    command: str,
    build_args: Sequence[str] = (),
) -> tuple[_ResolvedBuildEntry | None, _CliFailure | None]:
    config = _load_molt_config(project_root)
    build_cfg = _resolve_build_config(config)
    respect_pythonpath = _build_args_respect_pythonpath(list(build_args))
    if not any(
        arg in {"--respect-pythonpath", "--no-respect-pythonpath"} for arg in build_args
    ):
        respect_pythonpath = _coerce_bool(
            build_cfg.get("respect_pythonpath") or build_cfg.get("respect-pythonpath"),
            False,
        )
    cfg_lib_paths = build_cfg.get("lib_paths") or build_cfg.get("lib-paths") or []
    if isinstance(cfg_lib_paths, str):
        cfg_lib_paths = [cfg_lib_paths]
    lib_paths = _build_args_lib_paths(build_args) + list(cfg_lib_paths)
    from molt.cli.wrapper_build import _wrapper_target_python

    try:
        target_python = _wrapper_target_python(build_args, project_root=project_root)
    except ValueError as exc:
        return None, _fail(str(exc), json_output, command=command)
    cwd_root = _find_project_root(Path.cwd())
    return _resolve_build_entry(
        file_path=file_path,
        module=module,
        project_root=project_root,
        cwd_root=cwd_root,
        stdlib_root=_stdlib_root_path(),
        respect_pythonpath=respect_pythonpath,
        json_output=json_output,
        command=command,
        lib_paths=lib_paths or None,
        target_python=target_python,
        build_config=build_cfg,
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

def _is_stdlib_path(path: Path, stdlib_root: Path) -> bool:
    resolved = path.resolve()
    resolved_stdlib_root = stdlib_root.resolve()
    return _is_stdlib_resolved_path(resolved, resolved_stdlib_root)

def _merge_module_graph_with_reason(
    module_graph: MutableMapping[str, Path],
    additions: Mapping[str, Path],
    module_reasons: MutableMapping[str, set[str]],
    reason: str,
) -> None:
    for name, path in additions.items():
        _record_module_reason(module_reasons, name, reason)
        module_graph.setdefault(name, path)

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

_VALID_AUDIT_SINKS = frozenset({"jsonl", "stderr", "null", "buffered"})

def _parse_audit_log_flag(value: str) -> dict[str, str]:
    """Parse --audit-log flag value into environment variables.

    Format: SINK:OUTPUT (e.g., 'jsonl:stderr', 'stderr:stderr', 'jsonl:logs/audit.log')
    """
    parts = value.split(":", 1)
    sink = parts[0]
    if sink not in _VALID_AUDIT_SINKS:
        raise ValueError(
            f"Invalid audit sink: {sink!r}. "
            f"Must be one of: {', '.join(sorted(_VALID_AUDIT_SINKS))}"
        )
    output = parts[1] if len(parts) > 1 else "stderr"
    return {
        "MOLT_AUDIT_ENABLED": "1",
        "MOLT_AUDIT_SINK": sink,
        "MOLT_AUDIT_OUTPUT": output,
    }

def _parse_io_mode_flag(value: str) -> dict[str, str]:
    """Parse --io-mode flag value into environment variables.

    Valid values: real, virtual, callback
    """
    if value not in ("real", "virtual", "callback"):
        raise ValueError(
            f"Invalid IO mode: {value!r}. Must be one of: real, virtual, callback"
        )
    env: dict[str, str] = {}
    if value != "real":
        env["MOLT_IO_MODE"] = value
    return env

def _parse_type_gate_flag(enabled: bool) -> dict[str, str]:
    """Propagate --type-gate to the backend via environment variable."""
    if enabled:
        return {"MOLT_TYPE_GATE": "1"}
    return {}

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

def _capability_ambient_env_for_cache(env: Mapping[str, str]) -> dict[str, str]:
    return {
        key: value
        for key, value in sorted(env.items())
        if key in {"MOLT_CAPABILITIES", "MOLT_CAPABILITY_TIER", "MOLT_IO_MODE"}
        or key.startswith("MOLT_RESOURCE_")
        or key.startswith("MOLT_AUDIT_")
    }

def _capability_config_cache_digest_from_env(env: Mapping[str, str]) -> str:
    ambient_env = _capability_ambient_env_for_cache(env)
    if not ambient_env:
        return ""
    payload = {
        "ambient_env": ambient_env,
        "capabilities": None,
        "capability_profiles": [],
        "manifest_env": {},
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()

def _capability_config_cache_digest(
    *,
    capabilities_list: Sequence[str] | None,
    capability_profiles: Sequence[str] | None,
    manifest_env_vars: Mapping[str, str] | None,
) -> str:
    ambient_env = _capability_ambient_env_for_cache(os.environ)
    if (
        capabilities_list is None
        and not capability_profiles
        and not manifest_env_vars
        and not ambient_env
    ):
        return ""
    payload = {
        "ambient_env": ambient_env,
        "capabilities": (
            sorted(str(capability) for capability in capabilities_list)
            if capabilities_list is not None
            else None
        ),
        "capability_profiles": sorted(
            str(profile) for profile in (capability_profiles or ())
        ),
        "manifest_env": {
            str(key): str(value)
            for key, value in sorted((manifest_env_vars or {}).items())
        },
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()

def _append_rustflags(env: MutableMapping[str, str], flags: str) -> None:
    existing = env.get("RUSTFLAGS", "")
    joined = f"{existing} {flags}".strip()
    env["RUSTFLAGS"] = joined
