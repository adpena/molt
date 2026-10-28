from __future__ import annotations

from molt.cli.link_input_observation import CompilationInputRecord

from molt.backend_protocol import BackendEmissionOrigin

from molt.compilation_target import target_name

from molt.compilation_target import Target

from collections.abc import Callable, Generator, Mapping, MutableMapping, Sequence
from contextlib import contextmanager
from pathlib import Path
import os
import platform
import subprocess
import sys
import time
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from molt.build_record import NativeBuildResult, RecordedArtifact

from molt.toolchain_identity import (
    executable_content_identity,
    stable_file_content_identity,
)
from molt.capability_manifest import ResolvedRuntimePolicy
from molt.artifact_publication import discard_staged_output
from molt.cli.binary_image_analysis import (
    _native_artifact_binary_image_analysis_payload,
    _non_native_artifact_binary_image_analysis_payload,
)
from molt.cli.build_diagnostics import _emit_build_diagnostics_if_present
from molt.cli.command_runtime import (
    _finish_build_input_custody,
    _run_completed_command,
)
from molt.cli.output import success as _success
from molt.cli.extension_manifest import _cpu_baseline
from molt.cli.models import _StagedExternalPackageNativeArtifact
from molt.llvm_wasi_tools import llvm_tool_candidates
from molt.cli.native_binary import (
    _NativeBinaryInvalid,
    _assert_native_binary_valid,
)
from molt.cli import atomic_io
from molt.cli.atomic_io import _staged_copy_file
from molt.file_publication import (
    canonical_file_leaf,
    is_owned_staged_file_path,
    staged_file_path,
)
from molt.link_outputs import validate_link_output_paths
from molt.cli.native_link_plan import native_strip_flags
from molt.native_target import resolve_native_target_spec
from molt.native_artifact_header import read_macho_code_signature
from molt.native_target_shape import NativeObjectFormat
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import json_payload as _json_payload
from molt.cli import link_fingerprints, progress


def _observed_build_toolchain(
    *,
    backend_bin: Path | None,
    runtime_lib: Path | None,
    output: Path,
    selected_profiles: Mapping[str, str] | None = None,
    emission_origin: BackendEmissionOrigin | None = None,
    link_input_observation: CompilationInputRecord | None = None,
) -> dict[str, Any]:
    """Observe selected bytes; unknown or failed observations never become claims.

    This is an observation at publication, not an attestation of which bytes an
    already-running daemon loaded. Consumers must retain that distinction.
    """
    facts: dict[str, Any] = {
        "kind": "molt-build-observation-v1",
        "compiled_with_verified": False,
    }
    if link_input_observation is not None:
        facts["link_input_observation"] = dict(link_input_observation)
    if emission_origin is not None:
        facts["emission_origin"] = emission_origin.payload()
    if selected_profiles is not None:
        facts["selected_profiles"] = dict(selected_profiles)
    for name, path, executable in (
        ("compiler", backend_bin, True),
        ("runtime", runtime_lib, False),
        ("artifact", output, False),
    ):
        if path is None:
            facts[name] = None
            continue
        try:
            identity = (
                executable_content_identity(path, label=f"build {name}")
                if executable
                else stable_file_content_identity(path, label=f"build {name}")
            )
            facts[name] = {"path": str(path), "identity": identity}
        except (OSError, ValueError) as exc:
            facts[name] = {"error": str(exc)}
    if runtime_lib is None and link_input_observation is not None:
        link = link_input_observation["link"]
        if link["schema"] == "molt.wasm-link-inputs.v2":
            facts["runtime"] = {
                "identity": dict(link["source_inputs"]["runtime"]),
                "observation": "linker-input-buffer",
            }
    return facts


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
    target: Target,
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
        "target": target_name(target),
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


def _emit_native_link_process_output(
    process: subprocess.CompletedProcess[str],
) -> None:
    # Build status and diagnostics use stderr in text mode. Preserve both
    # captured linker streams there: drivers may report failures or warnings
    # on either stream, and the outer build consumer reads this one channel.
    for output in (process.stdout, process.stderr):
        if output:
            sys.stderr.write(output)
            if not output.endswith("\n"):
                sys.stderr.write("\n")


def _emit_build_result_json(
    *,
    data: Mapping[str, Any],
    warnings: Sequence[str],
    json_output: bool,
    returncode: int,
    diagnostics_error: str | None,
    errors: Sequence[str] = (),
) -> int:
    result = returncode or (1 if diagnostics_error is not None else 0)
    result_data = dict(data)
    if result != 0:
        result_data["returncode"] = result
        result_data.pop("messages", None)
    if diagnostics_error is not None:
        result_data["diagnostics_error"] = diagnostics_error
    payload = _json_payload(
        "build",
        "error" if result != 0 else "ok",
        data=result_data,
        warnings=list(warnings),
        errors=list(errors) or ([diagnostics_error] if diagnostics_error else []),
    )
    _emit_json(payload, json_output)
    return result


def _build_native_link_success_data(
    *,
    target: Target,
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
        "archive": str(output_obj),
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


def _recorded_toolchain_artifact(
    artifact: RecordedArtifact, *, executable: bool
) -> dict[str, object]:
    def filename(path: Path) -> str:
        return path.name.casefold() if os.name == "nt" else path.name

    identity: dict[str, str | int] = {
        "size": artifact.token[3],
        "sha256": artifact.sha256,
    }
    if executable:
        identity.update(
            {
                "entrypoint": filename(artifact.path),
                "content_filename": filename(artifact.resolved_path),
            }
        )
    else:
        identity["filename"] = filename(artifact.path)
    return {"path": str(artifact.path), "identity": identity}


def _record_native_build_result(
    *,
    data: Mapping[str, Any],
    result_inputs: tuple[RecordedArtifact, RecordedArtifact, RecordedArtifact] | None,
    warnings: Sequence[str],
    diagnostics_error: str | None,
) -> None:
    """Retain producer identities after finalization, never recapture link inputs."""
    from molt import input_report

    if not input_report.result_active():
        return
    from dataclasses import fields
    from molt.build_record import NativeBuildFacts, NativeBuildResult, RecordedArtifact
    from molt.backend_protocol import BackendEmissionOrigin

    if (
        warnings
        or diagnostics_error is not None
        or any(
            data.get(key)
            for key in (
                "stdout",
                "stderr",
                "compile_diagnostics",
                "pgo_profile",
                "runtime_feedback",
                "emit_ir",
                "external_native_artifacts",
            )
        )
    ):
        input_report.refuse(
            "native JSON contains invocation-specific or additional results"
        )
        return
    if result_inputs is None:
        input_report.refuse("native JSON lacks pre-link artifact identities")
        return
    supported_fields = {field.name for field in fields(NativeBuildFacts)} | {
        "target",
        "output",
        "consumer_output",
        "sysroot",
        "emit",
        "cache",
        "link",
        "messages",
        "artifacts",
        "observed_toolchain",
    }
    if set(data) - supported_fields:
        input_report.refuse("native JSON contains unclassified result fields")
        return
    try:
        archive, stub, runtime = result_inputs
        for artifact in result_inputs:
            artifact.verify()
        observed = data["observed_toolchain"]
        if (
            not isinstance(observed, dict)
            or set(observed)
            - {
                "kind",
                "compiled_with_verified",
                "compiler",
                "runtime",
                "artifact",
                "selected_profiles",
                "emission_origin",
            }
            or observed.get("kind") != "molt-build-observation-v1"
            or observed.get("compiled_with_verified") is not False
        ):
            raise ValueError("unsupported native toolchain observation")
        compiler = RecordedArtifact.capture(Path(observed["compiler"]["path"]))
        binary = RecordedArtifact.capture(Path(data["output"]))
        for name, artifact in (
            ("compiler", compiler),
            ("runtime", runtime),
            ("artifact", binary),
        ):
            if observed[name] != _recorded_toolchain_artifact(
                artifact, executable=name == "compiler"
            ):
                raise ValueError(f"native {name} observation changed before recording")
        if (
            data["artifacts"]
            != {
                "binary": str(binary.path),
                "archive": str(archive.path),
                "stub": str(stub.path),
                "runtime": str(runtime.path),
            }
            or Path(data["consumer_output"]).absolute() != binary.path
        ):
            raise ValueError(
                "native JSON artifacts differ from their producer selections"
            )
        if (
            data["target"] != "native"
            or data["emit"] != "bin"
            or data["sysroot"] is not None
        ):
            raise ValueError("unsupported native JSON result")
        facts = NativeBuildFacts.from_payload(
            {field.name: data[field.name] for field in fields(NativeBuildFacts)}
        )
        profiles = observed.get("selected_profiles", {})
        if not isinstance(profiles, dict) or any(
            not isinstance(key, str) or not isinstance(value, str)
            for key, value in profiles.items()
        ):
            raise ValueError("invalid selected profiles")
        origin = observed.get("emission_origin")
        result = NativeBuildResult(
            facts,
            binary,
            archive,
            stub,
            runtime,
            compiler,
            tuple(sorted(profiles.items())),
            None if origin is None else BackendEmissionOrigin.from_payload(origin),
        )
        result.verify()
        input_report.report_build_result(result.payload())
    except (OSError, ValueError, TypeError, KeyError) as exc:
        input_report.refuse(f"native JSON result cannot be retained: {exc}")


def _emit_reused_native_build_result(result: NativeBuildResult) -> int:
    """Project this invocation's reuse and the original artifact's provenance."""
    data = result.facts.payload()
    output = str(result.binary.path)
    data.update(
        {
            "target": "native",
            "output": output,
            "consumer_output": output,
            "sysroot": None,
            "emit": "bin",
            "cache": {"enabled": True, "hit": True, "hit_tier": "build-record"},
            "link": {"skipped": True},
            "messages": [f"Successfully built {output} (inputs unchanged)"],
            "artifacts": {
                name: str(getattr(result, name).path)
                for name in ("binary", "archive", "stub", "runtime")
            },
        }
    )
    observed: dict[str, Any] = {
        "kind": "molt-build-observation-v1",
        "compiled_with_verified": False,
        "selected_profiles": dict(result.selected_profiles),
    }
    for name, artifact in (
        ("compiler", result.compiler),
        ("runtime", result.runtime),
        ("artifact", result.binary),
    ):
        observed[name] = _recorded_toolchain_artifact(
            artifact, executable=name == "compiler"
        )
    if result.emission_origin is not None:
        observed["emission_origin"] = result.emission_origin.payload()
    data["observed_toolchain"] = observed
    return _emit_build_result_json(
        data=data,
        warnings=(),
        json_output=True,
        returncode=0,
        diagnostics_error=None,
    )


def _build_native_link_error_data(
    *,
    target: Target,
    source_path: Path,
    returncode: int,
    emit_mode: str,
    profile: str,
    native_arch_perf_enabled: bool,
    trusted: bool,
    cache_info: Mapping[str, Any],
) -> dict[str, Any]:
    return {
        "target": target_name(target),
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


def _signed_by_linker_as(stage: Path, destination: Path) -> bool:
    """Whether the linker signed ``stage`` ad hoc as the published binary."""
    signature = read_macho_code_signature(stage)
    return (
        signature is not None
        and signature.ad_hoc
        and signature.covers_image
        and signature.identifier == destination.name
    )


@contextmanager
def _native_publication_stage(
    candidate: Path,
    output_binary: Path,
    *,
    object_format: NativeObjectFormat,
    rewritten: bool,
) -> Generator[Path]:
    """The private stage that publication renames into place.

    The linker writes a private stage of the binary beside it, and that stage
    is published by rename, so the image is never copied (a copy of a 68 MiB
    dev binary cost 1.9-2.5 s). A candidate under another name in the same
    directory, such as BOLT's output, moves into a stage first; one elsewhere
    is copied.

    Only a Mach-O image carries a code signature, and it must name the
    published binary. When the selected linker signs its stage that way,
    the stage keeps that signature unless a tool rewrote the image after
    the link. Every other Mach-O stage is re-signed (codesign cost 1.4-3.2 s on
    the same binary).
    """
    macho = object_format is NativeObjectFormat.MACHO
    if is_owned_staged_file_path(candidate, output_binary):
        if macho and (rewritten or not _signed_by_linker_as(candidate, output_binary)):
            atomic_io._codesign_atomic_copy_temp(candidate, output_binary)
        yield candidate
        return
    if candidate.parent == output_binary.parent:
        stage = staged_file_path(output_binary, purpose="native-link")
        os.rename(candidate, stage)
        try:
            if macho:
                atomic_io._codesign_atomic_copy_temp(stage, output_binary)
            yield stage
        finally:
            discard_staged_output(stage)
        return
    with _staged_copy_file(candidate, output_binary, codesign=macho) as stage:
        yield stage


def _finalize_native_link_candidate(
    *,
    candidate: Path,
    output_binary: Path,
    target_triple: str | None,
    strip: bool,
    phase_times: MutableMapping[str, int] | None = None,
    receipt: link_fingerprints.FinalLinkReceiptRequest | None = None,
    link_selection: tuple[Path, Path] | None = None,
    link_input_artifacts: dict[str, tuple[Path, Path]] | None = None,
    validate_inputs: Callable[[], None] | None = None,
) -> str | None:
    """Finalize and validate a private candidate before atomic publication."""
    try:
        candidate = canonical_file_leaf(
            candidate, create_parent=False, role="native candidate"
        )
        outputs = {
            "binary": output_binary,
            **{k: v[1] for k, v in (link_input_artifacts or {}).items()},
        }
        if link_selection is not None:
            outputs["selection"] = link_selection[1]
        if receipt is not None:
            outputs["receipt"] = receipt.path
        validate_link_output_paths(
            outputs,
            inputs=(
                candidate,
                *(v[0] for v in (link_input_artifacts or {}).values()),
                *((link_selection[0],) if link_selection is not None else ()),
            ),
        )
        if candidate.stat().st_nlink != 1:
            raise ValueError("candidate has other filesystem names")
        if link_selection is not None and link_selection[0].stat().st_nlink != 1:
            raise ValueError("selection candidate has other filesystem names")
    except (OSError, ValueError) as exc:
        return f"native publication requires a private candidate: {exc}"
    try:
        try:
            object_format = resolve_native_target_spec(target_triple).object_format
        except RuntimeError as exc:
            return f"native publication target policy failed: {exc}"
        strip_started = time.perf_counter_ns() if phase_times is not None else 0
        strip_error: str | None = None
        if strip:
            strip_error = _post_link_strip(candidate, target_triple)
        if phase_times is not None:
            phase_times["strip_wall_ns"] = time.perf_counter_ns() - strip_started
        if strip and strip_error:
            return strip_error
        publish_started = time.perf_counter_ns() if phase_times is not None else 0
        validate_elapsed = 0
        try:
            with _native_publication_stage(
                candidate,
                output_binary,
                object_format=object_format,
                rewritten=strip,
            ) as signed:
                validate_started = (
                    time.perf_counter_ns() if phase_times is not None else 0
                )
                try:
                    _assert_native_binary_valid(signed, target_triple)
                except _NativeBinaryInvalid as exc:
                    return f"native candidate validation failed: {exc}"
                finally:
                    if phase_times is not None:
                        validate_elapsed = time.perf_counter_ns() - validate_started
                        phase_times["validate_wall_ns"] = validate_elapsed
                if validate_inputs is not None:
                    validate_inputs()
                link_fingerprints.publish_link_outputs(
                    {
                        "binary": (signed, output_binary),
                        **(link_input_artifacts or {}),
                        **(
                            {"selection": link_selection}
                            if link_selection is not None
                            else {}
                        ),
                    },
                    receipt=receipt,
                    retire_previous_outputs_under=output_binary.parent
                    if receipt is not None
                    else None,
                )
        except (OSError, ValueError, RuntimeError) as exc:
            return f"atomic native publication failed: {exc}"
        finally:
            if phase_times is not None:
                phase_times["publish_wall_ns"] = (
                    time.perf_counter_ns() - publish_started - validate_elapsed
                )
        return None
    finally:
        cleanup_started = time.perf_counter_ns() if phase_times is not None else 0
        discard_staged_output(candidate)
        for stage, _ in (link_input_artifacts or {}).values():
            discard_staged_output(stage)
        if link_selection is not None:
            discard_staged_output(link_selection[0])
        if phase_times is not None:
            phase_times["cleanup_wall_ns"] = time.perf_counter_ns() - cleanup_started


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
    target: Target,
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
    diagnostics_enabled: bool,
    build_diagnostics_payload: Callable[[], tuple[dict[str, Any] | None, Path | None]],
    pgo_profile_payload: Any | None,
    runtime_feedback_payload: Any | None,
    emit_ir_path: Path | None,
    stdlib_obj_path: Path | None,
    warnings: list[str],
    json_output: bool,
    resolved_diagnostics_verbosity: str,
    strip_after_link: bool = True,
    link_selection: tuple[Path, Path] | None = None,
    link_input_artifacts: dict[str, tuple[Path, Path]] | None = None,
    backend_bin: Path | None = None,
    selected_profiles: Mapping[str, str] | None = None,
    finalize_inputs: Callable[[], None] | None = None,
    validate_inputs: Callable[[], None] | None = None,
    emission_origin: BackendEmissionOrigin | None = None,
    link_input_observation: CompilationInputRecord | None = None,
    result_inputs: tuple[RecordedArtifact, RecordedArtifact, RecordedArtifact]
    | None = None,
) -> int:
    def emit_finalization_failure(message: str) -> int:
        diagnostics_payload, diagnostics_error = _emit_build_diagnostics_if_present(
            diagnostics_enabled=diagnostics_enabled,
            build_diagnostics_payload=build_diagnostics_payload,
            json_output=json_output,
            verbosity=resolved_diagnostics_verbosity,
        )
        if json_output:
            data: dict[str, Any] = {
                "output": str(output_binary),
                "target": target_name(target),
            }
            _attach_build_metadata(
                data,
                diagnostics_payload=diagnostics_payload,
                pgo_profile_payload=pgo_profile_payload,
                runtime_feedback_payload=runtime_feedback_payload,
                emit_ir_path=emit_ir_path,
            )
            _attach_process_output(data, link_process)
            return _emit_build_result_json(
                data=data,
                warnings=(),
                json_output=json_output,
                returncode=1,
                diagnostics_error=diagnostics_error,
                errors=[message],
            )
        print(message, file=sys.stderr)
        if diagnostics_error is not None:
            print(diagnostics_error, file=sys.stderr)
        return 1

    if not json_output:
        progress.finish()
        _emit_native_link_process_output(link_process)
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
                validate_inputs=validate_inputs,
                link_selection=link_selection,
                link_input_artifacts=link_input_artifacts,
                receipt=link_fingerprints.FinalLinkReceiptRequest.from_fingerprint(
                    link_fingerprint_path, link_fingerprint
                ),
            ):
                return emit_finalization_failure(
                    f"Build failed during native finalization: {finalize_error}"
                )
        # Fresh candidates were validated after signing, before publication.
        # Reused/already-finalized names still receive the target admission gate;
        # no receipt is ever minted from this public-name observation.
        try:
            if candidate == output_binary:
                _assert_native_binary_valid(output_binary, target_triple)
        except _NativeBinaryInvalid as validity_error:
            # This name may now belong to another publisher. Never delete a
            # public destination based on an unlocked post-publication check.
            return emit_finalization_failure(
                f"Build failed: produced binary is invalid. {validity_error}"
            )
        if custody_error := _finish_build_input_custody(finalize_inputs):
            return emit_finalization_failure(custody_error)
        diagnostics_payload, diagnostics_error = _emit_build_diagnostics_if_present(
            diagnostics_enabled=diagnostics_enabled,
            build_diagnostics_payload=build_diagnostics_payload,
            json_output=json_output,
            verbosity=resolved_diagnostics_verbosity,
            artifact_analysis=lambda: _native_artifact_binary_image_analysis_payload(
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
            data["observed_toolchain"] = _observed_build_toolchain(
                backend_bin=backend_bin,
                runtime_lib=runtime_lib,
                output=output_binary,
                selected_profiles=selected_profiles,
                emission_origin=emission_origin,
                link_input_observation=link_input_observation,
            )
            _attach_build_metadata(
                data,
                diagnostics_payload=diagnostics_payload,
                pgo_profile_payload=pgo_profile_payload,
                runtime_feedback_payload=runtime_feedback_payload,
                emit_ir_path=emit_ir_path,
            )
            _attach_process_output(data, link_process)
            _record_native_build_result(
                data=data,
                result_inputs=result_inputs,
                warnings=warnings,
                diagnostics_error=diagnostics_error,
            )
            return _emit_build_result_json(
                data=data,
                warnings=warnings,
                json_output=json_output,
                returncode=0,
                diagnostics_error=diagnostics_error,
            )
        if diagnostics_error is not None:
            print(diagnostics_error, file=sys.stderr)
            return 1
        _success(f"Successfully built {output_binary}", file=sys.stderr)
    else:
        diagnostics_payload, diagnostics_error = _emit_build_diagnostics_if_present(
            diagnostics_enabled=diagnostics_enabled,
            build_diagnostics_payload=build_diagnostics_payload,
            json_output=json_output,
            verbosity=resolved_diagnostics_verbosity,
        )
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
            return _emit_build_result_json(
                data=data,
                warnings=(),
                json_output=json_output,
                returncode=link_process.returncode,
                diagnostics_error=diagnostics_error,
                errors=["Linking failed"],
            )
        print("Linking failed", file=sys.stderr)
        if diagnostics_error is not None:
            print(diagnostics_error, file=sys.stderr)
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
    target: Target,
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
    diagnostics_enabled: bool,
    build_diagnostics_payload: Callable[[], tuple[dict[str, Any] | None, Path | None]],
    pgo_profile_payload: Any | None,
    runtime_feedback_payload: Any | None,
    emit_ir_path: Path | None,
    warnings: list[str],
    json_output: bool,
    resolved_diagnostics_verbosity: str,
    artifacts: Mapping[str, Any] | None = None,
    extra_fields: Mapping[str, Any] | None = None,
    success_messages: Sequence[str] = (),
    finalize_inputs: Callable[[], None] | None = None,
) -> int:
    custody_error = _finish_build_input_custody(finalize_inputs)
    diagnostics_payload, diagnostics_error = _emit_build_diagnostics_if_present(
        diagnostics_enabled=diagnostics_enabled,
        build_diagnostics_payload=build_diagnostics_payload,
        json_output=json_output,
        verbosity=resolved_diagnostics_verbosity,
        artifact_analysis=lambda: _non_native_artifact_binary_image_analysis_payload(
            kind=target_name(target),
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
        if success_messages and diagnostics_error is None and custody_error is None:
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
        return _emit_build_result_json(
            data=data,
            warnings=warnings,
            json_output=json_output,
            returncode=1 if custody_error is not None else 0,
            diagnostics_error=diagnostics_error,
            errors=[custody_error] if custody_error is not None else (),
        )
    if custody_error is not None:
        print(custody_error, file=sys.stderr)
        if diagnostics_error is not None:
            print(diagnostics_error, file=sys.stderr)
        return 1
    if diagnostics_error is not None:
        print(diagnostics_error, file=sys.stderr)
        return 1
    for message in success_messages:
        _success(message)
    return 0
