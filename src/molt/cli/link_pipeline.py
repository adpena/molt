from __future__ import annotations

import contextlib
import hashlib
import os
import subprocess
import sys
import time
import uuid
from pathlib import Path
from typing import Any, Collection, Sequence, cast

from molt.cli.artifact_state import _artifact_state_path
from molt.cli.config_resolution import DEFAULT_STDLIB_PROFILE
from molt.cli.backend_cache import (
    _shared_stdlib_cache_matches_key_locked,
    _stage_shared_stdlib_object_for_link,
)
from molt.cli.backend_execution import _backend_bin_path
from molt.cli.cargo_profiles import _resolve_backend_cargo_profile_name
from molt.cli.command_runtime import _load_cli_harness_memory_guard, _run_completed_command
from molt.cli.external_native import _stage_external_package_native_artifacts_for_build
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
    _build_native_link_command,
    _build_native_link_driver_command,
    _windows_coff_library_command,
)
from molt.cli.native_link_deps import _native_target_is_windows
from molt.cli.native_main_stub import _render_native_main_stub
from molt.cli.output import CliFailure as _CliFailure
from molt.cli.output import fail as _fail
from molt.cli.runtime_fingerprints import (
    _artifact_needs_rebuild,
    _hash_runtime_file,
    _hash_source_tree_metadata,
    _read_runtime_fingerprint,
    _stored_fingerprint_matches_source_metadata,
)
from molt.cli.runtime_paths import _runtime_lib_path
from molt.cli.atomic_io import _write_text_if_changed


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

def _run_native_link_command(
    *,
    link_cmd: Sequence[str],
    json_output: bool,
    link_timeout: float | None,
) -> subprocess.CompletedProcess[str]:
    result = _run_completed_command(
        list(link_cmd),
        capture_output=json_output,
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

def _run_native_partial_link_command(
    *,
    input_objects: Sequence[Path],
    output_path: Path,
    json_output: bool,
    link_timeout: float | None,
    target_triple: str | None = None,
    sysroot_path: Path | None = None,
) -> subprocess.CompletedProcess[str]:
    if _native_target_is_windows(target_triple):
        link_cmd = _windows_coff_library_command(
            input_objects=input_objects,
            output_path=output_path,
        )
        return _run_native_link_command(
            link_cmd=link_cmd,
            json_output=json_output,
            link_timeout=link_timeout,
        )
    primary_object = input_objects[0] if input_objects else None
    link_cmd, _linker_hint, _normalized_target = _build_native_link_driver_command(
        output_obj=primary_object,
        target_triple=target_triple,
        sysroot_path=None,
        profile="dev",
    )
    link_cmd = [arg for arg in link_cmd if not arg.startswith("-fuse-ld=")]
    link_cmd.extend(
        ["-Wl,-r", "-o", str(output_path), *[str(path) for path in input_objects]]
    )
    return _run_native_link_command(
        link_cmd=link_cmd,
        json_output=json_output,
        link_timeout=link_timeout,
    )

def _prepare_native_object_artifact(
    *,
    output_artifact: Path,
    artifacts_root: Path,
    stdlib_obj_path: Path | None,
    stdlib_object_cache_key: str | None,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None = None,
    json_output: bool,
    link_timeout: float | None,
    target_triple: str | None = None,
    sysroot_path: Path | None = None,
) -> tuple[Path | None, subprocess.CompletedProcess[str] | None, _CliFailure | None]:
    if stdlib_obj_path is None or not stdlib_obj_path.exists():
        return output_artifact, None, None
    if not _shared_stdlib_cache_matches_key_locked(
        stdlib_obj_path,
        stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
    ):
        return (
            None,
            None,
            _fail(
                "Shared stdlib cache mismatch before native object link",
                json_output,
                command="build",
            ),
        )
    merged_output = artifacts_root / (
        f".{output_artifact.stem}_linked."
        f"{os.getpid()}.{uuid.uuid4().hex}{output_artifact.suffix}"
    )
    try:
        link_process = _run_native_partial_link_command(
            input_objects=[output_artifact, stdlib_obj_path],
            output_path=merged_output,
            json_output=json_output,
            link_timeout=link_timeout,
            target_triple=target_triple,
            sysroot_path=sysroot_path,
        )
    except subprocess.TimeoutExpired:
        with contextlib.suppress(OSError):
            if merged_output.exists():
                merged_output.unlink()
        return (
            None,
            None,
            _fail(
                "Native object partial link timed out",
                json_output,
                command="build",
            ),
        )
    except RuntimeError as exc:
        with contextlib.suppress(OSError):
            if merged_output.exists():
                merged_output.unlink()
        return None, None, _fail(str(exc), json_output, command="build")
    if link_process.returncode != 0:
        with contextlib.suppress(OSError):
            if merged_output.exists():
                merged_output.unlink()
        err = (link_process.stderr or "").strip() or (link_process.stdout or "").strip()
        msg = "Native object partial link failed"
        if err:
            msg = f"{msg}: {err}"
        return None, link_process, _fail(msg, json_output, command="build")
    try:
        os.replace(merged_output, output_artifact)
    finally:
        with contextlib.suppress(OSError):
            if merged_output.exists():
                merged_output.unlink()
    return output_artifact, link_process, None

def _retry_native_link_without_hint(
    *,
    link_cmd: Sequence[str],
    linker_hint: str | None,
    json_output: bool,
    link_timeout: float | None,
) -> tuple[subprocess.CompletedProcess[str] | None, list[str]]:
    if linker_hint is None:
        return None, list(link_cmd)
    retry_cmd = [
        arg
        for arg in link_cmd
        if arg != f"-fuse-ld={linker_hint}" and arg != "-Wl,--icf=safe"
    ]
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

def _prepare_native_link(
    *,
    output_artifact: Path,
    trusted: bool,
    capabilities_list: list[str] | None,
    artifacts_root: Path,
    json_output: bool,
    output_binary: Path | None,
    runtime_lib: Path | None,
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
    stdlib_profile: str | None = DEFAULT_STDLIB_PROFILE,
) -> tuple[_PreparedNativeLink | None, _CliFailure | None]:
    output_obj = output_artifact
    link_stdlib_obj = stdlib_obj_path
    if stdlib_obj_path is not None:
        if stdlib_obj_path.exists() and not _shared_stdlib_cache_matches_key_locked(
            stdlib_obj_path,
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
        ):
            return None, _fail(
                "Shared stdlib cache key mismatch before native link",
                json_output,
                command="build",
            )
        if stdlib_obj_path.exists() and stdlib_obj_path.parent != artifacts_root:
            try:
                staged_stdlib_obj = _stage_shared_stdlib_object_for_link(
                    stdlib_obj_path,
                    stdlib_object_cache_key=stdlib_object_cache_key,
                    stdlib_object_manifest=stdlib_object_manifest,
                    stdlib_module_symbols=stdlib_module_symbols,
                    artifacts_root=artifacts_root,
                )
            except OSError as exc:
                return None, _fail(
                    f"Failed to stage shared stdlib object for native link: {exc}",
                    json_output,
                    command="build",
                )
            link_stdlib_obj = staged_stdlib_obj
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
        trusted=trusted,
        capabilities_list=capabilities_list,
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
        link_cmd, linker_hint, normalized_target = _build_native_link_command(
            output_obj=output_obj,
            stub_path=stub_path,
            runtime_lib=resolved_runtime_lib,
            output_binary=output_binary,
            target_triple=target_triple,
            sysroot_path=sysroot_path,
            profile=profile,
            stdlib_obj_path=link_stdlib_obj,
        )
    except RuntimeError as exc:
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
        print(f"native-link cmd: {link_cmd}", file=sys.stderr)
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
    external_native_fingerprint_inputs = [
        path
        for artifact in staged_external_native_artifacts
        for path in (
            artifact.staged_path,
            artifact.staged_manifest_path,
            *artifact.staged_support_paths,
        )
    ]
    link_fingerprint = _link_fingerprint(
        project_root=project_root,
        inputs=[
            stub_path,
            output_obj,
            resolved_runtime_lib,
            *(
                [link_stdlib_obj]
                if link_stdlib_obj is not None and link_stdlib_obj.exists()
                else []
            ),
            *external_native_fingerprint_inputs,
        ],
        link_cmd=link_cmd,
        stored_fingerprint=stored_link_fingerprint,
    )
    link_skipped = not _artifact_needs_rebuild(
        output_binary,
        link_fingerprint,
        stored_link_fingerprint,
    )
    # Staleness guard: even when the fingerprint matches, the cached binary
    # may be stale if ANY link input was rebuilt after the binary was linked.
    # This catches backend changes that produce identical .o files (from TIR
    # cache) but changed runtime internals. Comparing mtimes is O(1) and
    # eliminates the entire class of stale-binary bugs.
    if link_skipped and output_binary.exists():
        try:
            binary_mtime = output_binary.stat().st_mtime
            # Include the backend binary: when function_compiler.rs changes,
            # the backend is rebuilt, which changes how .o files are generated.
            # Even if the .o content is identical (TIR cache), the binary must
            # be relinked because the runtime library was also rebuilt.
            backend_cargo_profile, _ = _resolve_backend_cargo_profile_name(profile)
            backend_bin = _backend_bin_path(molt_root, backend_cargo_profile)
            deps = [
                resolved_runtime_lib,
                output_obj,
                stub_path,
                backend_bin,
                *external_native_fingerprint_inputs,
            ]
            for dep in deps:
                if dep.exists() and dep.stat().st_mtime > binary_mtime:
                    link_skipped = False
                    break
        except OSError:
            pass
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
            return None, _fail("Linker timed out", json_output, command="build")
        if link_process.returncode != 0 and linker_hint is not None:
            try:
                retry_process, _ = _retry_native_link_without_hint(
                    link_cmd=link_cmd,
                    linker_hint=linker_hint,
                    json_output=json_output,
                    link_timeout=link_timeout,
                )
            except subprocess.TimeoutExpired:
                return None, _fail("Linker timed out", json_output, command="build")
            if retry_process is not None and retry_process.returncode == 0:
                warnings.append(
                    f"Linker fallback: -fuse-ld={linker_hint} failed; retried default linker."
                )
                link_process = retry_process
        if (
            link_process.returncode == 0
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
                return None, _fail("Linker timed out", json_output, command="build")
        if (
            link_process.returncode == 0
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
                return None, _fail("Linker timed out", json_output, command="build")
    return _PreparedNativeLink(
        output_obj=output_obj,
        stub_path=stub_path,
        runtime_lib=resolved_runtime_lib,
        output_binary=output_binary,
        external_native_artifacts=staged_external_native_artifacts,
        link_cmd=link_cmd,
        linker_hint=linker_hint,
        normalized_target=normalized_target,
        link_fingerprint_path=link_fingerprint_path,
        link_fingerprint=link_fingerprint,
        link_skipped=link_skipped,
        link_process=link_process,
    ), None

def _link_fingerprint(
    *,
    project_root: Path,
    inputs: list[Path],
    link_cmd: list[str],
    stored_fingerprint: dict[str, Any] | None = None,
) -> dict[str, str | None] | None:
    inputs_meta = _hash_source_tree_metadata(inputs, project_root)
    inputs_digest = inputs_meta[0] if inputs_meta is not None else None
    meta_digest = hashlib.sha256("\0".join(link_cmd).encode("utf-8")).hexdigest()
    if _stored_fingerprint_matches_source_metadata(
        stored_fingerprint,
        inputs_digest=inputs_digest,
        rustc=None,
        meta_digest=meta_digest,
    ):
        assert stored_fingerprint is not None
        return {
            "hash": cast(str, stored_fingerprint.get("hash")),
            "rustc": None,
            "inputs_digest": inputs_digest,
            "meta_digest": meta_digest,
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
        "meta_digest": meta_digest,
    }
