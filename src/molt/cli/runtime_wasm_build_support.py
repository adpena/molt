from __future__ import annotations

from molt.cli.runtime_build_python import build_python_scope

import contextlib
import json
import shlex
import subprocess
from pathlib import Path
from typing import (
    Any,
    Sequence,
)

from molt.rust_toolchain import rust_flag_spans
from molt.cargo_execution_policy import source_build_disabled_reason
from molt.cli import progress as _progress
from molt.cli import wasm_link_inputs
from molt.cli.artifact_state import (
    _build_state_root,
    _runtime_fingerprint_path,
    _runtime_target_fingerprint_path,
)
from molt.cli.build_locks import _build_lock
from molt.cli.cargo_execution import (
    CargoExecutionResult,
    CargoPlanExecutionError,
    _build_slot,
    _cargo_build_env,
    _run_resolved_cargo_plan,
    _text_output,
    cargo_execution_evidence,
)
from molt.cli.command_runtime import (
    _run_completed_command,
    _run_subprocess_captured_to_tempfiles,
)
from molt.cli.compiler_metadata import _compiler_root
from molt.cli.models import _RuntimeArtifactState
from molt.cli.runtime_artifact_selection import (
    RUNTIME_STATICLIB_ARTIFACTS,
    RuntimeCrateType,
)
from molt.cli.runtime_build_identity import resolve_wasm_cpython_abi_build_identity
from molt.cli.runtime_identity_schema import runtime_build_fingerprint
from molt.cli.runtime_cargo_plan import (
    RuntimeCargoPlan,
    resolve_runtime_cargo_plan,
)
from molt.cli.runtime_fingerprints import (
    _read_runtime_fingerprint,
    _refresh_runtime_fingerprint_metadata,
    _runtime_artifact_fingerprint_matches,
    _runtime_fingerprint_metadata_needs_refresh,
    _write_runtime_fingerprint,
)
from molt.cli.runtime_paths import (
    _cargo_profile_dir,
    _cargo_target_root,
)
from molt.cli.runtime_wasm_build_policy import _resolve_wasm_cargo_profile
from molt.cli.runtime_wasm_build_timings import (
    _record_runtime_wasm_longdouble_archives,
)
from molt.cli.runtime_wasm_failure import record_runtime_wasm_failure
from molt.cli.runtime_wasm_validation import (
    _is_valid_runtime_wasm_artifact,
    _runtime_wasm_exports_satisfy,
    _runtime_wasm_missing_exports,
    _split_runtime_wasm_exports_satisfy,
    _split_runtime_wasm_missing_exports,
)
from molt.cli.wasm_link_args import (
    wasm_link_args_from_rustflags as _wasm_link_args_from_rustflags,
    wasm_link_output_arguments,
)
from molt.cli.wasm_link_args import (
    write_wasm_link_args_response_file as _write_wasm_link_args_response_file,
)
from molt.file_publication import durable_replace, staged_file_path
from molt.toolchain_identity import (
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)


def _wasm_runtime_artifact_path(target_root: Path, profile_dir: str) -> Path:
    return target_root / "wasm32-wasip1" / profile_dir / "molt_runtime.wasm"


def _wasm_runtime_staticlib_path(target_root: Path, profile_dir: str) -> Path:
    return target_root / "wasm32-wasip1" / profile_dir / "libmolt_runtime.a"


def _wasm_cpython_abi_staticlib_path(target_root: Path, profile_dir: str) -> Path:
    return target_root / "wasm32-wasip1" / profile_dir / "libmolt_cpython_abi.a"


def _runtime_target_candidates(primary: Path) -> list[Path]:
    """Enumerate Cargo primary/deps/hash shapes; receipts alone admit reuse."""
    deps = primary.parent / "deps"
    candidates = [path for path in (primary, deps / primary.name) if path.exists()]
    hashed: list[tuple[int, str, Path]] = []
    for path in deps.glob(f"{primary.stem}-*{primary.suffix}"):
        try:
            metadata = path.stat()
        except OSError:
            continue
        hashed.append((metadata.st_mtime_ns, path.name, path))
    candidates.extend(path for _mtime, _name, path in sorted(hashed, reverse=True))
    return candidates


def _wasm_cpython_abi_staticlib_candidates(
    target_root: Path, profile_dir: str
) -> list[Path]:
    return _runtime_target_candidates(
        _wasm_cpython_abi_staticlib_path(target_root, profile_dir)
    )


def _wasm_runtime_staticlib_candidates(
    target_root: Path, profile_dir: str
) -> list[Path]:
    return _runtime_target_candidates(
        _wasm_runtime_staticlib_path(target_root, profile_dir)
    )


def _wasm_runtime_deps_dir(target_root: Path, profile_dir: str) -> Path:
    return target_root / "wasm32-wasip1" / profile_dir / "deps"


def _ensure_wasm_cpython_abi_staticlib(
    *,
    project_root: Path,
    json_output: bool,
    cargo_profile: str,
    cargo_timeout: float | None,
    runtime_state: _RuntimeArtifactState | None = None,
) -> Path | None:
    root = project_root or _compiler_root()
    state = runtime_state if runtime_state is not None else _RuntimeArtifactState()
    state.runtime_wasm_build_failure = None
    command: Sequence[str] = ()
    build: CargoExecutionResult | None = None
    stage = "configuration"

    def fail(summary: str, *, timeout: subprocess.TimeoutExpired | None = None) -> None:
        record_runtime_wasm_failure(
            state,
            project_root=root,
            stage=f"cpython-abi-{stage}",
            summary=summary,
            command=command,
            stdout=(
                _text_output(timeout.stdout)
                if timeout is not None
                else build.stdout
                if build is not None
                else ""
            ),
            stderr=(
                _text_output(timeout.stderr)
                if timeout is not None
                else build.stderr
                if build is not None
                else ""
            ),
            returncode=build.returncode if build is not None else None,
            timed_out=timeout is not None
            or bool(build is not None and getattr(build, "timed_out", False)),
            details=(
                {"cargo_execution": cargo_execution_evidence(build)}
                if build is not None
                else None
            ),
        )

    with build_python_scope(state):
        try:
            cargo_profile = _resolve_wasm_cargo_profile(cargo_profile)
            profile_dir = _cargo_profile_dir(cargo_profile)
            target_root = _cargo_target_root(root)
            staticlib_path = _wasm_cpython_abi_staticlib_path(target_root, profile_dir)
            target_label = "wasm32-wasip1.cpython-abi"
            fingerprint_path = _runtime_fingerprint_path(
                root, staticlib_path, cargo_profile, target_label
            )
            env = _cargo_build_env()
            env["CARGO_TARGET_DIR"] = str(target_root)
            cmd = [
                env.get("CARGO", "cargo"),
                "rustc",
                "--package",
                "molt-lang-cpython-abi",
                "--profile",
                cargo_profile,
                "--target",
                "wasm32-wasip1",
                "--lib",
            ]
            RUNTIME_STATICLIB_ARTIFACTS.select_in(cmd)
            command = _cargo_cmd_with_json_artifact_messages(cmd)
            stage = "cargo-plan"
            plan = resolve_runtime_cargo_plan(
                root,
                env=env,
                cargo_command=command,
                requested_target="wasm32-wasip1",
                rustflags_transform=lambda flags: _wasm_runtime_codegen_flags(
                    flags,
                    simd_enabled=True,
                    freestanding=False,
                ),
            )
            command = plan.command
            stage = "effective-configuration"
            sysroot_raw = plan.environment.get(
                "MOLT_WASI_SYSROOT"
            ) or plan.environment.get("WASI_SYSROOT")
            if not sysroot_raw:
                return fail("CPython ABI wasm build has no resolved WASI sysroot.")

            def resolve_identity():
                return resolve_wasm_cpython_abi_build_identity(
                    root,
                    env=plan.environment,
                    cargo_profile=cargo_profile,
                    target_triple="wasm32-wasip1",
                    rustflags=shlex.join(plan.rustflags),
                    cargo_command=plan.command,
                    cargo_plan=plan,
                    artifact_selection=RUNTIME_STATICLIB_ARTIFACTS,
                    wasi_sysroot=Path(sysroot_raw),
                    build_python_admission=state.build_python_admission,
                )

            stage = "lock"
            lock_name = f"runtime.{cargo_profile}.wasm32-wasip1.cpython-abi"
            build_state_root = _build_state_root(root)
            with _build_lock(
                root,
                lock_name,
                default_timeout_s=cargo_timeout if cargo_timeout is not None else 300.0,
            ):
                stage = "pre-build-identity"
                pre_identity = resolve_identity()
                fingerprint = runtime_build_fingerprint(pre_identity)
                stage = "metadata-admission"
                stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
                current = _current_runtime_target_artifact(
                    _wasm_cpython_abi_staticlib_candidates(target_root, profile_dir),
                    build_state_root=build_state_root,
                    cargo_profile=cargo_profile,
                    target_label=target_label,
                    fingerprint=fingerprint,
                )
                if current is not None:
                    stage = "target-admission"
                    if resolve_identity() != pre_identity:
                        return fail(
                            "CPython ABI wasm identity changed during target admission."
                        )
                    return current[0]
                if _runtime_artifact_fingerprint_matches(
                    staticlib_path,
                    fingerprint,
                    fingerprint_path,
                    require_artifact_digest=True,
                ):
                    if _runtime_fingerprint_metadata_needs_refresh(
                        stored_fingerprint,
                        fingerprint,
                    ):
                        stage = "metadata-refresh"
                        _refresh_runtime_fingerprint_metadata(
                            fingerprint_path,
                            fingerprint,
                        )
                    stage = "artifact-admission"
                    if resolve_identity() != pre_identity:
                        return fail(
                            "CPython ABI wasm identity changed during artifact admission."
                        )
                    return staticlib_path
                if reason := source_build_disabled_reason("CPython ABI WASM provider"):
                    stage = "rebuild-policy"
                    return fail(reason)
                if not json_output:
                    _progress.notice("WASM CPython ABI provider needs a source build")
                stage = "cargo-execution"
                with _build_slot() as _slot:
                    build = _run_resolved_cargo_plan(
                        plan,
                        timeout=cargo_timeout,
                        json_output=json_output,
                        label="CPython ABI wasm build",
                        tempfile_runner=_run_subprocess_captured_to_tempfiles,
                        progress_label=None
                        if json_output
                        else "CPython ABI wasm build",
                    )
                if build.returncode != 0:
                    return fail("CPython ABI wasm build failed.")
                stage = "cargo-artifact"
                provider = _reported_cpython_abi_staticlib_from_cargo_stdout(
                    build.stdout,
                    target_root=target_root,
                )
                if provider is None or not provider.exists():
                    return fail(
                        "CPython ABI wasm build succeeded but Cargo did not report the staticlib artifact."
                    )
                stage = "post-build-identity"
                if resolve_identity() != pre_identity:
                    return fail(
                        "CPython ABI wasm identity changed during Cargo; refusing publication."
                    )
                stage = "metadata-publication"
                fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
                _write_runtime_fingerprint(
                    fingerprint_path,
                    fingerprint,
                    artifact=provider,
                )
                provider_fingerprint_path = _runtime_target_fingerprint_path(
                    build_state_root,
                    provider,
                    cargo_profile=cargo_profile,
                    target_label=target_label,
                )
                provider_fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
                _write_runtime_fingerprint(
                    provider_fingerprint_path,
                    fingerprint,
                    artifact=provider,
                )
                return provider
        except CargoPlanExecutionError as exc:
            build = exc.cargo_result
            return fail(f"CPython ABI wasm exact Cargo plan changed: {exc}")
        except subprocess.TimeoutExpired as exc:
            return fail(f"CPython ABI wasm {stage} timed out: {exc}", timeout=exc)
        except (OSError, ValueError, subprocess.SubprocessError, RuntimeError) as exc:
            return fail(f"CPython ABI wasm {stage} failed: {exc}")


def _wasm_runtime_wasm_candidates(target_root: Path, profile_dir: str) -> list[Path]:
    return _runtime_target_candidates(
        _wasm_runtime_artifact_path(target_root, profile_dir)
    )


def _current_runtime_target_artifact(
    candidates: Sequence[Path],
    *,
    build_state_root: Path,
    cargo_profile: str,
    target_label: str,
    fingerprint: dict[str, Any],
) -> tuple[Path, Path] | None:
    for candidate in candidates:
        fingerprint_path = _runtime_target_fingerprint_path(
            build_state_root,
            candidate,
            cargo_profile=cargo_profile,
            target_label=target_label,
        )
        stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
        if _runtime_artifact_fingerprint_matches(
            candidate,
            fingerprint,
            fingerprint_path,
            require_artifact_digest=True,
        ):
            if _runtime_fingerprint_metadata_needs_refresh(
                stored_fingerprint,
                fingerprint,
            ):
                with contextlib.suppress(OSError):
                    _refresh_runtime_fingerprint_metadata(
                        fingerprint_path,
                        fingerprint,
                    )
            return candidate, fingerprint_path
    return None


def _runtime_cargo_report_missing_artifact_path(
    target_root: Path,
    profile_dir: str,
    artifact_kind: RuntimeCrateType,
) -> Path:
    suffix = "a" if artifact_kind is RuntimeCrateType.STATICLIB else "wasm"
    return (
        _wasm_runtime_deps_dir(target_root, profile_dir)
        / f".molt_runtime.cargo-report-missing.{suffix}"
    )


def _cargo_cmd_with_json_artifact_messages(cmd: Sequence[str]) -> list[str]:
    if any(arg.startswith("--message-format") for arg in cmd):
        return list(cmd)
    try:
        rustc_arg_index = list(cmd).index("--")
    except ValueError:
        return [*cmd, "--message-format=json-render-diagnostics"]
    return [
        *cmd[:rustc_arg_index],
        "--message-format=json-render-diagnostics",
        *cmd[rustc_arg_index:],
    ]


def _reported_runtime_artifact_matches(
    path: Path,
    *,
    target_root: Path,
    artifact_kind: RuntimeCrateType,
) -> bool:
    try:
        resolved_path = path.resolve(strict=False)
        resolved_root = target_root.resolve(strict=False)
    except OSError:
        return False
    if not (
        resolved_path == resolved_root or resolved_path.is_relative_to(resolved_root)
    ):
        return False
    name = resolved_path.name
    if artifact_kind is RuntimeCrateType.STATICLIB:
        return name == "libmolt_runtime.a" or (
            name.startswith("libmolt_runtime-") and name.endswith(".a")
        )
    return name == "molt_runtime.wasm" or (
        name.startswith("molt_runtime-") and name.endswith(".wasm")
    )


def _reported_runtime_artifact_from_cargo_stdout(
    stdout: str,
    *,
    target_root: Path,
    artifact_kind: RuntimeCrateType,
) -> Path | None:
    return _reported_runtime_artifacts_from_cargo_stdout(
        stdout,
        target_root=target_root,
    ).get(artifact_kind)


def _reported_cargo_artifact_paths_from_stdout(
    stdout: str,
    *,
    target_root: Path,
    package_marker: str,
    target_names: frozenset[str],
) -> tuple[Path, ...]:
    """Return in-target artifact paths from the matching Cargo package report."""
    reported: list[Path] = []
    try:
        resolved_root = target_root.resolve(strict=False)
    except OSError:
        return ()
    for line in stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if (
            not isinstance(message, dict)
            or message.get("reason") != "compiler-artifact"
        ):
            continue
        target = message.get("target")
        target_name = target.get("name") if isinstance(target, dict) else None
        package_id = message.get("package_id")
        package_text = package_id if isinstance(package_id, str) else ""
        if target_name not in target_names or package_marker not in package_text:
            continue
        filenames = message.get("filenames")
        if not isinstance(filenames, list):
            continue
        for filename in filenames:
            if not isinstance(filename, str) or not filename:
                continue
            path = Path(filename)
            if not path.is_absolute():
                path = target_root / path
            try:
                resolved_path = path.resolve(strict=False)
            except OSError:
                continue
            if resolved_path == resolved_root or resolved_path.is_relative_to(
                resolved_root
            ):
                reported.append(path)
    return tuple(reported)


def _reported_runtime_artifacts_from_cargo_stdout(
    stdout: str,
    *,
    target_root: Path,
) -> dict[RuntimeCrateType, Path]:
    """Return the exact runtime crate-type artifacts reported by this Cargo run."""
    reported: dict[RuntimeCrateType, Path] = {}
    paths = _reported_cargo_artifact_paths_from_stdout(
        stdout,
        target_root=target_root,
        package_marker="molt-runtime",
        target_names=frozenset({"molt_runtime", "molt-runtime"}),
    )
    for path in paths:
        for kind in (RuntimeCrateType.CDYLIB, RuntimeCrateType.STATICLIB):
            if _reported_runtime_artifact_matches(
                path,
                target_root=target_root,
                artifact_kind=kind,
            ):
                reported[kind] = path
    return reported


def _reported_cpython_abi_staticlib_from_cargo_stdout(
    stdout: str,
    *,
    target_root: Path,
) -> Path | None:
    paths = _reported_cargo_artifact_paths_from_stdout(
        stdout,
        target_root=target_root,
        package_marker="molt-lang-cpython-abi",
        target_names=frozenset({"molt_cpython_abi", "molt-lang-cpython-abi"}),
    )
    reported: Path | None = None
    for path in paths:
        name = path.name
        if name == "libmolt_cpython_abi.a" or (
            name.startswith("libmolt_cpython_abi-") and name.endswith(".a")
        ):
            reported = path
    return reported


def _wasm_runtime_codegen_flags(
    flags: tuple[str, ...], *, simd_enabled: bool, freestanding: bool
) -> tuple[str, ...]:
    """Apply the requested target policy after all caller feature toggles.

    Rust combines repeated target-feature options and the last toggle wins.
    Preserve caller tokens, including unrelated features and their spelling;
    the runtime coordinate owns only reference-types and SIMD128.
    """
    spans = tuple(rust_flag_spans(flags))
    last_feature = next(
        (
            span.codegen
            for span in reversed(spans)
            if span.codegen is not None and span.codegen.startswith("target-feature=")
        ),
        None,
    )
    boundary = next(
        (span.start for span in spans if flags[span.start] == "--"), len(flags)
    )
    result = list(flags[:boundary])
    has_backend = any(
        span.option == "--cfg"
        and span.value is not None
        and span.value.startswith("getrandom_backend=")
        for span in spans
    )
    if freestanding and not has_backend:
        result.extend(("--cfg", 'getrandom_backend="unsupported"'))
    policy = "target-feature=-reference-types," + (
        "+simd128" if simd_enabled else "-simd128"
    )
    # SDK resource/mode normalization may follow this directive. Only a later
    # target-feature directive can require applying the coordinate again.
    if last_feature != policy:
        result.extend(("-C", policy))
    return (*result, *flags[boundary:])


def wasm_runtime_simd_enabled(flags: Sequence[str]) -> bool:
    """Read the last explicit SIMD128 toggle in resolved Rust codegen options."""
    enabled = False
    for span in rust_flag_spans(flags):
        if span.codegen is None or not span.codegen.startswith("target-feature="):
            continue
        for feature in span.codegen.removeprefix("target-feature=").split(","):
            if feature in {"+simd128", "-simd128"}:
                enabled = feature == "+simd128"
    return enabled


def _run_runtime_wasm_cargo_build(
    *,
    cargo_plan: RuntimeCargoPlan,
    cargo_timeout: float | None,
    profile_dir: str,
    target_root_override: Path | None = None,
    json_output: bool,
    artifact_kind: RuntimeCrateType = RuntimeCrateType.CDYLIB,
) -> tuple[subprocess.CompletedProcess[str], Path]:
    target_root = Path(cargo_plan.environment["CARGO_TARGET_DIR"])
    if target_root_override is not None and target_root_override != target_root:
        raise ValueError(
            "runtime WASM target directory crossed its resolved Cargo plan"
        )
    with _build_slot() as _slot:
        build = _run_resolved_cargo_plan(
            cargo_plan,
            timeout=cargo_timeout,
            json_output=json_output,
            label="Runtime wasm build",
            tempfile_runner=_run_subprocess_captured_to_tempfiles,
            progress_label=None if json_output else "Runtime wasm build",
        )
    reported_artifact = _reported_runtime_artifact_from_cargo_stdout(
        build.stdout,
        target_root=target_root,
        artifact_kind=artifact_kind,
    )
    if reported_artifact is None:
        reported_artifact = _runtime_cargo_report_missing_artifact_path(
            target_root,
            profile_dir,
            artifact_kind,
        )
    return build, reported_artifact


class RuntimeWasmLinkError(ValueError):
    def __init__(
        self,
        message: str,
        *,
        command: tuple[str, ...] = (),
        process: subprocess.CompletedProcess[str] | None = None,
        timeout_error: subprocess.TimeoutExpired | None = None,
    ) -> None:
        super().__init__(message)
        self.command = command
        self.process = process
        output = timeout_error if timeout_error is not None else process
        self.stdout = _text_output(None if output is None else output.stdout)
        self.stderr = _text_output(None if output is None else output.stderr)
        self.timed_out = timeout_error is not None or bool(
            getattr(process, "timed_out", False)
        )


def _link_runtime_staticlib_to_reloc_wasm(
    *,
    staticlib_path: Path,
    output_path: Path,
    json_output: bool,
    link_timeout: float | None,
    cargo_plan: RuntimeCargoPlan,
    export_link_args: str = "",
) -> bool:
    c_abi = cargo_plan.wasi_c_abi
    if c_abi is None:
        raise ValueError("runtime WASM relink requires its admitted SDK")
    wasm_ld = str(c_abi.linker)
    libc_archive = c_abi.path("libc")
    _record_runtime_wasm_longdouble_archives("selected-sdk-members")
    staticlib_path = staticlib_path.resolve(strict=False)
    libc_archive = libc_archive.resolve(strict=False)
    output_path = output_path.resolve(strict=False)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    tmp_output_path = staged_file_path(output_path, purpose="wasm-reloc")
    # All runtime families capture the complete mandatory archive closure.
    long_double_argv = wasm_link_inputs.long_double_whole_archive_link_argv(
        wasm_link_inputs.LongDoubleLinkPolicy(
            c_abi.path("long_double"), c_abi.path("compiler_rt")
        ),
        whole_archive=[str(staticlib_path)],
        trailing=[str(libc_archive)],
    )
    export_args = _wasm_link_args_from_rustflags(export_link_args)
    export_response_identity = None
    if export_args:
        export_response_path = _write_wasm_link_args_response_file(
            output_path.parent / ".molt_link_args",
            label=f"{output_path.stem}.reloc",
            link_args=export_args,
        )
        export_response_identity = stable_regular_file_identity(
            export_response_path, label="runtime WASM export response"
        )
        export_args = [f"@{export_response_path}"]
    command = (
        wasm_ld,
        "-r",
        *export_args,
        *long_double_argv,
        *wasm_link_output_arguments(output_path, staged_output=tmp_output_path),
    )
    process = None
    staticlib_identity = stable_regular_file_identity(
        staticlib_path, label="runtime WASM staticlib link input"
    )
    try:
        if export_response_identity is not None:
            verify_stable_regular_file_identity(
                export_response_identity, label="runtime WASM export response"
            )
        process = _run_completed_command(
            list(command),
            cwd=output_path.parent,
            env=dict(cargo_plan.environment),
            capture_output=True,
            memory_guard_prefix="MOLT_WASM_LINK",
            timeout=link_timeout,
        )
        if process.returncode != 0:
            raise RuntimeWasmLinkError(
                "Runtime relocatable wasm link failed", command=command, process=process
            )
        verify_stable_regular_file_identity(
            staticlib_identity, label="runtime WASM staticlib link input"
        )
        if export_response_identity is not None:
            verify_stable_regular_file_identity(
                export_response_identity, label="runtime WASM export response"
            )
        if not _is_valid_runtime_wasm_artifact(tmp_output_path):
            raise RuntimeWasmLinkError(
                f"Runtime relocatable wasm artifact is invalid/incomplete: {tmp_output_path}",
                command=command,
                process=process,
            )
        durable_replace(tmp_output_path, output_path)
    except RuntimeWasmLinkError:
        raise
    except subprocess.TimeoutExpired as exc:
        raise RuntimeWasmLinkError(
            str(exc), command=command, timeout_error=exc
        ) from exc
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        raise RuntimeWasmLinkError(str(exc), command=command, process=process) from exc
    finally:
        with contextlib.suppress(OSError):
            if tmp_output_path.exists():
                tmp_output_path.unlink()
    return True


def _runtime_exports_satisfy_for_mode(
    path: Path,
    required_exports: set[str] | frozenset[str] | None,
    *,
    reloc: bool,
) -> bool:
    if reloc:
        return _runtime_wasm_exports_satisfy(path, required_exports)
    return _split_runtime_wasm_exports_satisfy(path, required_exports)


def _runtime_missing_exports_for_mode(
    path: Path,
    required_exports: set[str] | frozenset[str] | None,
    *,
    reloc: bool,
) -> set[str]:
    if reloc:
        return _runtime_wasm_missing_exports(path, required_exports)
    return _split_runtime_wasm_missing_exports(path, required_exports)
