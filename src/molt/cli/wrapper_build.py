from __future__ import annotations

import hashlib
import importlib
import json
import os
import sys
from pathlib import Path
from typing import Any, Mapping, Sequence

from molt.cli.config_resolution import (
    STATIC_IMPORT_MODULES_ENV,
    _resolve_build_config,
)
from molt.cli.file_hashing import _sha256_file
from molt.cli.json_cache import _read_cached_json_object, _write_cached_json_object
from molt.cli.models import (
    _ImportAdmissionPolicy,
    _ResolvedBuildEntry,
    _WrapperBuildContract,
)
from molt.cli.target_python import (
    TargetPythonVersion,
    _parse_target_python_version,
    _resolve_target_python_version,
)


def _cli_module() -> Any:
    return importlib.import_module("molt.cli")


def _build_args_has_json_flag(args: Sequence[str]) -> bool:
    return any(arg == "--json" for arg in args)


def _build_args_has_python_version_flag(args: Sequence[str]) -> bool:
    return any(
        arg == "--python-version" or arg.startswith("--python-version=") for arg in args
    )


def _wrapper_target_python(
    build_args: Sequence[str],
    *,
    project_root: Path,
) -> TargetPythonVersion:
    for index, arg in enumerate(build_args):
        if arg == "--python-version" and index + 1 < len(build_args):
            return _parse_target_python_version(build_args[index + 1])
        if arg.startswith("--python-version="):
            return _parse_target_python_version(arg.split("=", 1)[1])
    cli = _cli_module()
    return _resolve_target_python_version(
        explicit=None,
        build_config=_resolve_build_config(cli._load_molt_config(project_root)),
        project_root=project_root,
    )


_WRAPPER_BUILD_CACHE_SCHEMA_VERSION = 2
_WRAPPER_BUILD_CACHE_ENV_KEYS = (
    "MOLT_CAPABILITIES",
    "MOLT_CAPABILITY_TIER",
    "MOLT_EXTERNAL_STATIC_PACKAGES",
    "MOLT_HASH_SEED",
    "MOLT_HERMETIC_MODULE_ROOTS",
    "MOLT_MODULE_ROOTS",
    STATIC_IMPORT_MODULES_ENV,
    "MOLT_TRUSTED",
    "PYTHONHASHSEED",
    "PYTHONPATH",
)


def _wrapper_build_cache_manifest_path(binary_path: Path) -> Path:
    return binary_path.with_name(f"{binary_path.name}.molt-run-cache.json")


def _wrapper_build_default_binary_path(
    resolved_build_entry: _ResolvedBuildEntry,
) -> Path:
    cli = _cli_module()
    output_base = cli._output_base_for_entry(
        resolved_build_entry.entry_module,
        resolved_build_entry.source_path,
    )
    return cli._default_molt_bin() / f"{output_base}_molt"


def _wrapper_build_cache_semantic_env(env: Mapping[str, str]) -> dict[str, str]:
    return {
        key: env[key]
        for key in _WRAPPER_BUILD_CACHE_ENV_KEYS
        if key in env and env[key] != ""
    }


def _wrapper_build_dependency_fingerprints(
    *,
    resolved_build_entry: _ResolvedBuildEntry,
    project_root: Path,
    capability_config_digest: str = "",
) -> list[dict[str, Any]] | None:
    cli = _cli_module()
    stdlib_root = cli._stdlib_root_path()
    module_roots = list(resolved_build_entry.module_roots)
    roots = list(dict.fromkeys([*module_roots, stdlib_root]))
    admitted_packages, admission_error = cli._parse_external_static_packages(
        os.environ.get("MOLT_EXTERNAL_STATIC_PACKAGES", "")
    )
    if admission_error is not None:
        return None
    native_plan, native_plan_errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=resolved_build_entry.external_module_roots,
        admitted_packages=admitted_packages,
    )
    if native_plan_errors or native_plan is None:
        return None
    import_admission_policy = _ImportAdmissionPolicy(
        external_roots=resolved_build_entry.external_module_roots,
        admitted_external_packages=admitted_packages,
        native_artifact_plan=native_plan,
    )
    stdlib_allowlist = cli._stdlib_allowlist()
    resolution_cache = cli._ModuleResolutionCache()
    try:
        graph, explicit_imports = cli._discover_module_graph(
            resolved_build_entry.source_path,
            roots,
            module_roots,
            stdlib_root,
            project_root,
            stdlib_allowlist,
            resolver_cache=resolution_cache,
            import_admission_policy=import_admission_policy,
            target_python=resolved_build_entry.target_python,
            capability_config_digest=capability_config_digest,
        )
    except (OSError, SyntaxError, UnicodeDecodeError):
        return None
    static_import_modules, static_import_error = cli._parse_static_import_modules(
        os.environ.get(STATIC_IMPORT_MODULES_ENV, "")
    )
    if static_import_error is not None:
        return None
    static_import_errors = cli._extend_module_graph_with_static_import_modules(
        module_graph=graph,
        explicit_imports=explicit_imports,
        module_names=static_import_modules,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        project_root=project_root,
        stdlib_allowlist=stdlib_allowlist,
        resolver_cache=resolution_cache,
        diagnostics_enabled=False,
        module_reasons={},
        import_admission_policy=import_admission_policy,
        target_python=resolved_build_entry.target_python,
    )
    if static_import_errors:
        return None
    dependencies: list[dict[str, Any]] = []
    for module_name, path in sorted(graph.items()):
        try:
            stat = path.stat()
        except OSError:
            return None
        source_hash = cli._source_content_sha256(path, stat)
        if source_hash is None:
            return None
        dependencies.append(
            {
                "module": module_name,
                "kind": "python_source",
                "path": os.fspath(path.resolve()),
                "size": stat.st_size,
                "mtime_ns": stat.st_mtime_ns,
                "source_sha256": source_hash,
            }
        )
    for artifact in import_admission_policy.native_artifact_plan.artifacts:
        try:
            artifact_stat = artifact.path.stat()
            manifest_stat = artifact.manifest_path.stat()
        except OSError:
            return None
        dependencies.append(
            {
                "module": artifact.module,
                "kind": "native_extension",
                "path": os.fspath(artifact.path),
                "size": artifact_stat.st_size,
                "mtime_ns": artifact_stat.st_mtime_ns,
                "source_sha256": artifact.extension_sha256,
                "manifest_path": os.fspath(artifact.manifest_path),
                "manifest_size": manifest_stat.st_size,
                "manifest_mtime_ns": manifest_stat.st_mtime_ns,
                "manifest_sha256": artifact.manifest_sha256,
                "capabilities": list(artifact.capabilities),
                "abi_tag": artifact.abi_tag,
                "target_triple": artifact.target_triple,
                "platform_tag": artifact.platform_tag,
            }
        )
    return dependencies


def _wrapper_build_cache_input(
    *,
    resolved_build_entry: _ResolvedBuildEntry,
    build_args: Sequence[str],
    env: Mapping[str, str],
    project_root: Path,
) -> tuple[dict[str, Any], str] | None:
    cli = _cli_module()
    source_path = resolved_build_entry.source_path
    try:
        resolved_source_path = source_path.resolve()
    except OSError:
        resolved_source_path = source_path
    source_hash = cli._source_content_sha256(resolved_source_path)
    if source_hash is None:
        return None
    capability_config_digest = cli._capability_config_cache_digest_from_env(env)
    dependencies = _wrapper_build_dependency_fingerprints(
        resolved_build_entry=resolved_build_entry,
        project_root=project_root,
        capability_config_digest=capability_config_digest,
    )
    if dependencies is None:
        return None
    payload: dict[str, Any] = {
        "version": _WRAPPER_BUILD_CACHE_SCHEMA_VERSION,
        "source_path": os.fspath(resolved_source_path),
        "source_sha256": source_hash,
        "module_sources": dependencies,
        "entry_module": resolved_build_entry.entry_module,
        "project_root": os.fspath(project_root.resolve()),
        "build_args": list(build_args),
        "semantic_env": _wrapper_build_cache_semantic_env(env),
        "capability_config_digest": capability_config_digest,
        "runtime_backend_fingerprint": cli._cache_fingerprint(),
        "frontend_tooling_fingerprint": cli._cache_tooling_fingerprint(),
        "python_cache_tag": sys.implementation.cache_tag,
        "target_python": _wrapper_target_python(
            build_args,
            project_root=project_root,
        ).tag,
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return payload, hashlib.sha256(encoded).hexdigest()


def _read_wrapper_build_cache_contract(
    *,
    resolved_build_entry: _ResolvedBuildEntry | None,
    build_args: Sequence[str],
    env: Mapping[str, str],
    project_root: Path,
) -> _WrapperBuildContract | None:
    if resolved_build_entry is None:
        return None
    cache_input = _wrapper_build_cache_input(
        resolved_build_entry=resolved_build_entry,
        build_args=build_args,
        env=env,
        project_root=project_root,
    )
    if cache_input is None:
        return None
    _payload, cache_key = cache_input
    cached_bin = _wrapper_build_default_binary_path(resolved_build_entry)
    manifest_path = _wrapper_build_cache_manifest_path(cached_bin)
    manifest = _read_cached_json_object(manifest_path)
    if (
        not isinstance(manifest, dict)
        or manifest.get("version") != _WRAPPER_BUILD_CACHE_SCHEMA_VERSION
        or manifest.get("cache_key") != cache_key
        or manifest.get("consumer_output") != os.fspath(cached_bin)
    ):
        return None
    if not cached_bin.exists():
        return None
    expected_binary_hash = manifest.get("binary_sha256")
    if not isinstance(expected_binary_hash, str):
        return None
    try:
        actual_binary_hash = _sha256_file(cached_bin)
    except OSError:
        return None
    if actual_binary_hash != expected_binary_hash:
        return None
    raw_output = manifest.get("output")
    if not isinstance(raw_output, str):
        return None
    artifacts: dict[str, Path] = {}
    raw_artifacts = manifest.get("artifacts")
    if isinstance(raw_artifacts, dict):
        for key, value in raw_artifacts.items():
            if isinstance(key, str) and isinstance(value, str):
                artifacts[key] = Path(value)
    bundle_root = manifest.get("bundle_root")
    return _WrapperBuildContract(
        output=Path(raw_output),
        consumer_output=cached_bin,
        bundle_root=Path(bundle_root) if isinstance(bundle_root, str) else None,
        artifacts=artifacts,
    )


def _write_wrapper_build_cache_manifest(
    *,
    resolved_build_entry: _ResolvedBuildEntry | None,
    build_args: Sequence[str],
    env: Mapping[str, str],
    project_root: Path,
    contract: _WrapperBuildContract,
) -> None:
    if resolved_build_entry is None:
        return
    cached_bin = _wrapper_build_default_binary_path(resolved_build_entry)
    try:
        if contract.consumer_output.resolve() != cached_bin.resolve():
            return
    except OSError:
        return
    cache_input = _wrapper_build_cache_input(
        resolved_build_entry=resolved_build_entry,
        build_args=build_args,
        env=env,
        project_root=project_root,
    )
    if cache_input is None:
        return
    input_payload, cache_key = cache_input
    try:
        binary_hash = _sha256_file(cached_bin)
    except OSError:
        return
    manifest: dict[str, Any] = {
        "version": _WRAPPER_BUILD_CACHE_SCHEMA_VERSION,
        "cache_key": cache_key,
        "input": input_payload,
        "binary_sha256": binary_hash,
        "output": os.fspath(contract.output),
        "consumer_output": os.fspath(contract.consumer_output),
        "bundle_root": os.fspath(contract.bundle_root)
        if contract.bundle_root is not None
        else None,
        "artifacts": {
            key: os.fspath(value) for key, value in sorted(contract.artifacts.items())
        },
    }
    _write_cached_json_object(
        _wrapper_build_cache_manifest_path(cached_bin),
        manifest,
    )
