from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
import hashlib
from pathlib import Path
from typing import Literal
from uuid import uuid4

from molt.artifact_publication import (
    atomic_write_bytes,
    atomic_write_json,
    publish_validated_outputs,
    staged_copy_file,
)
from molt.backend_environment import DEBUG_ARTIFACT_DIR_ENV, backend_debug_artifact_dir
from molt.file_publication import resolve_owned_path
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    open_stable_regular_file,
    stable_regular_file_handle_identity,
)

from tools import harness_memory_guard
from tests import process_guard_common

_MAX_EXECUTABLE_BYTES = 256 * 1024 * 1024
_MAX_CAPTURE_BYTES = 8 * 1024 * 1024


@dataclass(frozen=True)
class NativeExecutableObservation:
    """Pre-guest observation, never inferred from a post-failure pathname."""

    cwd: Path | None
    identity: StableRegularFileIdentity | None
    unavailable_reason: str | None


def observe_native_test_executable(
    path: Path, env: Mapping[str, str]
) -> NativeExecutableObservation | None:
    if not env.get(DEBUG_ARTIFACT_DIR_ENV, "").strip():
        return None
    cwd = None
    try:
        cwd = Path.cwd()
        with open_stable_regular_file(path, label="native test executable") as opened:
            identity = stable_regular_file_handle_identity(
                opened, label="native test executable", max_bytes=_MAX_EXECUTABLE_BYTES
            )
        return NativeExecutableObservation(cwd, identity, None)
    except Exception as exc:
        # Diagnostics must not change whether the original guest is executed.
        return NativeExecutableObservation(cwd, None, f"{type(exc).__name__}: {exc}")


def _retain_capture(directory: Path, name: str, data: bytes | None) -> dict | None:
    if data is None:
        return None
    retained = data[:_MAX_CAPTURE_BYTES]
    atomic_write_bytes(directory / name, retained)
    digest = hashlib.sha256(data).hexdigest()
    return {
        "path": name,
        "bytes": len(data),
        "sha256": digest,
        "retained_bytes": len(retained),
        "retained_sha256": (
            digest
            if len(data) == len(retained)
            else hashlib.sha256(retained).hexdigest()
        ),
        "truncated": len(retained) != len(data),
    }


def _native_result_payload(
    directory: Path,
    phase: str,
    command: Sequence[str],
    cwd: Path | None,
    result: harness_memory_guard.GuardedCompletedProcess[bytes] | None,
) -> dict:
    payload = {"command": list(command), "cwd": None if cwd is None else str(cwd)}
    if result is None:
        return {**payload, "result": None}
    guard = harness_memory_guard.memory_guard
    return {
        **payload,
        "result": {
            "command": list(result.args),
            "returncode": result.returncode,
            "child_returncode": result.child_returncode,
            "timed_out": result.timed_out,
            "cancelled": result.cancelled,
            "descendants_closed": result.descendants_closed,
            "child_process": guard.guarded_child_process_payload(result.child_process),
            "infrastructure_failure": guard.infrastructure_failure_payload(
                result.infrastructure_failure
            ),
            "guard_signal": result.guard_signal,
            "memory_violation": harness_memory_guard._rss_record_payload(
                result.violation
            ),
            "termination_reports": guard.termination_reports_payload(
                result.termination_reports
            ),
            "elapsed_s": result.elapsed_s,
            "child_elapsed_s": result.child_elapsed_s,
            "cleanup_elapsed_s": result.cleanup_elapsed_s,
            "stdout": _retain_capture(directory, f"{phase}.stdout", result.stdout),
            "stderr": _retain_capture(directory, f"{phase}.stderr", result.stderr),
            "child_stderr": _retain_capture(
                directory, f"{phase}.child.stderr", result.child_stderr
            ),
        },
    }


def retain_native_test_failure(
    error: Exception,
    *,
    phase: Literal["build", "run"],
    source: bytes,
    source_path: Path,
    executable_path: Path,
    executable: NativeExecutableObservation | None,
    backend: str | None,
    profile: str,
    build_command: Sequence[str],
    build_cwd: Path,
    run_command: Sequence[str],
    build: harness_memory_guard.GuardedCompletedProcess[bytes] | None,
    run: harness_memory_guard.GuardedCompletedProcess[bytes] | None,
    env: Mapping[str, str],
) -> None:
    """Publish optional diagnostics; always preserve the original failure.

    A manifest published last marks a complete bundle. Partial publication has
    no manifest, and cannot become a proof receipt or a successful test result.
    """
    if not env.get(DEBUG_ARTIFACT_DIR_ENV, "").strip():
        return
    try:
        directory = resolve_owned_path(
            backend_debug_artifact_dir(env) / "native-execution-failures" / uuid4().hex
        )
        directory.mkdir(parents=True, exist_ok=False)
        terminal = getattr(error, "guarded_result", None)
        if terminal is not None:
            if phase == "build":
                build = terminal
            else:
                run = terminal
        binary: dict = {"original_path": str(executable_path), "retained": False}
        if executable is None:
            binary["unavailable_reason"] = "guest execution was not reached"
        elif executable.identity is None:
            binary["unavailable_reason"] = executable.unavailable_reason
        else:
            observed = executable.identity
            binary.update({"bytes": observed.size, "sha256": observed.sha256})
            try:
                destination = directory / "executable"
                with staged_copy_file(
                    executable_path, destination, observed=observed
                ) as stage:
                    publish_validated_outputs([(stage, destination)])
                binary.update({"retained": True, "path": destination.name})
            except Exception as exc:
                binary["unavailable_reason"] = f"{type(exc).__name__}: {exc}"
        # Reuse the proof authority's immutable Git observation, with an explicit
        # post-failure scope: this does not attest the worktree used by the build.
        try:
            from tools.proof_plan import _source_identity

            compiler_source = {"observed_after_failure": _source_identity()}
        except Exception as exc:
            compiler_source = {"unavailable_reason": f"{type(exc).__name__}: {exc}"}
        manifest = {
            "schema": "molt.native-test-failure.v1",
            "phase": phase,
            "error_type": type(error).__name__,
            "test": env.get("PYTEST_CURRENT_TEST"),
            "backend": backend,
            "profile": profile,
            "compiler_source": compiler_source,
            "source_path": str(source_path),
            "source_input": _retain_capture(directory, "source.py", source),
            "executable": binary,
            "build": _native_result_payload(
                directory, "build", build_command, build_cwd, build
            ),
            # None at the launch boundary means inherited cwd, not build_cwd.
            "run": _native_result_payload(directory, "run", run_command, None, run),
            "run_cwd_observed_before_guest": None
            if executable is None or executable.cwd is None
            else str(executable.cwd),
        }
        atomic_write_json(directory / "manifest.json", manifest)
        error.add_note(
            f"native test failure diagnostics: {directory / 'manifest.json'}"
        )
    except Exception as exc:
        error.add_note(
            f"native test failure diagnostics unavailable: {type(exc).__name__}: {exc}"
        )


def run_native_test_process(
    args: Sequence[str],
    *,
    cwd: str | Path | None = None,
    env: Mapping[str, str] | None = None,
    timeout: float | None = None,
    default_timeout: float | None = None,
    capture_output: bool = True,
    text: bool = True,
    check: bool = False,
    input: str | None = None,
) -> harness_memory_guard.GuardedCompletedProcess:
    return process_guard_common.run_guarded_test_process(
        args,
        prefix="MOLT_NATIVE_TEST",
        cwd=cwd,
        env=env,
        timeout=timeout,
        default_timeout=default_timeout,
        capture_output=capture_output,
        text=text,
        check=check,
        input=input,
    )
