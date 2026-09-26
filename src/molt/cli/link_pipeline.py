from __future__ import annotations

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
from molt.cli.config_resolution import DEFAULT_RUNTIME_STDLIB_PROFILE
from molt.cli.backend_cache import (
    _stage_shared_stdlib_object_for_link,
)
from molt.cli.command_runtime import (
    _load_cli_harness_memory_guard,
    _run_completed_command,
)
from molt.cli.external_native import (
    _external_native_link_requirements,
    _stage_external_package_native_artifacts_for_build,
)
from molt.cli.models import (
    BuildProfile,
    _ExternalPackageNativeArtifactPlan,
    _PreparedNativeLink,
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
)
from molt.cli.native_binary import (
    _darwin_binary_imports_validation_error,
    _darwin_binary_magic_error,
)
from molt.cli.native_link_command import (
    _build_native_link_plan,
)
from molt.cli.native_link_plan import (
    _host_target_triple,
    NativeArtifactKind,
    native_link_execution_command as _native_link_execution_command,
    resolve_native_target_spec,
    validate_native_object_artifact,
)
from molt.cli.native_link_tool_identity import native_link_cache_tool_facts
from molt.cli.native_main_stub import _render_native_main_stub
from molt.cli.output import CliFailure as _CliFailure
from molt.cli.output import fail as _fail
from molt.link_outputs import link_selection_path, validate_link_output_paths
from molt.cli.link_selection_admission import (
    link_selection_policy,
    native_link_selection,
)
from molt.cli.runtime_paths import _runtime_lib_path
from molt.cli.runtime_build_identity import RuntimeBuildIdentity
from molt.cli.atomic_io import _write_text_if_changed


def _run_native_link_command(
    *,
    link_cmd: Sequence[str],
    json_output: bool,
    link_timeout: float | None,
) -> subprocess.CompletedProcess[str]:
    result = _run_completed_command(
        list(link_cmd),
        capture_output=True,
        env=None,
        cwd=None,
        timeout=link_timeout,
        memory_guard_prefix="MOLT_BUILD",
    )
    harness_memory_guard = _load_cli_harness_memory_guard(None)
    if (
        link_timeout is not None
        and result.returncode == harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE
    ):
        raise subprocess.TimeoutExpired(
            list(link_cmd),
            link_timeout,
            output=result.stdout,
            stderr=result.stderr,
        )
    return result


def _prepare_native_object_artifact(
    *,
    output_artifact: Path,
    stdlib_obj_path: Path | None,
    json_output: bool,
    target_triple: str | None = None,
) -> tuple[Path | None, _CliFailure | None]:
    if stdlib_obj_path is not None:
        return None, _fail(
            "Native object output cannot include a separately compiled stdlib; "
            "--emit obj requires one compilation unit.",
            json_output,
            command="build",
        )
    try:
        validate_native_object_artifact(
            output_artifact, resolve_native_target_spec(target_triple)
        )
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
    runtime_lib: Path | None,
    runtime_build_identity: RuntimeBuildIdentity,
    molt_root: Path,
    runtime_cargo_profile: str,
    target_triple: str | None,
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
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    bolt_requested: bool = False,
) -> tuple[_PreparedNativeLink | None, _CliFailure | None]:
    output_obj = output_artifact
    link_stdlib_obj = stdlib_obj_path
    if stdlib_obj_path is not None:
        # Admission and snapshotting are one locked transaction. A prior probe
        # cannot authorize a later read, even when the cache lives in this build.
        try:
            link_stdlib_obj = _stage_shared_stdlib_object_for_link(
                stdlib_obj_path,
                stdlib_object_cache_key=stdlib_object_cache_key,
                stdlib_object_manifest=stdlib_object_manifest,
                stdlib_module_symbols=stdlib_module_symbols,
                artifacts_root=artifacts_root,
                target_triple=target_triple,
            )
        except OSError as exc:
            # Built-in formatting preserves staging cleanup notes at the real
            # text/JSON consumer boundary without expanding a traceback tree.
            detail = "".join(traceback.format_exception_only(exc)).rstrip()
            return None, _fail(
                f"Failed to stage shared stdlib archive for native link: {detail}",
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
    resolved_runtime_lib = runtime_lib
    if resolved_runtime_lib is None:
        resolved_runtime_lib = _runtime_lib_path(
            molt_root,
            runtime_cargo_profile,
            target_triple,
            stdlib_profile=stdlib_profile,
        )
    try:
        link_plan = _build_native_link_plan(
            output_obj=output_obj,
            output_kind=NativeArtifactKind.ARCHIVE,
            stdlib_kind=NativeArtifactKind.ARCHIVE,
            stub_path=stub_path,
            runtime_lib=resolved_runtime_lib,
            output_binary=output_binary,
            target_triple=target_triple,
            sysroot_path=sysroot_path,
            profile=profile,
            runtime_build_identity=runtime_build_identity,
            stdlib_obj_path=link_stdlib_obj,
            external_link_requirements=(
                _external_native_link_requirements(
                    staged_external_native_artifacts,
                    target_triple=target_triple or _host_target_triple(),
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
    link_cmd = list(link_plan.command)
    linker_hint = link_plan.linker_hint
    normalized_target = link_plan.normalized_target
    if (
        normalized_target is not None
        and target_triple is not None
        and normalized_target != target_triple
    ):
        warnings.append(
            f"Zig target normalized to {normalized_target} from {target_triple}."
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
    link_skipped = link_fingerprints._link_outputs_match(
        outputs=link_outputs,
        fingerprint=link_fingerprint,
        receipt_path=link_fingerprint_path,
    )
    # BOLT replaces the linked image with a post-link transformed artifact.
    # Always recreate the unoptimized, relocation-bearing input before another
    # BOLT run; a link fingerprint describes link inputs, not BOLT profile data.
    if link_plan.policy.bolt_requested:
        link_skipped = False
    link_output = output_binary
    link_selection_candidate = None
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
        if diagnostics_enabled and "link" not in phase_starts:
            phase_starts["link"] = time.perf_counter()
        try:
            with native_link_selection(
                link_plan, link_output, surface=selection_surface
            ) as selection:
                with _native_link_execution_command(
                    link_plan,
                    planned_output=output_binary,
                    execution_output=link_output,
                    selection_arguments=selection.arguments if selection else (),
                ) as execution_link_cmd:
                    link_process = _run_native_link_command(
                        link_cmd=execution_link_cmd,
                        json_output=json_output,
                        link_timeout=link_timeout,
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
                and not target_triple
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
                and not target_triple
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
        strip_after_link=link_plan.policy.strip_after_link,
        link_selection=(link_selection_candidate, selection_output)
        if link_selection_candidate is not None and selection_output is not None
        else None,
    ), None
