from __future__ import annotations

import contextlib
import hashlib
import json
import os
import shlex
import sys
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Iterator, Mapping, Sequence

from molt.cli import runtime_build as _runtime_build
from molt.cli.backend_diagnostics import (
    _env_requests_backend_diagnostics,
    _forward_compilation_warnings,
)
from molt.cli.build_diagnostics import _emit_build_diagnostics
from molt.cli.command_runtime import _CLI_MEMORY_GUARD_PREFIX
from molt.cli.config_resolution import (
    STATIC_IMPORT_MODULES_ENV,
    _resolve_build_config,
)
from molt.cli.cache_fingerprints import _cache_fingerprint, _cache_tooling_fingerprint
from molt.cli.default_paths import _default_molt_bin
from molt.cli import build_inputs as _build_inputs
from molt.cli import frontend_pipeline as _frontend_pipeline
from molt.cli.external_native import (
    _parse_external_static_packages,
    _resolve_external_package_native_artifact_plan,
)
from molt.cli.file_hashing import _sha256_file
from molt.cli.json_cache import _read_cached_json_object, _write_cached_json_object
from molt.cli.json_contract import (
    _coerce_json_path,
    _extract_json_errors,
    _extract_json_warnings,
    _extract_payload_text_list,
    _wrapper_build_payload_data,
)
from molt.cli.module_graph import (
    _ModuleResolutionCache,
    _discover_module_graph,
    _extend_module_graph_with_static_import_modules,
    _parse_static_import_modules,
    _stdlib_allowlist,
    _stdlib_root_path,
)
from molt.cli.module_source import _source_content_sha256
from molt.cli.models import (
    _ImportAdmissionPolicy,
    _ResolvedBuildEntry,
    _WrapperBuildContract,
)
from molt.cli.output import (
    coerce_process_text as _coerce_process_text,
    emit_json as _emit_json,
    fail as _fail,
    json_payload as _json_payload,
)
from molt.cli.target_python import (
    TargetPythonVersion,
    _parse_target_python_version,
    _resolve_target_python_version,
)


def _build_args_has_json_flag(args: Sequence[str]) -> bool:
    return any(arg == "--json" for arg in args)


def _build_args_has_python_version_flag(args: Sequence[str]) -> bool:
    return any(
        arg == "--python-version" or arg.startswith("--python-version=") for arg in args
    )


@contextmanager
def _scoped_environ_updates(updates: Mapping[str, str]) -> Iterator[None]:
    if not updates:
        yield
        return
    previous = {key: os.environ.get(key) for key in updates}
    try:
        os.environ.update(updates)
        yield
    finally:
        for key, value in previous.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


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
    return _resolve_target_python_version(
        explicit=None,
        build_config=_resolve_build_config(_build_inputs._load_molt_config(project_root)),
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
    output_base = _frontend_pipeline._output_base_for_entry(
        resolved_build_entry.entry_module,
        resolved_build_entry.source_path,
    )
    return _default_molt_bin() / f"{output_base}_molt"


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
    stdlib_root = _stdlib_root_path()
    module_roots = list(resolved_build_entry.module_roots)
    roots = list(dict.fromkeys([*module_roots, stdlib_root]))
    admitted_packages, admission_error = _parse_external_static_packages(
        os.environ.get("MOLT_EXTERNAL_STATIC_PACKAGES", "")
    )
    if admission_error is not None:
        return None
    native_plan, native_plan_errors = _resolve_external_package_native_artifact_plan(
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
    stdlib_allowlist = _stdlib_allowlist()
    resolution_cache = _ModuleResolutionCache()
    try:
        graph, explicit_imports = _discover_module_graph(
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
    static_import_modules, static_import_error = _parse_static_import_modules(
        os.environ.get(STATIC_IMPORT_MODULES_ENV, "")
    )
    if static_import_error is not None:
        return None
    static_import_errors = _extend_module_graph_with_static_import_modules(
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
        source_hash = _source_content_sha256(path, stat)
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
    source_path = resolved_build_entry.source_path
    try:
        resolved_source_path = source_path.resolve()
    except OSError:
        resolved_source_path = source_path
    source_hash = _source_content_sha256(resolved_source_path)
    if source_hash is None:
        return None
    capability_config_digest = _build_inputs._capability_config_cache_digest_from_env(env)
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
        "runtime_backend_fingerprint": _cache_fingerprint(),
        "frontend_tooling_fingerprint": _cache_tooling_fingerprint(),
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


def _emit_wrapper_build_success_signals(payload: Mapping[str, Any]) -> None:
    data = _wrapper_build_payload_data(payload)
    stdout = data.get("stdout")
    if isinstance(stdout, str) and stdout:
        print(stdout, end="")
    stderr = data.get("stderr")
    if isinstance(stderr, str) and stderr:
        print(stderr, end="", file=sys.stderr)
    for warning in _extract_json_warnings(payload):
        label = warning if warning.startswith("Warning:") else f"Warning: {warning}"
        print(label, file=sys.stderr)
    for message in _extract_payload_text_list(data.get("messages")):
        print(message, file=sys.stderr)
    diagnostics = data.get("compile_diagnostics")
    if isinstance(diagnostics, dict):
        _emit_build_diagnostics(
            diagnostics=diagnostics,
            diagnostics_path=None,
            json_output=False,
        )


def _parse_wrapper_build_contract_payload(
    payload: Any,
    *,
    json_output: bool,
    command: str,
) -> tuple[_WrapperBuildContract | None, int | None]:
    if not isinstance(payload, dict):
        return None, _fail(
            "Build JSON payload must be an object.",
            json_output,
            command=command,
        )
    if payload.get("status") != "ok":
        errors = _extract_json_errors(payload)
        message = "\n".join(errors) if errors else "Build did not succeed."
        return None, _fail(message, json_output, command=command)
    data = payload.get("data")
    if not isinstance(data, dict):
        return None, _fail(
            "Build JSON payload missing data.",
            json_output,
            command=command,
        )
    output = _coerce_json_path(data.get("output"))
    if output is None:
        return None, _fail(
            "Build output missing in JSON payload.",
            json_output,
            command=command,
        )
    consumer_output = _coerce_json_path(data.get("consumer_output")) or output
    bundle_root = _coerce_json_path(data.get("bundle_root"))
    raw_artifacts = data.get("artifacts")
    artifacts: dict[str, Path] = {}
    if raw_artifacts is not None:
        if not isinstance(raw_artifacts, dict):
            return None, _fail(
                "Build artifacts must be a JSON object.",
                json_output,
                command=command,
            )
        for key, value in raw_artifacts.items():
            if not isinstance(key, str) or not isinstance(value, str):
                return None, _fail(
                    "Build artifacts must map string keys to string paths.",
                    json_output,
                    command=command,
                )
            artifacts[key] = Path(value)
    return (
        _WrapperBuildContract(
            output=output,
            consumer_output=consumer_output,
            bundle_root=bundle_root,
            artifacts=artifacts,
        ),
        None,
    )


def _emit_wrapper_build_failure(
    *,
    command: str,
    json_output: bool,
    returncode: int,
    stdout: str,
    stderr: str,
) -> int:
    nested_payload: dict[str, Any] | None = None
    if stdout.strip():
        try:
            decoded = json.loads(stdout)
        except json.JSONDecodeError:
            decoded = None
        if isinstance(decoded, dict):
            nested_payload = decoded
    errors = _extract_json_errors(nested_payload) or ["build failed"]
    nested_data = _wrapper_build_payload_data(nested_payload)
    if json_output:
        data: dict[str, Any] = {"returncode": returncode}
        if stdout:
            data["build_stdout"] = stdout
        if stderr:
            data["build_stderr"] = stderr
        _emit_json(
            _json_payload(command, "error", data=data, errors=errors),
            json_output=True,
        )
        return returncode
    detail_parts: list[str] = []
    detail = "\n".join(errors).strip("\n")
    if detail:
        detail_parts.append(detail)
    nested_stderr = nested_data.get("stderr")
    if isinstance(nested_stderr, str) and nested_stderr.strip():
        detail_parts.append(nested_stderr.strip())
    nested_stdout = nested_data.get("stdout")
    if isinstance(nested_stdout, str) and nested_stdout.strip():
        detail_parts.append(nested_stdout.strip())
    if stderr.strip():
        detail_parts.append(stderr.strip())
    if nested_payload is None and stdout.strip():
        detail_parts.append(stdout.strip())
    detail = "\n".join(part for part in detail_parts if part).strip("\n")
    if not detail:
        detail = "Build failed"
    return _fail(detail, json_output=False, code=returncode, command=command)


def _run_wrapper_build(
    *,
    file_path: str | None,
    module: str | None,
    build_args: Sequence[str],
    env: Mapping[str, str],
    project_root: Path,
    json_output: bool,
    command: str,
    verbose: bool,
    resolved_build_entry: _ResolvedBuildEntry | None = None,
    memory_guard_prefix: str | None = _CLI_MEMORY_GUARD_PREFIX,
) -> tuple[_WrapperBuildContract | None, float, int | None]:
    wrapper_cache_enabled = (
        resolved_build_entry is not None
        and "--no-cache" not in build_args
        and "--rebuild" not in build_args
    )
    if wrapper_cache_enabled:
        cached_contract = _read_wrapper_build_cache_contract(
            resolved_build_entry=resolved_build_entry,
            build_args=build_args,
            env=env,
            project_root=project_root,
        )
        if cached_contract is not None:
            return cached_contract, 0.0, None

    # Show progress when building (no silent hangs).
    if not json_output and not verbose:
        _source = Path(file_path).name if file_path else module or "module"
        print(f"Compiling {_source}...", file=sys.stderr, flush=True)

    build_cmd = [sys.executable, "-m", "molt.cli", "build"]
    if not _build_args_has_json_flag(build_args):
        build_cmd.append("--json")
    build_cmd.extend(build_args)
    if module:
        build_cmd.extend(["--module", module])
    else:
        assert file_path is not None
        build_cmd.append(file_path)
    if verbose and not json_output:
        print(f"Build command: {shlex.join(build_cmd)}", file=sys.stderr)
    start = time.monotonic()
    build_res = _runtime_build._run_completed_command(
        build_cmd,
        env=dict(env),
        cwd=project_root,
        capture_output=True,
        memory_guard_prefix=memory_guard_prefix,
    )
    duration = getattr(build_res, "elapsed_s", None)
    if duration is None:
        duration = time.monotonic() - start
    stdout = _coerce_process_text(build_res.stdout)
    stderr = _coerce_process_text(build_res.stderr)
    if build_res.returncode != 0:
        return (
            None,
            duration,
            _emit_wrapper_build_failure(
                command=command,
                json_output=json_output,
                returncode=build_res.returncode,
                stdout=stdout,
                stderr=stderr,
            ),
        )
    try:
        payload = json.loads(stdout.strip() or "{}")
    except json.JSONDecodeError:
        return (
            None,
            duration,
            _fail(
                "Failed to parse build JSON output.",
                json_output,
                command=command,
            ),
        )
    contract, contract_error = _parse_wrapper_build_contract_payload(
        payload,
        json_output=json_output,
        command=command,
    )
    if contract_error is not None:
        return None, duration, contract_error
    assert contract is not None
    if wrapper_cache_enabled:
        with contextlib.suppress(OSError):
            _write_wrapper_build_cache_manifest(
                resolved_build_entry=resolved_build_entry,
                build_args=build_args,
                env=env,
                project_root=project_root,
                contract=contract,
            )
    if not json_output:
        _emit_wrapper_build_success_signals(payload)
        # Forward compilation warnings (SyntaxWarning, DeprecationWarning)
        # from the build subprocess so they appear in `molt run` output,
        # matching CPython's behaviour where warnings are emitted during
        # compile() which runs inline with execution.
        #
        # When the user has explicitly opted into backend diagnostics via an
        # env knob (TIR_OPT_STATS, MOLT_BACKEND_TIMING, TIR_DUMP, ...), stream
        # subprocess stderr verbatim instead - the warning-only filter would
        # drop the diagnostic output the user asked for.
        if stderr:
            if _env_requests_backend_diagnostics(env):
                sys.stderr.write(stderr)
                sys.stderr.flush()
            else:
                _forward_compilation_warnings(stderr)
    return contract, duration, None
