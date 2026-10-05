from __future__ import annotations

from molt.cli import progress as _progress
from molt.cli.runtime_build_python import BuildPythonAdmission, build_python_scope

import json
import os
import re
import subprocess
import sys
import time
import uuid
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path
from typing import Collection, Mapping, Sequence

from molt.cargo_execution_policy import source_build_disabled_reason
from molt.cli.artifact_state import _build_state_root, _canonical_target_root
from molt.cli.atomic_io import (
    _atomic_write_json,
)
from molt.cli.build_locks import _build_lock, BuildLockAcquisitionError
from molt.cli.cargo_execution import (
    CargoExecutionResult,
    CargoPlanExecutionError,
    _build_slot,
    _cargo_build_env,
    _run_resolved_cargo_plan,
    _text_output,
    cargo_execution_evidence,
)
from molt.cli.config_resolution import (
    DEFAULT_RUNTIME_STDLIB_PROFILE,
)
from molt.cli.diagnostic_text import strip_terminal_decoration
from molt.cli.installed_runtime import (
    InstalledRuntimeCell,
    admit_installed_native_runtime,
    installed_native_runtime_identity,
    reuse_installed_native_admission,
    select_installed_native_runtime,
)
from molt.cli.models import (
    _NativeRuntimeBuildFailure,
    _RuntimeArtifactState,
)
from molt.cli.native_link_custody import NativeLinkCustodyError
from molt.cli.native_link_manifest import (
    NativeLinkDependencyManifestError,
    read_native_link_dependency_manifest,
)
from molt.cli.runtime_artifact_selection import (
    RUNTIME_STATICLIB_ARTIFACTS,
)
from molt.cli.runtime_build_identity import resolve_native_runtime_build_identity
from molt.cli.runtime_identity_schema import (
    RuntimeBuildIdentity,
    require_native_runtime_staticlib_identity,
)
from molt.cli.runtime_cargo_plan import RuntimeCargoPlan, resolve_runtime_cargo_plan
from molt.cli.runtime_features import (
    _runtime_builtin_features_for_profile,
    _runtime_cargo_features,
    runtime_cargo_feature_for_profile,
    runtime_fingerprint_features_for_profile,
)
from molt.cli.runtime_native_generation import (
    NativeRuntimeGeneration,
    publish_native_runtime_generation,
    read_native_runtime_generation,
)
from molt.cli.runtime_paths import (
    _cargo_profile_dir,
    _cargo_target_root,
    _runtime_cargo_scratch_lib_path,
)


_NATIVE_RUNTIME_READY_EXECUTOR: ThreadPoolExecutor | None = None
_NATIVE_RUNTIME_EVIDENCE_TEXT_LIMIT = 256 * 1024
_NATIVE_RUNTIME_SUMMARY_LIMIT = 2_000


def _bounded_native_runtime_evidence_text(text: str) -> str:
    if len(text) <= _NATIVE_RUNTIME_EVIDENCE_TEXT_LIMIT:
        return text
    half = _NATIVE_RUNTIME_EVIDENCE_TEXT_LIMIT // 2
    omitted = len(text) - (half * 2)
    return (
        text[:half]
        + f"\n... <{omitted} chars omitted from durable evidence> ...\n"
        + text[-half:]
    )


def _native_runtime_first_error(
    *,
    cargo_stdout: str,
    cargo_stderr: str,
    fallback: str,
) -> str:
    """Extract one bounded actionable diagnostic from Cargo's machine output."""
    for raw in cargo_stdout.splitlines():
        try:
            payload = json.loads(raw)
        except json.JSONDecodeError:
            continue
        if not isinstance(payload, dict) or payload.get("reason") != "compiler-message":
            continue
        diagnostic = payload.get("message")
        if not isinstance(diagnostic, dict) or diagnostic.get("level") != "error":
            continue
        rendered = diagnostic.get("rendered")
        message = diagnostic.get("message")
        selected = (
            rendered if isinstance(rendered, str) and rendered.strip() else message
        )
        if isinstance(selected, str) and selected.strip():
            return strip_terminal_decoration(selected.strip())[
                :_NATIVE_RUNTIME_SUMMARY_LIMIT
            ]

    clean_stderr = strip_terminal_decoration(cargo_stderr)
    lines = [line.rstrip() for line in clean_stderr.splitlines() if line.strip()]
    error_line = next(
        (
            line.strip()
            for line in lines
            if re.match(r"^\s*(?:error(?:\[[A-Z0-9]+\])?|fatal error):", line)
        ),
        None,
    )
    terminal_line = next(
        (
            line.strip()
            for line in reversed(lines)
            if re.search(
                r"(?:process didn't exit successfully|signal:\s*(?:\d+\s*,\s*)?SIG|"
                r"out of memory|memory allocation|LLVM ERROR|killed)",
                line,
                flags=re.IGNORECASE,
            )
        ),
        None,
    )
    compact_terminal: str | None = None
    if terminal_line is not None:
        termination = re.search(
            r"\((?:exit status|signal):[^)]*\)\s*$",
            terminal_line,
            flags=re.IGNORECASE,
        )
        if termination is not None:
            compact_terminal = f"process termination: {termination.group(0)}"
        elif len(terminal_line) > _NATIVE_RUNTIME_SUMMARY_LIMIT // 2:
            compact_terminal = (
                "terminal diagnostic tail: "
                + terminal_line[-(_NATIVE_RUNTIME_SUMMARY_LIMIT // 2) :]
            )
        else:
            compact_terminal = terminal_line
    selected = "\n".join(
        dict.fromkeys(
            part
            for part in (error_line, compact_terminal, fallback)
            if isinstance(part, str) and part.strip()
        )
    )
    return selected[:_NATIVE_RUNTIME_SUMMARY_LIMIT]


def _record_native_runtime_failure(
    runtime_state: _RuntimeArtifactState | None,
    *,
    project_root: Path,
    stage: str,
    summary: str,
    command: Sequence[str] | None = None,
    cargo_stdout: str = "",
    cargo_stderr: str = "",
    returncode: int | None = None,
    timed_out: bool = False,
    cargo_result: CargoExecutionResult | None = None,
    emit_diagnostic: bool = False,
) -> bool:
    """Publish one bounded durable failure record and attach it to build state."""
    execution = (
        cargo_execution_evidence(cargo_result)
        if cargo_result is not None
        else {
            "schema": "molt.cargo-execution.v1",
            "attempt_count": 0,
            "retry_reason": None,
            "timed_out": timed_out,
            "duration_seconds": None,
            "peak_process_rss_bytes": None,
            "peak_tree_rss_bytes": None,
            "signal": None,
            "attempts": [],
        }
    )
    signal_value = execution.get("signal")
    execution_signal: dict[str, object] | None = None
    if isinstance(signal_value, dict):
        signal_entries: dict[str, object] = {}
        for key, value in signal_value.items():
            if not isinstance(key, str):
                signal_entries.clear()
                break
            signal_entries[key] = value
        else:
            execution_signal = signal_entries
    duration_value = execution.get("duration_seconds")
    execution_duration = (
        float(duration_value) if isinstance(duration_value, (int, float)) else None
    )
    process_rss_value = execution.get("peak_process_rss_bytes")
    execution_process_rss = (
        int(process_rss_value) if isinstance(process_rss_value, int) else None
    )
    tree_rss_value = execution.get("peak_tree_rss_bytes")
    execution_tree_rss = (
        int(tree_rss_value) if isinstance(tree_rss_value, int) else None
    )
    execution_timed_out = timed_out or bool(execution.get("timed_out", False))
    attempt_count_value = execution.get("attempt_count")
    execution_attempt_count = (
        attempt_count_value
        if isinstance(attempt_count_value, int)
        and not isinstance(attempt_count_value, bool)
        and attempt_count_value >= 0
        else 0
    )
    retry_reason_value = execution.get("retry_reason")
    execution_retry_reason = (
        retry_reason_value if isinstance(retry_reason_value, str) else None
    )
    evidence_path: Path | None = None
    try:
        evidence_path = (
            _build_state_root(project_root)
            / "build_failures"
            / f"native-runtime-{stage}-{os.getpid()}-{uuid.uuid4().hex}.json"
        )
        _atomic_write_json(
            evidence_path,
            {
                "schema_version": 2,
                "schema": "molt.native-runtime-build-failure.v2",
                "kind": "molt_native_runtime_build_failure",
                "stage": stage,
                "summary": summary[:_NATIVE_RUNTIME_SUMMARY_LIMIT],
                "command": list(command or ()),
                "cwd": str(project_root),
                "returncode": returncode,
                "timed_out": execution_timed_out,
                "signal": execution_signal,
                "duration_seconds": execution_duration,
                "peak_process_rss_bytes": execution_process_rss,
                "peak_tree_rss_bytes": execution_tree_rss,
                "cargo_execution": execution,
                "cargo_stdout": _bounded_native_runtime_evidence_text(cargo_stdout),
                "cargo_stderr": _bounded_native_runtime_evidence_text(cargo_stderr),
            },
            indent=2,
            sort_keys=True,
        )
    except OSError as exc:
        evidence_path = None
        detail = f"Failure evidence publication failed: {exc}"
        summary = (
            summary[: _NATIVE_RUNTIME_SUMMARY_LIMIT // 2]
            + "; "
            + detail[: _NATIVE_RUNTIME_SUMMARY_LIMIT // 2 - 2]
        )
    if emit_diagnostic or (evidence_path is None and runtime_state is None):
        location = (
            f" Evidence: {evidence_path}"
            if evidence_path is not None
            else " Evidence publication failed."
        )
        print(f"{summary[:_NATIVE_RUNTIME_SUMMARY_LIMIT]}{location}", file=sys.stderr)
    if runtime_state is not None:
        runtime_state.native_runtime_build_failure = _NativeRuntimeBuildFailure(
            stage=stage,
            summary=summary[:_NATIVE_RUNTIME_SUMMARY_LIMIT],
            evidence_path=evidence_path,
            returncode=returncode,
            timed_out=execution_timed_out,
            signal=execution_signal,
            duration_seconds=execution_duration,
            peak_process_rss_bytes=execution_process_rss,
            peak_tree_rss_bytes=execution_tree_rss,
            attempt_count=execution_attempt_count,
            retry_reason=execution_retry_reason,
        )
        if runtime_state.build_python_admission is not None:
            runtime_state.build_python_admission.record_failure(
                runtime_state.native_runtime_build_failure
            )
    return False


def _record_runtime_build_stage_ms(
    stage_timings_ms: dict[str, float] | None,
    name: str,
    started_at: float,
) -> None:
    if stage_timings_ms is None:
        return
    stage_timings_ms[name] = round(
        max(0.0, (time.perf_counter() - started_at) * 1000.0),
        6,
    )


def _native_runtime_ready_executor() -> ThreadPoolExecutor:
    global _NATIVE_RUNTIME_READY_EXECUTOR
    if _NATIVE_RUNTIME_READY_EXECUTOR is None:
        _NATIVE_RUNTIME_READY_EXECUTOR = ThreadPoolExecutor(
            max_workers=1,
            thread_name_prefix="molt-runtime-ready",
        )
    return _NATIVE_RUNTIME_READY_EXECUTOR


def _maybe_start_native_runtime_lib_ready_async(
    runtime_state: _RuntimeArtifactState,
    *,
    target_triple: str | None,
    json_output: bool,
    runtime_cargo_profile: str,
    molt_root: Path,
    cargo_timeout: float | None,
    diagnostics_enabled: bool,
    phase_starts: dict[str, float] | None,
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    resolved_modules: set[str] | frozenset[str] | None = None,
) -> None:
    runtime_lib = runtime_state.runtime_lib
    if runtime_lib is None or runtime_state.runtime_lib_ready_future is not None:
        return
    if (
        diagnostics_enabled
        and phase_starts is not None
        and "runtime_setup" not in phase_starts
    ):
        phase_starts["runtime_setup"] = time.perf_counter()
    runtime_state.runtime_lib_ready_future = _native_runtime_ready_executor().submit(
        _progress.background_task(_ensure_runtime_lib_ready),
        runtime_state,
        target_triple=target_triple,
        json_output=json_output,
        runtime_cargo_profile=runtime_cargo_profile,
        molt_root=molt_root,
        cargo_timeout=cargo_timeout,
        stdlib_profile=stdlib_profile,
        resolved_modules=resolved_modules,
    )


def _ensure_runtime_lib_ready(
    runtime_state: _RuntimeArtifactState,
    *,
    target_triple: str | None,
    json_output: bool,
    runtime_cargo_profile: str,
    molt_root: Path,
    cargo_timeout: float | None,
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    resolved_modules: Collection[str] | None = None,
    stage_timings_ms: dict[str, float] | None = None,
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
        stdlib_profile=stdlib_profile,
        resolved_modules=resolved_modules,
        extra_runtime_features=runtime_state.extra_runtime_features,
        stage_timings_ms=stage_timings_ms,
        runtime_state=runtime_state,
    )


def _ensure_native_runtime_lib_ready_for_codegen(
    runtime_state: _RuntimeArtifactState,
    *,
    target_triple: str | None,
    json_output: bool,
    runtime_cargo_profile: str,
    molt_root: Path,
    cargo_timeout: float | None,
    diagnostics_enabled: bool,
    phase_starts: dict[str, float],
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    resolved_modules: set[str] | frozenset[str] | None = None,
    stage_timings_ms: dict[str, float] | None = None,
) -> bool:
    runtime_lib = runtime_state.runtime_lib
    if runtime_lib is None:
        return True
    if runtime_state.runtime_lib_ready_future is not None:
        if diagnostics_enabled and "runtime_setup" not in phase_starts:
            phase_starts["runtime_setup"] = time.perf_counter()
        try:
            with _progress.subprocess_status(
                None
                if json_output or runtime_state.runtime_lib_ready_future.done()
                else f"Preparing native runtime ({runtime_cargo_profile})"
            ):
                ready = bool(runtime_state.runtime_lib_ready_future.result())
            return ready and runtime_state.native_runtime_build_identity is not None
        finally:
            runtime_state.runtime_lib_ready_future = None
    if diagnostics_enabled and "runtime_setup" not in phase_starts:
        phase_starts["runtime_setup"] = time.perf_counter()
    ready = _ensure_runtime_lib_ready(
        runtime_state,
        target_triple=target_triple,
        json_output=json_output,
        runtime_cargo_profile=runtime_cargo_profile,
        molt_root=molt_root,
        cargo_timeout=cargo_timeout,
        stdlib_profile=stdlib_profile,
        resolved_modules=resolved_modules,
        stage_timings_ms=stage_timings_ms,
    )
    return ready and runtime_state.native_runtime_build_identity is not None


def _ensure_native_runtime_lib_ready_before_link(
    runtime_state: _RuntimeArtifactState,
    *,
    target_triple: str | None,
    json_output: bool,
    runtime_cargo_profile: str,
    molt_root: Path,
    cargo_timeout: float | None,
    diagnostics_enabled: bool,
    phase_starts: dict[str, float],
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    resolved_modules: set[str] | frozenset[str] | None = None,
) -> bool:
    """Revalidate codegen's generation; never select or build a replacement.

    A source checkout admits the archive and manifest against the original
    live codegen binding, then recaptures every current runtime, configuration,
    toolchain input through a fresh Cargo plan and verifies the operation's
    isolated Python admission; generation fences then close again. Installed Molt
    re-derives only the cell selection for this request and fences the retained
    generation this operation already admitted. The shipped cell is never
    admitted again beneath compiled code; the final link consumes the admitted
    generation's receipt and custody.
    """
    del cargo_timeout, resolved_modules
    if diagnostics_enabled and "runtime_setup" not in phase_starts:
        phase_starts["runtime_setup"] = time.perf_counter()
    binding = runtime_state.native_runtime_codegen_binding
    admission = runtime_state.installed_native_admission
    try:
        if binding is None:
            raise ValueError("native runtime has no admitted codegen generation")
        if (
            runtime_state.runtime_lib != binding.runtime_lib
            or runtime_state.native_runtime_build_identity != binding.build_identity
            or runtime_state.runtime_lib_ready_future is not None
        ):
            raise ValueError("native runtime selection changed after code generation")
        binding.verify()
        installed = select_installed_native_runtime(
            molt_root,
            target_triple=target_triple,
            cargo_profile=runtime_cargo_profile,
            stdlib_profile=stdlib_profile,
            extra_runtime_features=runtime_state.extra_runtime_features,
        )
        if installed is not None or admission is not None:
            if (
                installed is None
                or admission is None
                or admission.archive != binding.archive
                or admission.build_identity != binding.build_identity
            ):
                raise ValueError(
                    "native runtime selection changed after code generation"
                )
            reuse_installed_native_admission(installed, admission)
        else:
            read_native_link_dependency_manifest(
                binding.runtime_lib,
                cargo_profile=runtime_cargo_profile,
                target_triple=target_triple,
                runtime_build_identity=binding.build_identity,
            )
            current = current_native_runtime_build_identity(
                molt_root,
                binding.runtime_lib,
                target_triple=target_triple,
                cargo_profile=runtime_cargo_profile,
                stdlib_profile=stdlib_profile,
                extra_runtime_features=runtime_state.extra_runtime_features,
                build_python_admission=runtime_state.build_python_admission,
            )
            binding.verify()
            if current != binding.build_identity:
                raise ValueError(
                    "native runtime inputs changed after code generation; "
                    "refusing to link against a different runtime"
                )
    except (OSError, ValueError, NativeLinkDependencyManifestError) as exc:
        # A failed generation must not authorize any later link attempt.
        runtime_state.revoke_native_runtime_admission()
        return _record_native_runtime_failure(
            runtime_state,
            project_root=molt_root,
            stage="codegen-link-admission",
            summary=f"Native runtime codegen admission failed: {exc}",
            emit_diagnostic=not json_output,
        )
    return True


def _native_runtime_cargo_command(
    *,
    cargo_profile: str,
    concrete_stdlib_profile: str,
    runtime_features: Sequence[str],
    builtin_features: Sequence[str],
    concrete_stdlib_feature: str,
    target_triple: str | None,
) -> list[str]:
    """Return the exact Cargo command for a complete native runtime generation."""
    cmd = [
        "cargo",
        "rustc",
        # This command is a machine protocol: ``native_link_manifest`` parses
        # rustc's native-static-libs note.  CI sets CARGO_TERM_COLOR=always,
        # which otherwise decorates the note with ANSI escapes and makes the
        # exact-prefix parser report a false missing-note failure after a
        # successful multi-minute build.
        "--color=never",
        "-p",
        "molt-runtime",
        "--profile",
        cargo_profile,
        "--message-format=json-render-diagnostics",
    ]
    if concrete_stdlib_profile != "full":
        cmd.append("--no-default-features")
        concrete_features = dict.fromkeys(
            list(runtime_features) + list(builtin_features) + [concrete_stdlib_feature]
        )
        cmd.extend(["--features", ",".join(concrete_features)])
    elif target_triple and "wasm" in target_triple:
        cmd.append("--no-default-features")
        wasm_features = list(runtime_features) + [
            "stdlib_crypto",
            "stdlib_compression",
            "stdlib_serialization",
            "stdlib_archive",
            "stdlib_ast",
            "stdlib_fs_extra",
            "builtin_set",
            "builtin_complex",
            "builtin_memoryview",
            "builtin_contextvars",
            "builtin_fcntl",
        ]
        cmd.extend(["--features", ",".join(wasm_features)])
    else:
        full_features = dict.fromkeys(
            list(runtime_features) + [concrete_stdlib_feature]
        )
        cmd.extend(["--features", ",".join(full_features)])
    if target_triple:
        cmd.extend(["--target", target_triple])
    RUNTIME_STATICLIB_ARTIFACTS.select_in(cmd)
    cmd.extend(["--", "--print", "native-static-libs"])
    return cmd


def _runtime_build_identity_for_plan(
    project_root: Path,
    *,
    env: Mapping[str, str],
    cargo_profile: str,
    target_triple: str | None,
    runtime_features: tuple[str, ...],
    cargo_command: Sequence[str],
    cargo_plan: RuntimeCargoPlan,
    build_python_admission: BuildPythonAdmission | None = None,
) -> RuntimeBuildIdentity:
    return resolve_native_runtime_build_identity(
        project_root,
        env=env,
        cargo_profile=cargo_profile,
        target_triple=target_triple,
        runtime_features=runtime_features,
        cargo_command=cargo_command,
        artifact_selection=RUNTIME_STATICLIB_ARTIFACTS,
        cargo_plan=cargo_plan,
        build_python_admission=build_python_admission,
    )


@dataclass(slots=True)
class _NativeRuntimeBuildPlan:
    runtime_lib: Path
    target_triple: str | None
    json_output: bool
    cargo_profile: str
    project_root: Path
    cargo_timeout: float | None
    stage_timings_ms: dict[str, float] | None
    runtime_state: _RuntimeArtifactState | None
    cargo_plan: RuntimeCargoPlan
    fingerprint_features: tuple[str, ...]
    build_identity: RuntimeBuildIdentity
    candidates: tuple[NativeRuntimeGeneration, ...]
    build_python_admission: BuildPythonAdmission | None = None

    @property
    def cmd(self) -> list[str]:
        return list(self.cargo_plan.command)

    @property
    def build_env(self) -> dict[str, str]:
        return dict(self.cargo_plan.environment)

    def record_execution_failure(
        self, error: OSError | ValueError, *, stage: str
    ) -> bool:
        result = (
            error.cargo_result if isinstance(error, CargoPlanExecutionError) else None
        )
        summary = f"Cannot execute exact runtime Cargo plan: {error}"
        if not self.json_output:
            print(summary, file=sys.stderr)
        return _record_native_runtime_failure(
            self.runtime_state,
            project_root=self.project_root,
            stage=stage,
            summary=summary,
            command=self.cmd,
            cargo_stdout=result.stdout if result is not None else "",
            cargo_stderr=result.stderr if result is not None else "",
            returncode=result.returncode if result is not None else None,
            cargo_result=result,
        )

    def record_timeout_failure(
        self, error: subprocess.TimeoutExpired, *, stage: str, label: str
    ) -> bool:
        """Retain partial output from the exact-plan Cargo build."""
        stdout = _text_output(error.stdout)
        stderr = _text_output(error.stderr)
        summary = _native_runtime_first_error(
            cargo_stdout=stdout,
            cargo_stderr=stderr,
            fallback=f"{label} timed out after {error.timeout}s.",
        )
        return _record_native_runtime_failure(
            self.runtime_state,
            project_root=self.project_root,
            stage=stage,
            summary=summary,
            command=self.cmd,
            cargo_stdout=stdout,
            cargo_stderr=stderr,
            timed_out=True,
            emit_diagnostic=not self.json_output,
        )

    def build_permitted(self) -> bool:
        if reason := source_build_disabled_reason("Native runtime"):
            return _record_native_runtime_failure(
                self.runtime_state,
                project_root=self.project_root,
                stage="rebuild-policy",
                summary=reason,
                command=self.cmd,
                emit_diagnostic=not self.json_output,
            )
        return True

    def resolve_build_identity(self) -> RuntimeBuildIdentity:
        return _runtime_build_identity_for_plan(
            self.project_root,
            env=self.build_env,
            cargo_profile=self.cargo_profile,
            target_triple=self.target_triple,
            runtime_features=self.fingerprint_features,
            cargo_command=self.cmd,
            cargo_plan=self.cargo_plan,
            build_python_admission=self.build_python_admission,
        )

    def identity_is_current(self, *, stage: str) -> bool:
        started = time.perf_counter()
        try:
            current = self.resolve_build_identity()
        except (OSError, ValueError) as exc:
            summary = f"Runtime native build identity revalidation failed: {exc}"
            if not self.json_output:
                print(summary, file=sys.stderr)
            return _record_native_runtime_failure(
                self.runtime_state,
                project_root=self.project_root,
                stage=stage,
                summary=summary,
                command=self.cmd,
            )
        finally:
            _record_runtime_build_stage_ms(
                self.stage_timings_ms, "runtime_lib_identity_publication", started
            )
        if current != self.build_identity:
            summary = (
                "Runtime native build inputs changed during the build transaction; "
                "refusing artifact admission."
            )
            if not self.json_output:
                print(summary, file=sys.stderr)
            return _record_native_runtime_failure(
                self.runtime_state,
                project_root=self.project_root,
                stage=stage,
                summary=summary,
                command=self.cmd,
            )
        return True

    def accept(self, generation: NativeRuntimeGeneration) -> bool:
        """Reuse the operation's input capture and fence its admitted generation."""
        try:
            if generation.build_identity != self.build_identity:
                raise ValueError("native runtime generation has another build identity")
            generation.verify()
        except (OSError, ValueError) as exc:
            return _record_native_runtime_failure(
                self.runtime_state,
                project_root=self.project_root,
                stage="generation-admission",
                summary=f"Native runtime generation admission failed: {exc}",
                command=self.cmd,
                emit_diagnostic=not self.json_output,
            )
        if self.runtime_state is not None:
            self.runtime_state.runtime_lib = generation.runtime_lib
            self.runtime_state.native_runtime_build_identity = generation.build_identity
        return True


def _native_runtime_generation_candidates(
    runtime_lib: Path,
    *,
    project_root: Path,
    cargo_profile: str,
    target_triple: str | None,
) -> tuple[NativeRuntimeGeneration, ...]:
    canonical = _canonical_target_root(project_root)
    if target_triple:
        canonical /= target_triple
    canonical = canonical / _cargo_profile_dir(cargo_profile) / runtime_lib.name
    candidates = []
    for coordinate in dict.fromkeys((runtime_lib, canonical)):
        generation = read_native_runtime_generation(
            coordinate, cargo_profile=cargo_profile, target_triple=target_triple
        )
        if generation is not None:
            candidates.append(generation)
    return tuple(candidates)


def _prepare_native_runtime_build(
    runtime_lib: Path,
    target_triple: str | None,
    json_output: bool,
    cargo_profile: str,
    project_root: Path,
    cargo_timeout: float | None,
    *,
    stdlib_profile: str | None,
    extra_runtime_features: Sequence[str] | None,
    stage_timings_ms: dict[str, float] | None,
    runtime_state: _RuntimeArtifactState | None,
    read_generations: bool = True,
    build_python_admission: BuildPythonAdmission | None = None,
) -> _NativeRuntimeBuildPlan | None:
    if runtime_state is not None:
        runtime_state.revoke_native_runtime_admission()
        runtime_state.native_runtime_build_failure = None
    runtime_features = tuple(
        dict.fromkeys(
            [*_runtime_cargo_features(target_triple), *(extra_runtime_features or ())]
        )
    )
    concrete_stdlib_profile = stdlib_profile or DEFAULT_RUNTIME_STDLIB_PROFILE
    fingerprint_features = runtime_fingerprint_features_for_profile(
        concrete_stdlib_profile,
        target_triple=target_triple,
        extra_runtime_features=extra_runtime_features,
    )
    cmd = _native_runtime_cargo_command(
        cargo_profile=cargo_profile,
        concrete_stdlib_profile=concrete_stdlib_profile,
        runtime_features=runtime_features,
        builtin_features=_runtime_builtin_features_for_profile(
            stdlib_profile, target_triple=target_triple
        ),
        concrete_stdlib_feature=runtime_cargo_feature_for_profile(
            concrete_stdlib_profile
        ),
        target_triple=target_triple,
    )
    build_env = _cargo_build_env()
    build_env["CARGO_TARGET_DIR"] = str(_cargo_target_root(project_root))
    try:
        cargo_plan = resolve_runtime_cargo_plan(
            project_root,
            env=build_env,
            cargo_command=cmd,
            requested_target=target_triple,
        )
        cmd = list(cargo_plan.command)
        build_env = dict(cargo_plan.environment)
    except (OSError, ValueError) as exc:
        summary = f"Failed to resolve the exact runtime Cargo plan: {exc}"
        if not json_output:
            print(summary, file=sys.stderr)
        _record_native_runtime_failure(
            runtime_state,
            project_root=project_root,
            stage="build-identity",
            summary=summary,
        )
        return None
    started = time.perf_counter()
    candidates = (
        _native_runtime_generation_candidates(
            runtime_lib,
            project_root=project_root,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
        )
        if read_generations
        else ()
    )
    _record_runtime_build_stage_ms(
        stage_timings_ms, "runtime_lib_generation_read", started
    )
    # Candidate bytes are admitted first. This fresh capture is their current
    # expectation, never an expectation copied from a stored receipt.
    started = time.perf_counter()
    try:
        build_identity = _runtime_build_identity_for_plan(
            project_root,
            env=build_env,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            runtime_features=fingerprint_features,
            cargo_command=cmd,
            cargo_plan=cargo_plan,
            build_python_admission=build_python_admission,
        )
    except (OSError, ValueError) as exc:
        summary = f"Failed to compute exact runtime native build identity: {exc}"
        if not json_output:
            print(summary, file=sys.stderr)
        _record_native_runtime_failure(
            runtime_state,
            project_root=project_root,
            stage="build-identity",
            summary=summary,
            command=cmd,
        )
        return None
    _record_runtime_build_stage_ms(
        stage_timings_ms, "runtime_lib_identity_initial", started
    )
    return _NativeRuntimeBuildPlan(
        runtime_lib=runtime_lib,
        target_triple=target_triple,
        json_output=json_output,
        cargo_profile=cargo_profile,
        project_root=project_root,
        cargo_timeout=cargo_timeout,
        stage_timings_ms=stage_timings_ms,
        runtime_state=runtime_state,
        cargo_plan=cargo_plan,
        fingerprint_features=fingerprint_features,
        build_identity=build_identity,
        build_python_admission=build_python_admission,
        candidates=candidates,
    )


def current_native_runtime_build_identity(
    project_root: Path,
    runtime_lib: Path,
    *,
    target_triple: str | None,
    cargo_profile: str,
    stdlib_profile: str | None,
    extra_runtime_features: Sequence[str] | None = None,
    build_python_admission: BuildPythonAdmission | None = None,
) -> RuntimeBuildIdentity:
    """Resolve the exact current native-runtime build-plan identity.

    Artifact consumers use this authority instead of accepting a receipt's
    stored identity as its own expectation. For installed Molt the shipped
    cell's canonical receipt is that authority; no Cargo plan is resolved.
    A build operation's final link reuses its installed admission instead.
    """

    installed = select_installed_native_runtime(
        project_root,
        target_triple=target_triple,
        cargo_profile=cargo_profile,
        stdlib_profile=stdlib_profile,
        extra_runtime_features=extra_runtime_features,
    )
    if installed is not None:
        return installed_native_runtime_identity(installed, runtime_lib)
    plan = _prepare_native_runtime_build(
        runtime_lib,
        target_triple,
        True,
        cargo_profile,
        project_root,
        None,
        stdlib_profile=stdlib_profile,
        extra_runtime_features=extra_runtime_features,
        stage_timings_ms=None,
        runtime_state=None,
        read_generations=False,
        build_python_admission=build_python_admission,
    )
    if plan is None:
        raise ValueError("cannot resolve the current native-runtime build identity")
    return require_native_runtime_staticlib_identity(
        plan.build_identity,
        cargo_profile=cargo_profile,
        target_triple=target_triple,
        artifact_selection=RUNTIME_STATICLIB_ARTIFACTS,
    )


def _publish_native_runtime_build(
    plan: _NativeRuntimeBuildPlan,
    build: CargoExecutionResult,
) -> bool:
    started = time.perf_counter()
    try:
        generation = publish_native_runtime_generation(
            plan.runtime_lib,
            source_archive=_runtime_cargo_scratch_lib_path(
                plan.runtime_lib, plan.target_triple
            ),
            cargo_stdout=build.stdout,
            cargo_stderr=build.stderr,
            cargo_profile=plan.cargo_profile,
            target_triple=plan.target_triple,
            build_identity=plan.build_identity,
            inputs_are_current=lambda: plan.identity_is_current(
                stage="generation-publication-identity-stability"
            ),
        )
    except (
        OSError,
        ValueError,
        NativeLinkCustodyError,
        NativeLinkDependencyManifestError,
    ) as exc:
        return _record_native_runtime_failure(
            plan.runtime_state,
            project_root=plan.project_root,
            stage="generation-publication",
            summary=f"Failed to publish native runtime generation: {exc}",
            command=plan.cmd,
            cargo_stdout=build.stdout,
            cargo_stderr=build.stderr,
            returncode=build.returncode,
            cargo_result=build,
            emit_diagnostic=not plan.json_output,
        )
    finally:
        _record_runtime_build_stage_ms(
            plan.stage_timings_ms, "runtime_lib_generation_publish", started
        )
    return generation is not None and plan.accept(generation)


def _build_native_runtime_under_lock(plan: _NativeRuntimeBuildPlan) -> bool:
    if not plan.build_permitted():
        return False
    if not plan.json_output:
        _progress.notice("Native runtime artifacts need a source build")
    try:
        with _build_slot() as _slot:
            started = time.perf_counter()
            build = _run_resolved_cargo_plan(
                plan.cargo_plan,
                timeout=plan.cargo_timeout,
                json_output=plan.json_output,
                label="Runtime build",
            )
            _record_runtime_build_stage_ms(
                plan.stage_timings_ms, "runtime_lib_cargo_build", started
            )
    except subprocess.TimeoutExpired as exc:
        return plan.record_timeout_failure(
            exc,
            stage="cargo",
            label="Runtime build",
        )
    except (OSError, ValueError) as exc:
        return plan.record_execution_failure(exc, stage="cargo")
    if build.returncode != 0:
        summary = _native_runtime_first_error(
            cargo_stdout=build.stdout,
            cargo_stderr=build.stderr,
            fallback=f"Cargo exited with code {build.returncode}",
        )
        if not plan.json_output:
            print(summary, file=sys.stderr)
        return _record_native_runtime_failure(
            plan.runtime_state,
            project_root=plan.project_root,
            stage="cargo",
            summary=summary,
            command=plan.cmd,
            cargo_stdout=build.stdout,
            cargo_stderr=build.stderr,
            returncode=build.returncode,
            cargo_result=build,
        )

    return _publish_native_runtime_build(plan, build)


def _admit_installed_native_runtime(
    cell: InstalledRuntimeCell,
    runtime_lib: Path,
    *,
    project_root: Path,
    json_output: bool,
    runtime_state: _RuntimeArtifactState | None,
) -> bool:
    """Admit the shipped cell selected for this build; installed Molt never builds.

    This is the operation's one content admission; code generation and final
    link reuse it through ``runtime_state.installed_native_admission``.
    """
    try:
        if runtime_lib != cell.runtime_lib:
            raise ValueError(
                "selected native runtime path is not the installed cell's retained "
                "generation"
            )
        admission = admit_installed_native_runtime(cell)
        if admission.runtime_lib != runtime_lib:
            raise ValueError("installed native runtime retention changed generation")
    except (OSError, ValueError) as exc:
        return _record_native_runtime_failure(
            runtime_state,
            project_root=project_root,
            stage="installed-runtime-admission",
            summary=f"Installed native runtime admission failed: {exc}",
            emit_diagnostic=not json_output,
        )
    if runtime_state is not None:
        runtime_state.installed_native_admission = admission
        runtime_state.native_runtime_build_identity = admission.build_identity
    return True


def _ensure_runtime_lib(
    runtime_lib: Path,
    target_triple: str | None,
    json_output: bool,
    cargo_profile: str,
    project_root: Path,
    cargo_timeout: float | None,
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    resolved_modules: Collection[str] | None = None,
    extra_runtime_features: Sequence[str] | None = None,
    stage_timings_ms: dict[str, float] | None = None,
    runtime_state: _RuntimeArtifactState | None = None,
) -> bool:
    del resolved_modules
    try:
        installed = select_installed_native_runtime(
            project_root,
            target_triple=target_triple,
            cargo_profile=cargo_profile,
            stdlib_profile=stdlib_profile,
            extra_runtime_features=extra_runtime_features,
        )
    except ValueError as exc:
        if runtime_state is not None:
            runtime_state.revoke_native_runtime_admission()
        return _record_native_runtime_failure(
            runtime_state,
            project_root=project_root,
            stage="installed-runtime-selection",
            summary=str(exc),
            emit_diagnostic=not json_output,
        )
    if installed is not None:
        if runtime_state is not None:
            runtime_state.revoke_native_runtime_admission()
            runtime_state.native_runtime_build_failure = None
        return _admit_installed_native_runtime(
            installed,
            runtime_lib,
            project_root=project_root,
            json_output=json_output,
            runtime_state=runtime_state,
        )
    # Wait before the transaction captures inputs. A queued producer must never
    # admit a candidate against an expectation captured before its lock wait.
    if runtime_state is not None:
        runtime_state.revoke_native_runtime_admission()
        runtime_state.native_runtime_build_failure = None
    lock_start = time.perf_counter()
    try:
        with (
            build_python_scope(runtime_state) as build_python_admission,
            _build_lock(
                project_root,
                f"runtime.{cargo_profile}.{target_triple or 'native'}",
                default_timeout_s=cargo_timeout if cargo_timeout is not None else 300.0,
            ),
        ):
            plan = _prepare_native_runtime_build(
                runtime_lib,
                target_triple,
                json_output,
                cargo_profile,
                project_root,
                cargo_timeout,
                stdlib_profile=stdlib_profile,
                extra_runtime_features=extra_runtime_features,
                stage_timings_ms=stage_timings_ms,
                runtime_state=runtime_state,
                build_python_admission=build_python_admission,
            )
            if plan is None:
                build_python_admission.record_failure()
                return False
            for generation in plan.candidates:
                if generation.build_identity == plan.build_identity:
                    accepted = plan.accept(generation)
                    if not accepted:
                        build_python_admission.record_failure()
                    return accepted
            built = _build_native_runtime_under_lock(plan)
            if not built:
                build_python_admission.record_failure()
            return built
    except BuildLockAcquisitionError as exc:
        _record_runtime_build_stage_ms(
            stage_timings_ms, "runtime_lib_build_lock", lock_start
        )
        return _record_native_runtime_failure(
            runtime_state,
            project_root=project_root,
            stage="build-lock",
            summary=f"Native runtime build lock acquisition failed: {exc}",
            emit_diagnostic=not json_output,
        )
