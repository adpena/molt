from __future__ import annotations

from molt.cli.link_input_observation import CompilationInputRecord

from dataclasses import dataclass
import json
from molt.backend_protocol import BackendDaemonJobOrigin
from molt.cli.link_input_observation import (
    capture_link_inputs as observe_link_inputs,
    compilation_input_observation,
    capture_native_link_preparation,
)
from molt.toolchain_identity import stable_regular_file_content_identity
from molt.cli.native_link_plan import NativeLinkerKind
from molt.native_target import NativeObjectFormat

import contextlib
import os
import subprocess
import sys
import time
import traceback
from pathlib import Path
from typing import Collection, Sequence

from molt import file_publication
from molt.artifact_publication import discard_staged_output
from molt.capability_manifest import ResolvedRuntimePolicy
from molt.cli import link_fingerprints
from molt.cli.backend_cache import (
    _validate_shared_stdlib_output_for_link,
)
from molt.cli.command_runtime import _run_completed_command
from molt.cli.external_native import (
    _external_native_link_requirements,
    _stage_external_package_native_artifacts_for_build,
)
from molt.cli.build_request import BuildProfile
from molt.cli.models import (
    _ExternalPackageNativeArtifactPlan,
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    _StagedExternalPackageNativeArtifact,
)
from molt.cli.native_binary import (
    _darwin_binary_imports_validation_error,
    _darwin_binary_magic_error,
)
from molt.cli.native_link_command import (
    _build_native_link_plan,
)
from molt.cli.native_link_plan import (
    NativeSymbolStrip,
    native_link_execution_command as _native_link_execution_command,
    validate_native_object_artifact,
)
from molt.native_target import NativeTargetSpec, NativeArtifactKind
from molt.cli.native_link_tool_identity import (
    native_link_cache_tool_facts,
    native_link_tool_facts,
)
from molt.cli.native_main_stub import _render_native_main_stub
from molt.cli.output import CliFailure as _CliFailure
from molt.cli.output import fail as _fail
from molt.link_outputs import link_selection_path, validate_link_output_paths
from molt.cli.link_selection_admission import (
    link_selection_policy,
    native_link_selection,
)
from molt.cli.runtime_native_codegen import NativeRuntimeCodegenBinding
from molt.cli.atomic_io import _write_text_if_changed


@dataclass(frozen=True)
class _PreparedNativeLink:
    output_obj: Path
    stub_path: Path
    runtime_lib: Path
    output_binary: Path
    link_output: Path
    external_native_artifacts: tuple[_StagedExternalPackageNativeArtifact, ...]
    link_cmd: list[str]
    linker_hint: str | None
    normalized_target: str | None
    link_fingerprint_path: Path
    link_fingerprint: dict[str, str | None] | None
    link_skipped: bool
    link_process: subprocess.CompletedProcess[str]
    strip_after_link: bool
    link_selection: tuple[Path, Path] | None = None
    link_input_artifacts: dict[str, tuple[Path, Path]] | None = None
    link_input_observation: CompilationInputRecord | None = None


def _run_native_link_command(
    *,
    link_cmd: Sequence[str],
    json_output: bool,
    link_timeout: float | None,
) -> subprocess.CompletedProcess[str]:
    # The guarded runner raises TimeoutExpired for a guard timeout; a linker
    # that itself exits 124 is a link failure, not a timeout.
    return _run_completed_command(
        list(link_cmd),
        capture_output=True,
        env=None,
        cwd=None,
        timeout=link_timeout,
        memory_guard_prefix="MOLT_BUILD",
    )


def _prepare_native_object_artifact(
    *,
    output_artifact: Path,
    stdlib_obj_path: Path | None,
    json_output: bool,
    target: NativeTargetSpec,
) -> tuple[Path | None, _CliFailure | None]:
    if stdlib_obj_path is not None:
        return None, _fail(
            "Native object output cannot include a separately compiled stdlib; "
            "--emit obj requires one compilation unit.",
            json_output,
            command="build",
        )
    try:
        validate_native_object_artifact(output_artifact, target)
    except (OSError, RuntimeError) as exc:
        return None, _fail(str(exc), json_output, command="build")
    return output_artifact, None


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
    output_binary: Path,
    validation_kind: str,
) -> subprocess.CompletedProcess[str]:
    validation_error = _darwin_link_validation_failure(
        output_binary=output_binary,
        kind=validation_kind,
    )
    if validation_error is None:
        return link_process
    failure_stderr = (link_process.stderr or "") + "\n" + validation_error
    return subprocess.CompletedProcess(
        args=list(link_cmd),
        returncode=1,
        stdout=link_process.stdout,
        stderr=failure_stderr,
    )


def _prepare_native_link(
    *,
    output_artifact: Path,
    resolved_capability_policy: ResolvedRuntimePolicy,
    artifacts_root: Path,
    json_output: bool,
    output_binary: Path | None,
    runtime_codegen_binding: NativeRuntimeCodegenBinding,
    target: NativeTargetSpec,
    sysroot_path: Path | None,
    profile: BuildProfile,
    project_root: Path,
    diagnostics_enabled: bool,
    phase_starts: dict[str, float],
    link_timeout: float | None,
    warnings: list[str],
    stdlib_obj_path: Path | None = None,
    stdlib_object_cache_key: str | None = None,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    native_artifact_plan: _ExternalPackageNativeArtifactPlan = (
        _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN
    ),
    compiled_source_paths: tuple[str, ...] = (),
    bolt_requested: bool = False,
    capture_link_inputs: bool = False,
    daemon_job_origin: BackendDaemonJobOrigin | None = None,
) -> tuple[_PreparedNativeLink | None, _CliFailure | None]:
    if diagnostics_enabled:
        phase_starts["link"] = time.perf_counter()
    output_obj = output_artifact
    link_stdlib_obj = stdlib_obj_path
    # Shared stdlib bytes already belong to this build. Cache retention cannot
    # invalidate a required input after backend preparation/admission.
    if stdlib_obj_path is not None:
        try:
            _validate_shared_stdlib_output_for_link(
                stdlib_obj_path,
                stdlib_object_cache_key=stdlib_object_cache_key,
                stdlib_object_manifest=stdlib_object_manifest,
                stdlib_module_symbols=stdlib_module_symbols,
                target_triple=target.triple,
            )
        except OSError as exc:
            detail = "".join(traceback.format_exception_only(exc)).rstrip()
            return None, _fail(
                f"Failed to admit shared stdlib archive for native link: {detail}",
                json_output,
                command="build",
            )
    try:
        staged_external_native_artifacts = (
            _stage_external_package_native_artifacts_for_build(
                native_artifact_plan,
                artifacts_root=artifacts_root,
            )
        )
    except OSError as exc:
        return None, _fail(
            f"Failed to stage external native artifacts for native build: {exc}",
            json_output,
            command="build",
        )
    main_c_content = _render_native_main_stub(
        resolved_capability_policy=resolved_capability_policy,
        compiled_source_paths=compiled_source_paths,
        runtime_module_roots=tuple(
            dict.fromkeys(
                artifact.runtime_root for artifact in staged_external_native_artifacts
            )
        ),
    )
    stub_path = artifacts_root / "main_stub.c"
    _write_text_if_changed(stub_path, main_c_content)

    if output_binary is None:
        return None, _fail("Binary output unavailable", json_output, command="build")
    if output_binary.parent != Path("."):
        output_binary.parent.mkdir(parents=True, exist_ok=True)
    resolved_runtime_lib = runtime_codegen_binding.runtime_lib
    try:
        runtime_codegen_binding.verify()
        link_plan = _build_native_link_plan(
            output_obj=output_obj,
            output_kind=NativeArtifactKind.ARCHIVE,
            stdlib_kind=NativeArtifactKind.ARCHIVE,
            stub_path=stub_path,
            runtime_lib=resolved_runtime_lib,
            output_binary=output_binary,
            target=target,
            sysroot_path=sysroot_path,
            profile=profile,
            runtime_build_identity=runtime_codegen_binding.build_identity,
            runtime_codegen_binding=runtime_codegen_binding,
            stdlib_obj_path=link_stdlib_obj,
            external_link_requirements=(
                _external_native_link_requirements(
                    staged_external_native_artifacts,
                    target_triple=target.triple,
                ),
            ),
            bolt_requested=bolt_requested,
        )
    except (OSError, RuntimeError, ValueError) as exc:
        return None, _fail(str(exc), json_output, command="build")
    if os.environ.get("MOLT_TRACE_NATIVE_LINK") == "1":
        stdlib_exists = (
            link_stdlib_obj.exists() if link_stdlib_obj is not None else False
        )
        print(
            "native-link trace: "
            f"output_obj={output_obj} "
            f"stdlib_obj={link_stdlib_obj} "
            f"stdlib_exists={stdlib_exists} "
            f"runtime_lib={resolved_runtime_lib} "
            f"output_binary={output_binary}",
            file=sys.stderr,
        )
        print(f"native-link plan: {link_plan}", file=sys.stderr)
    if capture_link_inputs and (
        daemon_job_origin is None
        or link_plan.capabilities.linker is not NativeLinkerKind.LLD
        or link_plan.capabilities.object_format is not NativeObjectFormat.ELF
    ):
        return None, _fail(
            "Link input admission capture requires ELF LLD and retained compiler emission origin.",
            json_output,
            command="build",
        )
    link_cmd = list(link_plan.command)
    linker_hint = link_plan.linker_hint
    normalized_target = link_plan.normalized_target
    if (
        normalized_target is not None
        and not target.is_host
        and normalized_target != target.triple
    ):
        warnings.append(
            f"Zig target normalized to {normalized_target} from {target.triple}."
        )

    link_fingerprint_path = link_fingerprints._link_fingerprint_path(output_binary)
    selection_output = (
        link_selection_path(output_binary)
        if link_plan.selection_requirements is not None
        else None
    )
    link_outputs = {"binary": output_binary}
    if selection_output is not None:
        link_outputs["selection"] = selection_output
    stored_link_fingerprint = link_fingerprints._read_link_fingerprint(
        link_fingerprint_path
    )
    external_native_fingerprint_inputs = [
        path
        for artifact in staged_external_native_artifacts
        for path in (
            artifact.staged_path,
            artifact.staged_manifest_path,
            *artifact.staged_support_paths,
            *artifact.staged_link_input_paths,
        )
    ]
    link_inputs = [
        stub_path,
        output_obj,
        resolved_runtime_lib,
        *([link_stdlib_obj] if link_stdlib_obj is not None else []),
        *external_native_fingerprint_inputs,
    ]
    try:
        validate_link_output_paths(
            {**link_outputs, "receipt": link_fingerprint_path},
            inputs=(
                *link_inputs,
                *((stdlib_obj_path,) if stdlib_obj_path is not None else ()),
                *(
                    path
                    for artifact in staged_external_native_artifacts
                    for path in (artifact.source_path, artifact.source_manifest_path)
                ),
            ),
        )
    except (OSError, ValueError) as exc:
        return None, _fail(str(exc), json_output, command="build")
    try:
        from molt.cli.link_selection_admission import _support_surface

        selection_surface = _support_surface() if selection_output is not None else None
    except (OSError, ValueError) as exc:
        return None, _fail(str(exc), json_output, command="build")
    link_tool_facts = [
        *native_link_cache_tool_facts(link_plan),
        *link_plan.sidecar_facts(),
        *(
            (link_selection_policy(selection_surface),)
            if selection_surface is not None
            else ()
        ),
    ]
    link_fingerprint = link_fingerprints._link_fingerprint(
        project_root=project_root,
        inputs=link_inputs,
        link_cmd=link_cmd,
        tool_facts=link_tool_facts,
        stored_fingerprint=(
            stored_link_fingerprint["fingerprint"] if stored_link_fingerprint else None
        ),
    )
    if capture_link_inputs and link_fingerprint is None:
        return None, _fail(
            "Link input capture requires an output-family receipt",
            json_output,
            command="build",
        )
    link_skipped = link_fingerprints._link_outputs_match(
        outputs=link_outputs,
        fingerprint=link_fingerprint,
        receipt_path=link_fingerprint_path,
    )
    # BOLT replaces the linked image with a post-link transformed artifact.
    # Always recreate the unoptimized, relocation-bearing input before another
    # BOLT run; a link fingerprint describes link inputs, not BOLT profile data.
    if link_plan.policy.bolt_requested or capture_link_inputs:
        link_skipped = False
    link_output = output_binary
    link_selection_candidate = None
    link_input_artifacts = None
    link_input_observation = None
    capture_expected = None
    if link_skipped:
        link_process = subprocess.CompletedProcess(
            args=link_cmd,
            returncode=0,
            stdout="",
            stderr="",
        )
    else:
        link_output = file_publication.staged_file_path(
            output_binary,
            purpose="native-link",
            suffix=output_binary.suffix or ".tmp",
        )
        if selection_output is not None:
            link_selection_candidate = file_publication.staged_file_path(
                selection_output, purpose="native-link"
            )
        try:
            if capture_link_inputs:
                selected_tools = native_link_tool_facts(link_plan)
                selected_linkers = [
                    row
                    for row in selected_tools
                    if row["role"] == "linker" and row["resolved"]
                ]
                selected_drivers = [
                    row
                    for row in selected_tools
                    if row["role"] == "driver" and row["resolved"]
                ]
                if len(selected_linkers) != 1 or len(selected_drivers) != 1:
                    raise ValueError("link input capture has no unique selected linker")
                selected_linker = Path(str(selected_linkers[0]["path"]))
                selected_driver = Path(str(selected_drivers[0]["path"]))
                assert daemon_job_origin is not None
                report = output_binary.with_name(
                    output_binary.name + ".molt-link-inputs.tar"
                )
                observation = output_binary.with_name(
                    output_binary.name + ".molt-link-inputs.json"
                )
                dependencies = output_binary.with_name(
                    output_binary.name + ".molt-native-inputs.d"
                )
                preparation = output_binary.with_name(
                    output_binary.name + ".molt-native-inputs.json"
                )
                link_input_artifacts = {
                    "driver_dependencies": (
                        file_publication.staged_file_path(
                            dependencies, purpose="link-inputs", suffix=".d"
                        ),
                        dependencies,
                    ),
                    "driver_preparation": (
                        file_publication.staged_file_path(
                            preparation, purpose="link-inputs", suffix=".json"
                        ),
                        preparation,
                    ),
                    "input_reproducer": (
                        file_publication.staged_file_path(
                            report, purpose="link-inputs", suffix=".tar"
                        ),
                        report,
                    ),
                    "input_observation": (
                        file_publication.staged_file_path(
                            observation, purpose="link-inputs", suffix=".json"
                        ),
                        observation,
                    ),
                }
                capture_expected = {
                    "compiler_output": (
                        output_obj,
                        {
                            "size": daemon_job_origin.output_size,
                            "sha256": daemon_job_origin.output_sha256,
                        },
                    ),
                    "runtime": (
                        resolved_runtime_lib,
                        {
                            k: v
                            for k, v in stable_regular_file_content_identity(
                                resolved_runtime_lib, label="admission runtime input"
                            ).items()
                            if k in {"size", "sha256"}
                        },
                    ),
                }
                validate_link_output_paths(
                    {
                        **link_outputs,
                        **{k: v[1] for k, v in link_input_artifacts.items()},
                    },
                    inputs=link_inputs,
                )
            with native_link_selection(
                link_plan, link_output, surface=selection_surface
            ) as selection:
                with _native_link_execution_command(
                    link_plan,
                    planned_output=output_binary,
                    execution_output=link_output,
                    selection_arguments=(
                        *(selection.arguments if selection else ()),
                        *(
                            (
                                "-Xlinker",
                                "--reproduce="
                                + str(link_input_artifacts["input_reproducer"][0]),
                                "-MD",
                                "-MF",
                                str(link_input_artifacts["driver_dependencies"][0]),
                            )
                            if link_input_artifacts is not None
                            else ()
                        ),
                    ),
                ) as execution_link_cmd:
                    link_process = _run_native_link_command(
                        link_cmd=execution_link_cmd,
                        json_output=json_output,
                        link_timeout=link_timeout,
                    )
                if link_process.returncode == 0 and link_input_artifacts is not None:
                    assert capture_expected is not None
                    preparation_record = capture_native_link_preparation(
                        link_input_artifacts["driver_dependencies"][0],
                        source=stub_path,
                        driver=selected_driver,
                        cwd=Path.cwd(),
                    )
                    link_input_artifacts["driver_preparation"][0].write_text(
                        json.dumps(
                            preparation_record, sort_keys=True, separators=(",", ":")
                        )
                        + "\n",
                        encoding="utf-8",
                    )
                    observed_inputs = observe_link_inputs(
                        link_input_artifacts["input_reproducer"][0],
                        dialect="elf-gnu",
                        linker_path=selected_linker,
                        expected=capture_expected,
                    ).as_record()
                    assert daemon_job_origin is not None
                    link_input_observation = compilation_input_observation(
                        daemon_job_origin.payload(), observed_inputs
                    )
                    link_input_artifacts["input_observation"][0].write_text(
                        json.dumps(
                            link_input_observation,
                            sort_keys=True,
                            separators=(",", ":"),
                        )
                        + "\n",
                        encoding="utf-8",
                    )
                if link_process.returncode == 0 and selection is not None:
                    assert link_selection_candidate is not None
                    selection.admit(
                        stdout=link_process.stdout,
                        stderr=link_process.stderr,
                        output=link_selection_candidate,
                    )
            if (
                link_process.returncode == 0
                and sys.platform == "darwin"
                and target.is_host
            ):
                link_process = _validate_darwin_link_output(
                    link_process=link_process,
                    link_cmd=execution_link_cmd,
                    output_binary=link_output,
                    validation_kind="magic",
                )
            if (
                link_process.returncode == 0
                and sys.platform == "darwin"
                and target.is_host
            ):
                link_process = _validate_darwin_link_output(
                    link_process=link_process,
                    link_cmd=execution_link_cmd,
                    output_binary=link_output,
                    validation_kind="dyld",
                )
        except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as exc:
            with contextlib.suppress(OSError):
                link_output.unlink()
            if link_selection_candidate is not None:
                discard_staged_output(link_selection_candidate)
            for stage, _ in (link_input_artifacts or {}).values():
                discard_staged_output(stage)
            message = (
                "Linker timed out"
                if isinstance(exc, subprocess.TimeoutExpired)
                else str(exc)
            )
            return None, _fail(message, json_output, command="build")
        if link_process.returncode != 0:
            with contextlib.suppress(OSError):
                link_output.unlink()
            if link_selection_candidate is not None:
                discard_staged_output(link_selection_candidate)
            for stage, _ in (link_input_artifacts or {}).values():
                discard_staged_output(stage)
    try:
        # Link-cache reuse and real execution both consume the same codegen
        # generation. A mutation after final preflight cannot publish success.
        runtime_codegen_binding.verify()
        if link_plan.runtime_inputs is None:
            raise ValueError("Native link plan lost its runtime custody observation")
        link_plan.runtime_inputs.verify()
    except (OSError, RuntimeError, ValueError) as exc:
        if not link_skipped:
            with contextlib.suppress(OSError):
                link_output.unlink()
        if link_selection_candidate is not None:
            discard_staged_output(link_selection_candidate)
        for stage, _ in (link_input_artifacts or {}).values():
            discard_staged_output(stage)
        return None, _fail(
            f"Native runtime changed during final linking: {exc}",
            json_output,
            command="build",
        )
    return _PreparedNativeLink(
        output_obj=output_obj,
        stub_path=stub_path,
        runtime_lib=resolved_runtime_lib,
        output_binary=output_binary,
        link_output=link_output,
        external_native_artifacts=staged_external_native_artifacts,
        link_cmd=link_cmd,
        linker_hint=linker_hint,
        normalized_target=normalized_target,
        link_fingerprint_path=link_fingerprint_path,
        link_fingerprint=link_fingerprint,
        link_skipped=link_skipped,
        link_process=link_process,
        strip_after_link=(
            link_plan.policy.symbol_strip is NativeSymbolStrip.AFTER_LINK
        ),
        link_input_artifacts=link_input_artifacts,
        link_input_observation=link_input_observation,
        link_selection=(link_selection_candidate, selection_output)
        if link_selection_candidate is not None and selection_output is not None
        else None,
    ), None
