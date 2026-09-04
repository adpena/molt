from __future__ import annotations

import contextlib
from collections.abc import Mapping, MutableMapping, Sequence
from pathlib import Path
import platform
import subprocess
import sys
import time
from typing import Any

from molt.capability_manifest import ResolvedRuntimePolicy
from molt.cli.binary_image_analysis import (
    _merge_binary_image_analysis_stage,
    _native_artifact_binary_image_analysis_payload,
    _non_native_artifact_binary_image_analysis_payload,
)
from molt.cli.build_diagnostics import _emit_build_diagnostics_if_present
from molt.cli.command_runtime import _run_completed_command
from molt.cli.extension_manifest import _cpu_baseline
from molt.cli.models import _StagedExternalPackageNativeArtifact
from molt.cli.llvm_wasi_tools import llvm_tool_candidates
from molt.cli.native_binary import (
    _NativeBinaryInvalid,
    _assert_native_binary_valid,
)
from molt.cli.atomic_io import _atomic_copy_file
from molt.cli.native_link_plan import (
    native_strip_flags,
    resolve_native_target_spec,
)
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import json_payload as _json_payload
from molt.cli.runtime_fingerprints import _write_runtime_fingerprint


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
    resolved_capability_policy: ResolvedRuntimePolicy,
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
        "capabilities": list(resolved_capability_policy.grants.capabilities),
        "capability_profiles": list(resolved_capability_policy.grants.profiles),
        "capability_policy_digest": resolved_capability_policy.digest(),
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


def _build_native_link_success_data(
    *,
    target: str,
    target_triple: str | None,
    source_path: Path,
    output_binary: Path,
    deterministic: bool,
    trusted: bool,
    resolved_capability_policy: ResolvedRuntimePolicy,
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
    external_native_artifacts: Sequence[_StagedExternalPackageNativeArtifact] = (),
) -> dict[str, Any]:
    data = _build_common_build_json_data(
        target=target,
        target_triple=target_triple,
        source_path=source_path,
        output=output_binary,
        deterministic=deterministic,
        trusted=trusted,
        resolved_capability_policy=resolved_capability_policy,
        capabilities_source=capabilities_source,
        sysroot_path=sysroot_path,
        cache_info=cache_info,
        emit_mode=emit_mode,
        profile=profile,
        native_arch_perf_enabled=native_arch_perf_enabled,
    )
    data["consumer_output"] = str(output_binary)
    data["messages"] = [f"Successfully built {output_binary}"]
    data["artifacts"] = {
        "binary": str(output_binary),
        "object": str(output_obj),
        "stub": str(stub_path),
        "runtime": str(runtime_lib),
    }
    if external_native_artifacts:
        data["external_native_artifacts"] = [
            artifact.json_payload() for artifact in external_native_artifacts
        ]
        data["artifacts"]["external_static_packages_root"] = str(
            external_native_artifacts[0].runtime_root
        )
        for index, artifact in enumerate(external_native_artifacts):
            data["artifacts"][f"external_native_artifact_{index}"] = str(
                artifact.staged_path
            )
            data["artifacts"][f"external_native_artifact_{index}_manifest"] = str(
                artifact.staged_manifest_path
            )
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


def _post_link_strip(binary: Path, target_triple: str | None) -> str | None:
    """Strip through an object-format-capable tool without host-target guessing."""
    if not binary.exists():
        return f"post-link strip input does not exist: {binary}"
    try:
        target = resolve_native_target_spec(
            target_triple,
            host_platform=sys.platform,
            host_arch=platform.machine(),
        )
        host = resolve_native_target_spec(
            None,
            host_platform=sys.platform,
            host_arch=platform.machine(),
        )
        flags = native_strip_flags(target)
    except RuntimeError as exc:
        return f"post-link strip target policy failed: {exc}"

    candidates = llvm_tool_candidates("strip")
    target_is_host = (target.os, target.arch) == (host.os, host.arch)
    if not target_is_host:
        candidates = tuple(
            candidate
            for candidate in candidates
            if candidate.stem.lower() == "llvm-strip"
        )
    if not candidates:
        qualifier = "target-capable llvm-strip" if not target_is_host else "strip tool"
        return f"post-link {qualifier} is unavailable for {target.os}/{target.arch}"

    strip_command = [str(candidates[0]), *flags, str(binary)]
    try:
        result = _run_completed_command(
            strip_command,
            capture_output=True,
            env=None,
            cwd=binary.parent,
            memory_guard_prefix="MOLT_BUILD",
            timeout=30,
        )
    except FileNotFoundError:
        return "post-link strip tool is unavailable"
    except subprocess.TimeoutExpired:
        return "post-link strip timed out"
    except OSError as exc:
        return f"post-link strip could not start: {exc}"
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "").strip()
        return f"post-link strip failed{f': {detail}' if detail else ''}"
    return None


def _finalize_native_link_candidate(
    *,
    candidate: Path,
    output_binary: Path,
    target_triple: str | None,
    strip: bool,
    phase_times: MutableMapping[str, int] | None = None,
) -> str | None:
    """Finalize and validate a private candidate before atomic publication."""
    if candidate == output_binary:
        return None
    strip_started = time.perf_counter_ns() if phase_times is not None else 0
    strip_error: str | None = None
    if strip:
        strip_error = _post_link_strip(candidate, target_triple)
    if phase_times is not None:
        phase_times["strip_wall_ns"] = time.perf_counter_ns() - strip_started
    if strip and strip_error:
        return strip_error
    validate_started = time.perf_counter_ns() if phase_times is not None else 0
    try:
        _assert_native_binary_valid(candidate, target_triple)
    except _NativeBinaryInvalid as exc:
        if phase_times is not None:
            phase_times["validate_wall_ns"] = time.perf_counter_ns() - validate_started
        return f"native candidate validation failed: {exc}"
    if phase_times is not None:
        phase_times["validate_wall_ns"] = time.perf_counter_ns() - validate_started
    publish_started = time.perf_counter_ns() if phase_times is not None else 0
    try:
        _atomic_copy_file(candidate, output_binary, codesign=True)
    except OSError as exc:
        if phase_times is not None:
            phase_times["publish_wall_ns"] = time.perf_counter_ns() - publish_started
        return f"atomic native publication failed: {exc}"
    if phase_times is not None:
        phase_times["publish_wall_ns"] = time.perf_counter_ns() - publish_started
    cleanup_started = time.perf_counter_ns() if phase_times is not None else 0
    with contextlib.suppress(OSError):
        candidate.unlink()
    if phase_times is not None:
        phase_times["cleanup_wall_ns"] = time.perf_counter_ns() - cleanup_started
    return None


def _write_link_fingerprint_if_needed(
    *,
    link_skipped: bool,
    link_fingerprint: dict[str, Any] | None,
    link_fingerprint_path: Path,
    json_output: bool,
) -> str | None:
    del json_output
    if link_skipped or link_fingerprint is None:
        return None
    try:
        link_fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
        _write_runtime_fingerprint(link_fingerprint_path, link_fingerprint)
    except OSError as exc:
        return f"failed to write link fingerprint metadata: {exc}"
    return None


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
    link_candidate: Path | None = None,
    deterministic: bool,
    trusted: bool,
    resolved_capability_policy: ResolvedRuntimePolicy,
    capabilities_source: str | None,
    sysroot_path: Path | None,
    emit_mode: str,
    profile: str,
    native_arch_perf_enabled: bool,
    output_obj: Path,
    stub_path: Path,
    runtime_lib: Path,
    external_native_artifacts: Sequence[_StagedExternalPackageNativeArtifact],
    diagnostics_payload: dict[str, Any] | None,
    diagnostics_path: Path | None,
    pgo_profile_payload: Any | None,
    runtime_feedback_payload: Any | None,
    emit_ir_path: Path | None,
    stdlib_obj_path: Path | None,
    warnings: list[str],
    json_output: bool,
    resolved_diagnostics_verbosity: str,
    strip_after_link: bool = True,
) -> int:
    if link_process.returncode == 0:
        # LinkPlan owns strip ordering. Ordinary release plans strip here;
        # BOLT plans retain symbols and relocations until the optimized image
        # has been validated and published, then strip in the BOLT executor.
        candidate = link_candidate or output_binary
        if candidate != output_binary:
            if finalize_error := _finalize_native_link_candidate(
                candidate=candidate,
                output_binary=output_binary,
                target_triple=target_triple,
                strip=strip_after_link,
            ):
                message = f"Build failed during native finalization: {finalize_error}"
                if json_output:
                    _emit_json(
                        _json_payload(
                            "build",
                            "error",
                            data={"output": str(output_binary), "target": target},
                            errors=[message],
                        ),
                        json_output,
                    )
                else:
                    print(message, file=sys.stderr)
                _emit_build_diagnostics_if_present(
                    diagnostics_payload=diagnostics_payload,
                    diagnostics_path=diagnostics_path,
                    json_output=json_output,
                    verbosity=resolved_diagnostics_verbosity,
                )
                return 1
        # Build-time output validity gate (self-protection, task #18): a link
        # that returns 0 can still emit a structurally corrupt artifact (e.g. a
        # mis-applied relocation that flips the Mach-O magic 0xFEEDFACF->0xFEEDFACE,
        # yielding a kernel-SIGKILLed binary). Validate the produced binary's
        # object-file magic against the target format (deterministic and side-
        # effect-free); the deeper exec loader probe is opt-in via
        # MOLT_BUILD_SMOKE_EXEC=1 (it runs the image). On failure, fail the build
        # loudly instead of reporting success — this class must never ship.
        try:
            _assert_native_binary_valid(output_binary, target_triple)
        except _NativeBinaryInvalid as validity_error:
            # Remove the corrupt artifact so a stale-but-invalid binary cannot be
            # picked up by a later step, then surface a clear error and fail.
            with contextlib.suppress(OSError):
                if emit_mode == "bin" and output_binary.exists():
                    output_binary.unlink()
            message = f"Build failed: produced binary is invalid. {validity_error}"
            if json_output:
                payload = _json_payload(
                    "build",
                    "error",
                    data={"output": str(output_binary), "target": target},
                    errors=[message],
                )
                _emit_json(payload, json_output)
            else:
                print(message, file=sys.stderr)
            _emit_build_diagnostics_if_present(
                diagnostics_payload=diagnostics_payload,
                diagnostics_path=diagnostics_path,
                json_output=json_output,
                verbosity=resolved_diagnostics_verbosity,
            )
            return 1
        link_fingerprint_warning = _write_link_fingerprint_if_needed(
            link_skipped=link_skipped,
            link_fingerprint=link_fingerprint,
            link_fingerprint_path=link_fingerprint_path,
            json_output=json_output,
        )
        if link_fingerprint_warning is not None:
            warnings.append(link_fingerprint_warning)
            if not json_output:
                print(f"Warning: {link_fingerprint_warning}", file=sys.stderr)
        _merge_binary_image_analysis_stage(
            diagnostics_payload,
            "artifacts",
            _native_artifact_binary_image_analysis_payload(
                output_binary=output_binary,
                output_obj=output_obj,
                runtime_lib=runtime_lib,
                stdlib_obj_path=stdlib_obj_path,
                link_skipped=link_skipped,
                link_fingerprint=link_fingerprint,
                link_fingerprint_path=link_fingerprint_path,
                external_native_artifact_count=len(external_native_artifacts),
            ),
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
                resolved_capability_policy=resolved_capability_policy,
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
                external_native_artifacts=external_native_artifacts,
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
            print(f"Successfully built {output_binary}", file=sys.stderr)
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


def _emit_non_native_build_result(
    *,
    output: Path,
    consumer_output: Path | None,
    bundle_root: Path | None,
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
    deterministic: bool,
    trusted: bool,
    resolved_capability_policy: ResolvedRuntimePolicy,
    capabilities_source: str | None,
    sysroot_path: Path | None,
    emit_mode: str,
    profile: str,
    native_arch_perf_enabled: bool,
    diagnostics_payload: dict[str, Any] | None,
    diagnostics_path: Path | None,
    pgo_profile_payload: Any | None,
    runtime_feedback_payload: Any | None,
    emit_ir_path: Path | None,
    warnings: list[str],
    json_output: bool,
    resolved_diagnostics_verbosity: str,
    artifacts: Mapping[str, Any] | None = None,
    extra_fields: Mapping[str, Any] | None = None,
    success_messages: Sequence[str] = (),
) -> int:
    _merge_binary_image_analysis_stage(
        diagnostics_payload,
        "artifacts",
        _non_native_artifact_binary_image_analysis_payload(
            kind=target,
            output=output,
            consumer_output=consumer_output,
            artifacts=artifacts,
        ),
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
        data = _build_common_build_json_data(
            target=target,
            target_triple=target_triple,
            source_path=source_path,
            output=output,
            deterministic=deterministic,
            trusted=trusted,
            resolved_capability_policy=resolved_capability_policy,
            capabilities_source=capabilities_source,
            sysroot_path=sysroot_path,
            cache_info=cache_info,
            emit_mode=emit_mode,
            profile=profile,
            native_arch_perf_enabled=native_arch_perf_enabled,
        )
        if consumer_output is not None:
            data["consumer_output"] = str(consumer_output)
        if bundle_root is not None:
            data["bundle_root"] = str(bundle_root)
        if success_messages:
            data["messages"] = list(success_messages)
        if artifacts is not None:
            data["artifacts"] = dict(artifacts)
        if extra_fields is not None:
            data.update(extra_fields)
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
        for message in success_messages:
            print(message)
    _emit_build_diagnostics_if_present(
        diagnostics_payload=diagnostics_payload,
        diagnostics_path=diagnostics_path,
        json_output=json_output,
        verbosity=resolved_diagnostics_verbosity,
    )
    return 0
