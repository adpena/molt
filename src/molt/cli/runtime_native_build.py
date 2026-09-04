from __future__ import annotations

import contextlib
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
from typing import (
    Any,
    Collection,
    Mapping,
    Sequence,
)

from molt.cli.artifact_state import (
    _artifact_state_path_for_build_state_root,
    _build_state_root,
    _canonical_build_state_root,
    _canonical_target_root,
    _maybe_hydrate_artifact_from_canonical_target,
    _runtime_fingerprint_path,
)
from molt.cli.atomic_io import (
    _atomic_copy_file,
    _atomic_write_json,
)
from molt.cli.build_locks import _build_lock
from molt.cli.cargo_execution import (
    CargoExecutionResult,
    _build_slot,
    _cargo_build_env,
    _run_cargo_with_sccache_retry,
    cargo_execution_evidence,
)
from molt.cli.config_resolution import (
    DEFAULT_RUNTIME_STDLIB_PROFILE,
)
from molt.cli.diagnostic_text import strip_terminal_decoration
from molt.cli.models import (
    _NativeRuntimeBuildFailure,
    _RuntimeArtifactState,
)
from molt.cli.native_link_manifest import (
    NativeLinkDependencyManifestError,
    native_link_dependency_manifest_path,
    read_native_link_dependency_manifest,
    write_native_link_dependency_manifest,
)
from molt.cli.runtime_artifact_selection import (
    RUNTIME_STATICLIB_ARTIFACTS,
)
from molt.cli.runtime_features import (
    _runtime_builtin_features_for_profile,
    _runtime_cargo_features,
    runtime_cargo_feature_for_profile,
    runtime_fingerprint_features_for_profile,
)
from molt.cli.runtime_fingerprints import (
    _read_runtime_fingerprint,
    _refresh_runtime_fingerprint_metadata,
    _runtime_artifact_fingerprint_matches,
    _runtime_fingerprint,
    _runtime_fingerprint_metadata_needs_refresh,
    _write_runtime_fingerprint,
)
from molt.cli.runtime_paths import (
    _cargo_profile_dir,
    _cargo_target_root,
    _runtime_cargo_scratch_lib_path,
)
from molt.cli.static_archive_identity import (
    StaticArchiveIdentityError,
    artifact_content_identity,
)

_RuntimeLibSessionKey = tuple[
    str,
    str,
    str,
    str,
    str | None,
    str,
    tuple[str, ...],
    tuple[str | None, str | None, str | None, str | None],
]

_RUNTIME_LIB_VERIFIED: set[_RuntimeLibSessionKey] = set()
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
    except OSError:
        evidence_path = None
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
        _ensure_runtime_lib_ready,
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
    stage_timings_ms: dict[str, float] | None = None,
) -> bool:
    runtime_lib = runtime_state.runtime_lib
    if runtime_lib is None:
        return True
    if runtime_state.runtime_lib_ready_future is not None:
        if diagnostics_enabled and "runtime_setup" not in phase_starts:
            phase_starts["runtime_setup"] = time.perf_counter()
        try:
            ready = bool(runtime_state.runtime_lib_ready_future.result())
            return ready and runtime_state.native_link_source_fingerprint is not None
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
    return ready and runtime_state.native_link_source_fingerprint is not None


def _runtime_lib_verified_session_key(
    *,
    project_root: Path,
    runtime_lib: Path,
    fingerprint_path: Path,
    cargo_profile: str,
    target_triple: str | None,
    rustflags: str,
    fingerprint_features: tuple[str, ...],
    fingerprint: Mapping[str, str | None] | None,
) -> _RuntimeLibSessionKey | None:
    if fingerprint is None:
        return None
    return (
        os.fspath(project_root),
        os.fspath(runtime_lib),
        os.fspath(fingerprint_path),
        cargo_profile,
        target_triple,
        rustflags,
        fingerprint_features,
        (
            fingerprint.get("hash"),
            fingerprint.get("rustc"),
            fingerprint.get("inputs_digest"),
            fingerprint.get("meta_digest"),
        ),
    )


def _native_runtime_cargo_command(
    *,
    cargo_profile: str,
    concrete_stdlib_profile: str,
    runtime_features: Sequence[str],
    builtin_features: Sequence[str],
    concrete_stdlib_feature: str,
    target_triple: str | None,
) -> list[str]:
    """Return the one exact Cargo command used for build and manifest refresh."""
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
        concrete_features = list(
            dict.fromkeys(
                [*runtime_features, *builtin_features, concrete_stdlib_feature]
            )
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
        full_features = list(
            dict.fromkeys([*runtime_features, concrete_stdlib_feature])
        )
        cmd.extend(["--features", ",".join(full_features)])
    if target_triple:
        cmd.extend(["--target", target_triple])
    RUNTIME_STATICLIB_ARTIFACTS.select_in(cmd)
    cmd.extend(["--", "--print", "native-static-libs"])
    return cmd


def _native_link_manifest_matches(
    runtime_lib: Path,
    *,
    cargo_profile: str,
    target_triple: str | None,
    source_root: Path,
    source_fingerprint: Mapping[str, object],
) -> bool:
    try:
        read_native_link_dependency_manifest(
            runtime_lib,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            source_root=source_root,
            source_fingerprint=source_fingerprint,
        )
    except NativeLinkDependencyManifestError:
        return False
    return True


def _native_link_source_fingerprint(
    fingerprint: Mapping[str, object] | None,
) -> dict[str, object] | None:
    """Project the runtime fingerprint into the native-link source identity."""
    if fingerprint is None:
        return None
    projected = {
        key: fingerprint.get(key)
        for key in ("hash", "inputs_digest", "meta_digest", "rustc")
    }
    if any(
        not isinstance(projected[key], str) or not projected[key]
        for key in ("hash", "meta_digest", "rustc")
    ):
        return None
    inputs_digest = projected["inputs_digest"]
    if inputs_digest is not None and (
        not isinstance(inputs_digest, str) or not inputs_digest
    ):
        return None
    return projected


def _runtime_archives_semantically_match(left: Path, right: Path) -> bool:
    try:
        return artifact_content_identity(left) == artifact_content_identity(right)
    except (OSError, StaticArchiveIdentityError):
        return False


def _refresh_native_link_manifest(
    *,
    runtime_lib: Path,
    target_triple: str | None,
    cargo_profile: str,
    project_root: Path,
    cmd: list[str],
    build_env: dict[str, str],
    cargo_timeout: float | None,
    json_output: bool,
    source_fingerprint: Mapping[str, object],
    runtime_state: _RuntimeArtifactState | None = None,
) -> bool:
    """Refresh missing provenance through the exact no-op-capable Cargo command."""
    try:
        with _build_slot() as _slot:
            result = _run_cargo_with_sccache_retry(
                cmd,
                cwd=project_root,
                env=build_env,
                timeout=cargo_timeout,
                json_output=json_output,
                label="Runtime native-link manifest refresh",
            )
    except subprocess.TimeoutExpired:
        return _record_native_runtime_failure(
            runtime_state,
            project_root=project_root,
            stage="native-link-manifest-refresh",
            summary=(
                f"Cargo timed out after {cargo_timeout:.1f}s while refreshing the "
                "native-link manifest"
                if cargo_timeout is not None
                else "Cargo timed out while refreshing the native-link manifest"
            ),
            command=cmd,
            timed_out=True,
        )
    if result.returncode != 0:
        summary = _native_runtime_first_error(
            cargo_stdout=result.stdout,
            cargo_stderr=result.stderr,
            fallback=f"Cargo exited with code {result.returncode}",
        )
        if not json_output:
            print(summary, file=sys.stderr)
        return _record_native_runtime_failure(
            runtime_state,
            project_root=project_root,
            stage="native-link-manifest-refresh",
            summary=summary,
            command=cmd,
            cargo_stdout=result.stdout,
            cargo_stderr=result.stderr,
            returncode=result.returncode,
            cargo_result=result,
        )
    cargo_runtime_lib = _runtime_cargo_scratch_lib_path(runtime_lib, target_triple)
    if not _runtime_archives_semantically_match(runtime_lib, cargo_runtime_lib):
        if not json_output:
            print(
                "Runtime native-link manifest refresh produced an archive that "
                "does not match the selected runtime artifact.",
                file=sys.stderr,
            )
        return _record_native_runtime_failure(
            runtime_state,
            project_root=project_root,
            stage="native-link-manifest-refresh",
            summary=(
                "Cargo refreshed the native-link manifest but changed the selected "
                "runtime archive members"
            ),
            command=cmd,
            cargo_stdout=result.stdout,
            cargo_stderr=result.stderr,
            returncode=result.returncode,
            cargo_result=result,
        )
    try:
        write_native_link_dependency_manifest(
            result.stdout,
            cargo_stderr=result.stderr,
            runtime_lib=runtime_lib,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            source_root=project_root,
            source_fingerprint=source_fingerprint,
        )
    except (OSError, NativeLinkDependencyManifestError) as exc:
        if not json_output:
            print(
                f"Failed to publish runtime native-link manifest: {exc}",
                file=sys.stderr,
            )
        return _record_native_runtime_failure(
            runtime_state,
            project_root=project_root,
            stage="native-link-manifest-publication",
            summary=f"Failed to publish runtime native-link manifest: {exc}",
            command=cmd,
            cargo_stdout=result.stdout,
            cargo_stderr=result.stderr,
            returncode=result.returncode,
            cargo_result=result,
        )
    return True


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
    cmd: list[str]
    build_env: dict[str, str]
    fingerprint_path: Path
    stored_fingerprint: dict[str, Any] | None
    fingerprint: dict[str, Any] | None
    source_fingerprint: dict[str, object]
    session_key: _RuntimeLibSessionKey | None

    @property
    def lock_name(self) -> str:
        return f"runtime.{self.cargo_profile}.{self.target_triple or 'native'}"

    def manifest_matches(self) -> bool:
        return _native_link_manifest_matches(
            self.runtime_lib,
            cargo_profile=self.cargo_profile,
            target_triple=self.target_triple,
            source_root=self.project_root,
            source_fingerprint=self.source_fingerprint,
        )

    def refresh_manifest(self) -> bool:
        return _refresh_native_link_manifest(
            runtime_lib=self.runtime_lib,
            target_triple=self.target_triple,
            cargo_profile=self.cargo_profile,
            project_root=self.project_root,
            cmd=self.cmd,
            build_env=self.build_env,
            cargo_timeout=self.cargo_timeout,
            json_output=self.json_output,
            source_fingerprint=self.source_fingerprint,
            runtime_state=self.runtime_state,
        )

    def accept(self) -> bool:
        if self.runtime_state is not None:
            self.runtime_state.native_link_source_fingerprint = dict(
                self.source_fingerprint
            )
        if self.session_key is not None:
            _RUNTIME_LIB_VERIFIED.add(self.session_key)
        return True


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
) -> _NativeRuntimeBuildPlan | None:
    if runtime_state is not None:
        runtime_state.native_link_source_fingerprint = None
        runtime_state.native_runtime_build_failure = None
    rustflags = os.environ.get("RUSTFLAGS", "")
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
    fingerprint_path = _runtime_fingerprint_path(
        project_root, runtime_lib, cargo_profile, target_triple
    )
    started = time.perf_counter()
    stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
    _record_runtime_build_stage_ms(
        stage_timings_ms, "runtime_lib_read_fingerprint", started
    )
    started = time.perf_counter()
    fingerprint = _runtime_fingerprint(
        project_root,
        cargo_profile=cargo_profile,
        target_triple=target_triple,
        rustflags=rustflags,
        runtime_features=fingerprint_features,
        artifact_selection=RUNTIME_STATICLIB_ARTIFACTS,
        stored_fingerprint=stored_fingerprint,
    )
    _record_runtime_build_stage_ms(
        stage_timings_ms, "runtime_lib_compute_fingerprint", started
    )
    source_fingerprint = _native_link_source_fingerprint(fingerprint)
    if source_fingerprint is None:
        summary = (
            "Failed to compute an exact source/toolchain identity for the "
            "runtime native-link manifest"
        )
        if not json_output:
            print(f"{summary}.", file=sys.stderr)
        _record_native_runtime_failure(
            runtime_state,
            project_root=project_root,
            stage="source-fingerprint",
            summary=summary,
        )
        return None
    session_key = _runtime_lib_verified_session_key(
        project_root=project_root,
        runtime_lib=runtime_lib,
        fingerprint_path=fingerprint_path,
        cargo_profile=cargo_profile,
        target_triple=target_triple,
        rustflags=rustflags,
        fingerprint_features=fingerprint_features,
        fingerprint=fingerprint,
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
        cmd=cmd,
        build_env=build_env,
        fingerprint_path=fingerprint_path,
        stored_fingerprint=stored_fingerprint,
        fingerprint=fingerprint,
        source_fingerprint=source_fingerprint,
        session_key=session_key,
    )


def _reuse_native_runtime_under_lock(
    plan: _NativeRuntimeBuildPlan,
) -> bool | None:
    if plan.stored_fingerprint is None:
        started = time.perf_counter()
        plan.stored_fingerprint = _read_runtime_fingerprint(plan.fingerprint_path)
        _record_runtime_build_stage_ms(
            plan.stage_timings_ms,
            "runtime_lib_reread_fingerprint_in_lock",
            started,
        )
    started = time.perf_counter()
    matches = _runtime_artifact_fingerprint_matches(
        plan.runtime_lib,
        plan.fingerprint,
        plan.fingerprint_path,
        require_artifact_digest=True,
    )
    _record_runtime_build_stage_ms(
        plan.stage_timings_ms, "runtime_lib_artifact_match", started
    )
    if not matches:
        return None
    if plan.fingerprint is not None and _runtime_fingerprint_metadata_needs_refresh(
        plan.stored_fingerprint, plan.fingerprint
    ):
        with contextlib.suppress(OSError):
            _refresh_runtime_fingerprint_metadata(
                plan.fingerprint_path, plan.fingerprint
            )
    if not plan.manifest_matches() and not plan.refresh_manifest():
        return False
    return plan.accept()


def _hydrate_native_runtime_under_lock(
    plan: _NativeRuntimeBuildPlan,
) -> bool | None:
    canonical_target_root = _canonical_target_root(plan.project_root)
    profile_dir = _cargo_profile_dir(plan.cargo_profile)
    canonical_runtime_lib = canonical_target_root / profile_dir / plan.runtime_lib.name
    if plan.target_triple:
        canonical_runtime_lib = (
            canonical_target_root
            / plan.target_triple
            / profile_dir
            / plan.runtime_lib.name
        )
    target_label = (
        (plan.target_triple or "native").replace(os.sep, "_").replace(":", "_")
    )
    canonical_fingerprint_path = _artifact_state_path_for_build_state_root(
        _canonical_build_state_root(plan.project_root),
        canonical_runtime_lib,
        subdir="runtime_fingerprints",
        stem_suffix=f"{plan.cargo_profile}.{target_label}",
        extension="fingerprint",
    )
    started = time.perf_counter()
    hydrated = _maybe_hydrate_artifact_from_canonical_target(
        artifact=plan.runtime_lib,
        fingerprint=plan.fingerprint,
        fingerprint_path=plan.fingerprint_path,
        candidate_artifact=canonical_runtime_lib,
        candidate_fingerprint_path=canonical_fingerprint_path,
        require_artifact_digest=True,
    )
    _record_runtime_build_stage_ms(
        plan.stage_timings_ms, "runtime_lib_canonical_hydrate", started
    )
    if not hydrated:
        return None
    try:
        read_native_link_dependency_manifest(
            canonical_runtime_lib,
            cargo_profile=plan.cargo_profile,
            target_triple=plan.target_triple,
            source_root=plan.project_root,
            source_fingerprint=plan.source_fingerprint,
        )
        _atomic_copy_file(
            native_link_dependency_manifest_path(canonical_runtime_lib),
            native_link_dependency_manifest_path(plan.runtime_lib),
        )
        manifest_ready = plan.manifest_matches()
    except (OSError, NativeLinkDependencyManifestError):
        manifest_ready = False
    if not manifest_ready and not plan.refresh_manifest():
        return False
    return plan.accept()


def _publish_native_runtime_build(
    plan: _NativeRuntimeBuildPlan,
    build: CargoExecutionResult,
) -> bool:
    cargo_runtime_lib = _runtime_cargo_scratch_lib_path(
        plan.runtime_lib, plan.target_triple
    )
    if cargo_runtime_lib != plan.runtime_lib:
        if not cargo_runtime_lib.exists():
            summary = (
                "Cargo reported a successful runtime build but the staticlib "
                f"artifact is missing: {cargo_runtime_lib}"
            )
            if not plan.json_output:
                print(
                    f"Runtime build succeeded but archive is missing: {cargo_runtime_lib}",
                    file=sys.stderr,
                )
            return _record_native_runtime_failure(
                plan.runtime_state,
                project_root=plan.project_root,
                stage="cargo-artifact",
                summary=summary,
                command=plan.cmd,
                cargo_stdout=build.stdout,
                cargo_stderr=build.stderr,
                returncode=build.returncode,
                cargo_result=build,
            )
        try:
            _atomic_copy_file(cargo_runtime_lib, plan.runtime_lib)
        except OSError as exc:
            summary = (
                f"Failed to materialize runtime archive alias {plan.runtime_lib}: {exc}"
            )
            if not plan.json_output:
                print(summary, file=sys.stderr)
            return _record_native_runtime_failure(
                plan.runtime_state,
                project_root=plan.project_root,
                stage="artifact-publication",
                summary=summary,
                command=plan.cmd,
                cargo_stdout=build.stdout,
                cargo_stderr=build.stderr,
                returncode=build.returncode,
                cargo_result=build,
            )
    try:
        write_native_link_dependency_manifest(
            build.stdout,
            cargo_stderr=build.stderr,
            runtime_lib=plan.runtime_lib,
            cargo_profile=plan.cargo_profile,
            target_triple=plan.target_triple,
            source_root=plan.project_root,
            source_fingerprint=plan.source_fingerprint,
        )
    except (OSError, NativeLinkDependencyManifestError) as exc:
        summary = f"Failed to publish runtime native-link manifest: {exc}"
        if not plan.json_output:
            print(summary, file=sys.stderr)
        return _record_native_runtime_failure(
            plan.runtime_state,
            project_root=plan.project_root,
            stage="native-link-manifest-publication",
            summary=summary,
            command=plan.cmd,
            cargo_stdout=build.stdout,
            cargo_stderr=build.stderr,
            returncode=build.returncode,
            cargo_result=build,
        )
    if plan.fingerprint is not None:
        try:
            plan.fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
            _write_runtime_fingerprint(
                plan.fingerprint_path,
                plan.fingerprint,
                artifact=plan.runtime_lib,
            )
        except OSError:
            if not plan.json_output:
                print(
                    "Warning: failed to write runtime fingerprint metadata.",
                    file=sys.stderr,
                )
    return plan.accept()


def _build_native_runtime_under_lock(plan: _NativeRuntimeBuildPlan) -> bool:
    if not plan.json_output:
        message = (
            "Building optimized runtime (first time only)..."
            if not plan.runtime_lib.exists()
            else "Runtime sources changed; rebuilding runtime..."
        )
        print(message, file=sys.stderr)
    try:
        with _build_slot() as _slot:
            started = time.perf_counter()
            build = _run_cargo_with_sccache_retry(
                plan.cmd,
                cwd=plan.project_root,
                env=plan.build_env,
                timeout=plan.cargo_timeout,
                json_output=plan.json_output,
                label="Runtime build",
            )
            _record_runtime_build_stage_ms(
                plan.stage_timings_ms, "runtime_lib_cargo_build", started
            )
    except subprocess.TimeoutExpired:
        summary = (
            f"Runtime build timed out after {plan.cargo_timeout:.1f}s."
            if plan.cargo_timeout is not None
            else "Runtime build timed out."
        )
        if not plan.json_output:
            print(summary, file=sys.stderr)
        return _record_native_runtime_failure(
            plan.runtime_state,
            project_root=plan.project_root,
            stage="cargo",
            summary=summary,
            command=plan.cmd,
            timed_out=True,
        )
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
    )
    if plan is None:
        return False
    if os.environ.get("MOLT_SKIP_RUNTIME_REBUILD") == "1" and runtime_lib.exists():
        if plan.manifest_matches():
            return plan.accept()
        with _build_lock(project_root, plan.lock_name):
            return plan.accept() if plan.refresh_manifest() else False
    if (
        plan.session_key is not None
        and plan.session_key in _RUNTIME_LIB_VERIFIED
        and plan.manifest_matches()
    ):
        return plan.accept()
    with _build_lock(project_root, plan.lock_name):
        reuse = _reuse_native_runtime_under_lock(plan)
        if reuse is not None:
            return reuse
        hydrate = _hydrate_native_runtime_under_lock(plan)
        if hydrate is not None:
            return hydrate
        return _build_native_runtime_under_lock(plan)
