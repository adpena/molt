import atexit
import concurrent.futures
import contextlib
import hashlib
import io
import json
import os
import shutil
import signal
import subprocess
import sys
import threading
import time
from collections.abc import Callable, Iterator, Mapping, MutableMapping, Sequence
from dataclasses import dataclass, replace
from functools import lru_cache
from pathlib import Path
from typing import Any, cast

# Make the repository root importable so the `tools` / `tests` packages resolve
# regardless of the current working directory or how this harness is launched.
# Running `python3 tests/molt_diff.py` puts `tests/` (the script directory) on
# sys.path, NOT the repo root, so the `from tools...` imports below would fail
# with ModuleNotFoundError unless PYTHONPATH happened to include the root. This
# self-bootstrap makes the documented invocation work from any directory.
_REPO_ROOT = Path(__file__).resolve().parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))
_SRC_ROOT = _REPO_ROOT / "src"
if str(_SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(_SRC_ROOT))

from tools.batch_compile_client import BatchCompileServerClient  # noqa: E402  (must follow the sys.path self-bootstrap above)
from molt._host_capabilities_generated import (  # noqa: E402
    EXPLICIT_CAPABILITY_TIER,
)
from tools import (  # noqa: E402  (must follow the sys.path self-bootstrap above)
    harness_memory_guard,
    memory_guard,
    process_sentinel,
    resource_pressure,
)
from molt.dx import (  # noqa: E402
    CANONICAL_RUN_ENV_KEYS,
    DX_ENV_KEYS,
    artifact_root as canonical_artifact_root,
    cargo_target_dir_for_environment,
    development_artifact_env,
    proof_scratch_root,
    scratch_root,
)
from molt import compiler_process, host_capacity, process_model  # noqa: E402
from molt.file_publication import resolve_owned_path  # noqa: E402
from molt.portable_paths import portable_relative_path  # noqa: E402
from molt import backend_daemon_custody as daemon_custody  # noqa: E402
from molt.cli.backend_daemon_config import _backend_daemon_enabled  # noqa: E402
from molt.cli.build_request import (  # noqa: E402
    BuildProfile,
    BuildRequest,
    ParseCodec,
    build_request_to_payload,
)
from molt import file_locks  # noqa: E402
from molt import python_interpreter  # noqa: E402
from molt.target_python import (  # noqa: E402
    TargetPythonVersion,
    resolve_target_python_for_oracle,
)
from tests import process_guard_common  # noqa: E402
from tools.compat import backends as compat_backends  # noqa: E402
from tools.compat import comparison as compat_comparison  # noqa: E402
from tools.compat import diff_output_layout  # noqa: E402
from tools.compat import test_policy  # noqa: E402
from tools.memory_guard_core import harness_outcomes  # noqa: E402
from molt.build_state_layout import build_state_root  # noqa: E402
from molt.memory_guard_paths import memory_guard_state_root  # noqa: E402

_FAILURE_STATUSES = frozenset({"fail", "oom", "uncalibrated"})
_PROCESS_STARTED = time.monotonic()
_DIFF_RUN_LOCK_HANDLE: file_locks._FileLockHandle | None = None
_WORKER_ORPHAN_GUARD_INSTALLED = False
_BATCH_COMPILE_SERVER_CLIENT: "_BatchCompileServerClient | None" = None
_BATCH_COMPILE_SERVER_CLIENT_PID = 0
_DIFF_MEMORY_GUARD_TRIP_FILE_ENV = harness_outcomes.SUITE_TRIP_FILE_ENV
_DIFF_MEMORY_GUARD_EVENTS_JSONL_ENV = "MOLT_DIFF_MEMORY_GUARD_EVENTS_JSONL"
_DIFF_MEMORY_GUARD_GLOBAL_SAMPLES_JSONL_ENV = (
    "MOLT_DIFF_MEMORY_GUARD_GLOBAL_SAMPLES_JSONL"
)
_DIFF_MEMORY_GUARD_DEFAULT_SAMPLE_INTERVAL_SEC = 1.0
_DIFF_MEMORY_GUARD_DEFAULT_EVENT_MAX_MB = 1.0
_DIFF_MEMORY_GUARD_DEFAULT_SAMPLE_MAX_MB = 2.0
_DIFF_MEMORY_GUARD_RETURN_CODE = memory_guard.GUARD_RETURN_CODE
_DIFF_MEMORY_GUARD_HARD_GLOBAL_GB = harness_memory_guard.HARD_RSS_LIMIT_GB
_DIFF_MEMORY_GUARD_HARD_GLOBAL_KB = memory_guard.max_rss_kb_from_gb(
    _DIFF_MEMORY_GUARD_HARD_GLOBAL_GB
)

PythonCommand = str | Sequence[str]


def _resolve_python_command(
    python_exe: PythonCommand, *, budget: compat_backends.ItemBudget | None = None
) -> tuple[str, ...]:
    if isinstance(python_exe, str):
        try:
            return python_interpreter.resolve_python_selector(
                python_exe,
                probe_timeout=None if budget is None else budget.probe_timeout,
            )
        finally:
            if budget is not None:
                budget.check()
    command = tuple(str(part) for part in python_exe if str(part))
    return command or (sys.executable,)


def _python_command_display(python_exe: PythonCommand) -> str:
    return " ".join(_resolve_python_command(python_exe))


def _resolve_diff_python_command(
    python_version: str | None, *, budget: compat_backends.ItemBudget | None = None
) -> tuple[str, ...]:
    if not python_version:
        return (sys.executable,)
    target_python = python_interpreter.parse_target_python_version(python_version)
    return tuple(
        python_interpreter.resolve_target_python_command(
            target_python,
            override=os.environ.get("MOLT_DIFF_PYTHON", "").strip() or None,
            cwd=_repo_root(),
            probe_timeout=None if budget is None else budget.probe_timeout,
        )
    )


def _metadata_probe_timeout_sec() -> float:
    raw = os.environ.get("MOLT_DIFF_METADATA_PROBE_TIMEOUT_SEC", "5").strip()
    try:
        value = float(raw)
    except ValueError:
        return 5.0
    return value if value > 0 else 5.0


# Successful probe facts retain their existing command identity. Deadlines are
# per-call inputs, never cache keys; an exhausted item cannot poison later facts.
_METADATA_PROBE_CACHE: dict[tuple[str, ...], subprocess.CompletedProcess[str]] = {}


def _run_metadata_probe(
    cmd: Sequence[str],
    *,
    budget: compat_backends.ItemBudget | None = None,
    build_failed: bool = False,
) -> subprocess.CompletedProcess[str] | None:
    if budget is not None:
        budget.check(build_failed=build_failed)
    key = tuple(cmd)
    if key in _METADATA_PROBE_CACHE:
        return _METADATA_PROBE_CACHE[key]
    timeout = _metadata_probe_timeout_sec()
    if budget is not None:
        timeout = budget.limit(timeout)
        if timeout == 0:
            raise compat_backends.ItemBudgetExpired(build_failed=build_failed)
    try:
        result = process_guard_common.run_guarded_test_process(
            cmd,
            prefix="MOLT_DIFF",
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except (OSError, subprocess.TimeoutExpired):
        if budget is not None:
            budget.check(build_failed=build_failed)
        return None
    if budget is not None:
        budget.check(build_failed=build_failed)
    # Probe facts are small, successful immutable command results. Bound retained
    # payloads as well as entries; never retain an arbitrary interpreter banner.
    if (
        result.returncode == 0
        and len(result.stdout or "") + len(result.stderr or "") <= 65536
    ):
        if len(_METADATA_PROBE_CACHE) >= 32:
            del _METADATA_PROBE_CACHE[next(iter(_METADATA_PROBE_CACHE))]
        _METADATA_PROBE_CACHE[key] = result
    return result


def _resolve_molt_cli_python() -> str:
    override = os.environ.get("MOLT_DIFF_MOLT_PYTHON", "").strip()
    if override:
        command = python_interpreter.explicit_python_command(override)
        if len(command) != 1:
            raise python_interpreter.PythonInterpreterError(
                "MOLT_DIFF_MOLT_PYTHON must name one executable path"
            )
        return command[0]

    return sys.executable


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def _configure_diff_artifact_environment(
    *,
    repo_root: Path,
    environment: MutableMapping[str, str],
) -> None:
    """Install canonical artifact custody at the CLI execution boundary."""

    inherited_outputs = {
        key: environment[key]
        for key in diff_output_layout.OUTPUT_KEYS
        if environment.get(key)
    }
    resolved = development_artifact_env(
        repo_root,
        environment,
        session_prefix="diff",
        session_id=environment.get("MOLT_SESSION_ID") or f"diff-{os.getpid()}",
        create_dirs=True,
    )
    for key in (*CANONICAL_RUN_ENV_KEYS, *DX_ENV_KEYS, "PYTHONPATH"):
        if key in resolved:
            environment[key] = resolved[key]
    diff_output_layout.admit(
        environment,
        repo_root=repo_root,
        custody_root=Path(environment["MOLT_DIFF_ROOT"]),
        explicit_outputs=inherited_outputs,
    )


def _configure_diff_evidence(
    args: Any,
    *,
    repo_root: Path,
    environment: MutableMapping[str, str],
) -> None:
    """Bind one evidence owner beneath the configured run's scratch root."""
    if args.evidence_dir is None:
        return
    conflicts = [
        name
        for name, explicit in (
            ("--json-output", args.json_output is not None),
            ("--log-aggregate", args.log_aggregate is not None),
            ("MOLT_DIFF_SUMMARY", "MOLT_DIFF_SUMMARY" in environment),
            ("MOLT_DIFF_RESULTS_JSONL", "MOLT_DIFF_RESULTS_JSONL" in environment),
        )
        if explicit
    ]
    if conflicts:
        raise ValueError(
            "--evidence-dir conflicts with explicit artifact paths: "
            + ", ".join(conflicts)
        )
    purpose = portable_relative_path(args.evidence_dir)
    directory = resolve_owned_path(
        proof_scratch_root(repo_root, environment).joinpath(
            "differential", *purpose.parts
        )
    )
    summary = resolve_owned_path(directory / "summary.json")
    results = resolve_owned_path(directory / "results.jsonl")
    aggregate = resolve_owned_path(directory / "output.log")
    directory.mkdir(parents=True, exist_ok=True)
    environment["MOLT_DIFF_SUMMARY"] = str(summary)
    environment["MOLT_DIFF_RESULTS_JSONL"] = str(results)
    args.log_aggregate = str(aggregate)


def _collect_env_overrides(file_path: str) -> dict[str, str]:
    return test_policy.collect_environment_overrides(file_path, repo_root=_REPO_ROOT)


def _diff_capabilities(env: dict[str, str]) -> str:
    if "MOLT_DIFF_CAPABILITIES" in env:
        return env["MOLT_DIFF_CAPABILITIES"]
    if "MOLT_CAPABILITIES" in env:
        return env["MOLT_CAPABILITIES"]
    return "fs,env,time,random"


def _metadata_stdlib_profile(file_path: str) -> tuple[str | None, str | None]:
    try:
        return test_policy.parse_metadata(file_path).stdlib_profile, None
    except ValueError as exc:
        return None, str(exc)


def _apply_metadata_env_overrides(
    file_path: str,
    env: dict[str, str],
    *,
    metadata: test_policy.TestMetadata | None = None,
) -> str | None:
    if metadata is None:
        profile, error = _metadata_stdlib_profile(file_path)
        if error is not None:
            return error
    else:
        profile = metadata.stdlib_profile
    if profile is None:
        return None
    explicit = env.get("MOLT_DIFF_STDLIB_PROFILE", "").strip().lower()
    if explicit and explicit != profile:
        return (
            f"{file_path} requires MOLT_DIFF_STDLIB_PROFILE={profile} "
            f"but selected {explicit}"
        )
    env["MOLT_DIFF_STDLIB_PROFILE"] = profile
    return None


@dataclass(frozen=True, slots=True)
class _PythonOracleMetadata:
    feature_version: tuple[int, int]
    executable: str


def _python_command_metadata(
    command: tuple[str, ...], *, budget: compat_backends.ItemBudget | None = None
) -> _PythonOracleMetadata | None:
    """Observe the oracle's target minor and executable in one cached probe."""

    if not command:
        return None
    code = (
        "import json,sys;"
        "print(json.dumps({"
        "'executable':sys.executable,"
        "'version_info':list(sys.version_info[:2])"
        "}))"
    )
    result = _run_metadata_probe([*command, "-c", code], budget=budget)
    if result is None:
        return None
    if result.returncode != 0:
        return None
    raw = (result.stdout or "").strip().splitlines()
    if not raw:
        return None
    try:
        payload = json.loads(raw[-1])
    except json.JSONDecodeError:
        return None
    if not isinstance(payload, dict):
        return None
    version_info = payload.get("version_info")
    executable = payload.get("executable")
    if (
        not isinstance(version_info, list)
        or len(version_info) != 2
        or not all(type(part) is int for part in version_info)
        or not isinstance(executable, str)
        or not executable
    ):
        return None
    return _PythonOracleMetadata((version_info[0], version_info[1]), executable)


def _python_exe_version(
    python_exe: PythonCommand, *, budget: compat_backends.ItemBudget | None = None
) -> tuple[int, int] | None:
    metadata = _python_command_metadata(
        _resolve_python_command(python_exe, budget=budget), budget=budget
    )
    return None if metadata is None else metadata.feature_version


def _resolve_molt_target_python(
    python_exe: PythonCommand,
    explicit: str | TargetPythonVersion | None,
    *,
    budget: compat_backends.ItemBudget | None = None,
) -> TargetPythonVersion:
    return resolve_target_python_for_oracle(
        _python_exe_version(python_exe, budget=budget), explicit
    )


# The CPython-parity comparison law lives in ONE place: tools/compat/comparison.py
# (doc 66 Phase 0). These names are thin re-exports so the in-file comparison
# sites keep working while routing through the single law. There is no second
# comparison implementation or backend-specific runner in the
# tree after this extraction; tests/test_compat_comparison.py proves the law
# behaves byte-identically to the inlined version it replaced.
_canonicalize_stdout = compat_comparison.canonicalize_stdout
_extract_exception_signature = compat_comparison.extract_exception_signature
_stderr_matches = compat_comparison.stderr_matches


@dataclass(frozen=True, slots=True)
class _AdmissionDeadline:
    """Reserve the same bounded item policy that execution actually consumes."""

    cutoff: float | None
    deadline: float | None
    policy: compat_backends.TimeoutPolicy
    targets: tuple[str, ...]

    @classmethod
    def for_run(
        cls, deadline_s: float | None, targets: tuple[str, ...]
    ) -> "_AdmissionDeadline":
        policy = compat_backends.TimeoutPolicy.from_environment(os.environ)
        worst = policy.reservation(targets)
        if deadline_s is not None and worst is None:
            raise ValueError(
                "--deadline-s requires MOLT_DIFF_TIMEOUT and finite per-target phase bounds"
            )
        deadline = None if deadline_s is None else _PROCESS_STARTED + deadline_s
        return cls(
            None if deadline is None else deadline - worst, deadline, policy, targets
        )

    def admits(self) -> bool:
        return self.cutoff is None or time.monotonic() < self.cutoff

    def new_budget(self, *, build_only: bool = False) -> compat_backends.ItemBudget:
        return compat_backends.ItemBudget.start(
            self.policy,
            ("native",) if build_only else self.targets,
            cpython=not build_only,
            build_only=build_only,
            suite_deadline=self.deadline,
        )


@dataclass(frozen=True, slots=True)
class DiffArtifactLayout:
    """Immutable artifact-root projection for one repository/environment pair."""

    repo_root: Path
    artifact_root: Path
    diff_root: Path
    tmp_root: Path
    cargo_target_root: Path
    cache_root: Path

    @classmethod
    def from_environment(
        cls,
        *,
        repo_root: Path,
        environment: Mapping[str, str],
    ) -> "DiffArtifactLayout":
        repository = Path(repo_root).expanduser()

        def configured_path(key: str) -> Path | None:
            raw = environment.get(key, "").strip()
            return Path(raw).expanduser() if raw else None

        artifact_root = canonical_artifact_root(repository, environment)
        diff_root = diff_output_layout.diff_custody_root(
            environment, repo_root=repository
        )
        selected = diff_output_layout.selected_root(
            environment, repo_root=repository, custody_root=diff_root
        )
        if selected is None:
            tmp_root = configured_path("MOLT_DIFF_TMPDIR") or scratch_root(
                repository, environment
            )
            cargo_target_root = (
                configured_path("MOLT_DIFF_CARGO_TARGET_DIR")
                or configured_path("CARGO_TARGET_DIR")
                or cargo_target_dir_for_environment(
                    artifact_root,
                    environment,
                    source_root=repository,
                )
            )
        else:
            tmp_root = selected / "guest-tmp"
            cargo_target_root = diff_output_layout.projected_target(
                environment, selected
            )
        return cls(
            repo_root=repository,
            artifact_root=artifact_root,
            diff_root=diff_root,
            tmp_root=tmp_root,
            cargo_target_root=cargo_target_root,
            cache_root=artifact_root / ".molt_cache",
        )


def _diff_artifact_layout(
    *,
    repo_root: Path | None = None,
    environment: Mapping[str, str] | None = None,
) -> DiffArtifactLayout:
    return DiffArtifactLayout.from_environment(
        repo_root=repo_root if repo_root is not None else _repo_root(),
        environment=environment if environment is not None else os.environ,
    )


def _materialize_artifact_root(path: Path) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    return path


def _diff_root() -> Path:
    return _materialize_artifact_root(_diff_artifact_layout().diff_root)


def _diff_results_jsonl_path() -> Path | None:
    """Optional per-test RAW-status sink for the suite-honesty ratchet (#46).

    When MOLT_DIFF_RESULTS_JSONL is set, every diff_test() call appends one JSON
    line recording the test's repo-relative path and its RAW status (before the
    xfail/xpass overlay), plus the resolved status and reason tag for audit. This
    is the single authoritative record of what Molt actually did vs CPython;
    tools/check_suite_honesty.py consumes it so a tracked-but-failing test, or a
    silently-fixed one, can never read as green. Off by default (returns None).
    Workers are separate processes, so each appends to the shared file — the same
    cross-process JSONL idiom used by the memory guard above.
    """
    raw = os.environ.get("MOLT_DIFF_RESULTS_JSONL", "").strip()
    if not raw:
        return None
    return Path(raw).expanduser()


def _record_diff_result(record: dict[str, object]) -> None:
    path = _diff_results_jsonl_path()
    if path is None:
        return
    # Per-backend rows (doc 66) ride along on the aggregate record under the
    # transient "backend_rows" key; they are emitted as their own JSONL lines so
    # the suite-honesty ratchet can dimension a failure by backend exactly as it
    # dimensions native today. Strip the key from the aggregate line so the
    # existing single-row honesty schema is unchanged.
    backend_rows = (
        record.pop("backend_rows", None) if isinstance(record, dict) else None
    )
    # A raw_status of None means an exception escaped before any status was
    # assigned; record it explicitly as "error" so the honesty ratchet treats an
    # unexpected crash as a hard, visible outcome rather than a missing line.
    if record.get("raw_status") is None:
        record = {**record, "raw_status": "error", "resolved_status": "error"}
    lines: list[str] = [json.dumps(record, sort_keys=True)]
    if isinstance(backend_rows, list):
        for row in backend_rows:
            lines.append(json.dumps(row, sort_keys=True))
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a", encoding="utf-8") as handle:
            handle.write("\n".join(lines) + "\n")
    except OSError as exc:
        raise RuntimeError(
            f"differential result receipt write failed at {path}: {exc}"
        ) from exc


def _diff_tmp_root(environment: Mapping[str, str] | None = None) -> Path:
    return _materialize_artifact_root(
        _diff_artifact_layout(environment=environment).tmp_root
    )


def _diff_cargo_target_root() -> Path:
    return _materialize_artifact_root(_diff_artifact_layout().cargo_target_root)


def _diff_cache_root() -> Path:
    return _materialize_artifact_root(_diff_artifact_layout().cache_root)


def _diff_backend_daemon_root() -> Path:
    return _diff_state_root() / "backend_daemon"


def _diff_build_lock_root() -> Path:
    return _diff_state_root() / "build_locks"


def _diff_state_root() -> Path:
    layout = _diff_artifact_layout()
    return build_state_root(
        project_root=layout.repo_root,
        cargo_target=layout.cargo_target_root,
        environment=os.environ,
    )


def _diff_run_lock_path() -> Path:
    return _diff_state_root() / "diff_run.lock"


def _diff_run_lock_wait_sec() -> float:
    raw = _parse_float_env("MOLT_DIFF_RUN_LOCK_WAIT_SEC")
    if raw is None:
        return 15 * 60.0
    return max(0.0, raw)


def _diff_run_lock_poll_sec() -> float:
    raw = _parse_float_env("MOLT_DIFF_RUN_LOCK_POLL_SEC")
    if raw is None:
        return 0.5
    return max(0.05, raw)


def _release_diff_run_lock() -> None:
    global _DIFF_RUN_LOCK_HANDLE
    handle = _DIFF_RUN_LOCK_HANDLE
    _DIFF_RUN_LOCK_HANDLE = None
    if handle is None:
        return
    file_locks._release_file_lock(handle)


def _ensure_diff_run_lock() -> None:
    global _DIFF_RUN_LOCK_HANDLE
    lock_path = _diff_run_lock_path()
    if _DIFF_RUN_LOCK_HANDLE is not None:
        if _DIFF_RUN_LOCK_HANDLE.registry_key != file_locks._in_process_lock_key(
            lock_path
        ):
            raise RuntimeError("differential run target changed while its lock is held")
        return
    wait_sec = _diff_run_lock_wait_sec()
    poll_sec = _diff_run_lock_poll_sec()
    handle = file_locks._acquire_file_lock(
        lock_path,
        timeout_s=wait_sec,
        timeout_message=f"Timed out waiting for differential run lock: {lock_path}",
        poll_s=poll_sec,
    )
    try:
        handle.file.seek(0)
        handle.file.truncate(0)
        handle.file.write(
            f"pid={os.getpid()} started={int(time.time())}\n".encode("utf-8")
        )
        handle.file.flush()
    except BaseException:
        file_locks._release_file_lock(handle)
        raise
    _DIFF_RUN_LOCK_HANDLE = handle
    atexit.register(_release_diff_run_lock)


def _parse_int_env(name: str, default: int) -> int:
    raw = os.environ.get(name, "").strip()
    if not raw:
        return default
    try:
        value = int(raw)
    except ValueError:
        return default
    return value


_DAEMON_CONTROL_TIMEOUT_S = 0.75


def _daemon_health(socket_path: Path) -> dict[str, Any] | None:
    """The daemon's health report through the CLI's daemon client."""
    if os.name != "posix" or not socket_path.exists():
        return None
    from molt.cli.backend_execution import backend_daemon_health

    return backend_daemon_health(socket_path, timeout=_DAEMON_CONTROL_TIMEOUT_S)


@dataclass(frozen=True)
class _PrunableProcess:
    pid: int
    identity: memory_guard.ProcessIdentity
    command: str
    reason: str


def _list_backend_daemon_processes() -> list[daemon_custody.BackendDaemonObservation]:
    return sorted(
        daemon_custody.backend_daemon_process_observations(),
        key=lambda process: process.pid,
    )


def _current_session_backend_daemon_records_by_pid() -> dict[
    int, daemon_custody.BackendDaemonIdentityRecord
]:
    session_id = os.environ.get("MOLT_SESSION_ID", "").strip()
    if not session_id:
        return {}
    return {
        record.identity.pid: record
        for record in daemon_custody.iter_backend_daemon_identity_records(
            _diff_backend_daemon_root(),
            session_id=session_id,
        )
    }


def _verified_backend_daemon_record(
    process: daemon_custody.BackendDaemonObservation,
    records_by_pid: Mapping[int, daemon_custody.BackendDaemonIdentityRecord],
) -> daemon_custody.BackendDaemonIdentityRecord | None:
    record = records_by_pid.get(process.sample.pid)
    if record is None:
        return None
    identity = record.identity
    if identity.socket_path != process.socket_path:
        return None
    if (
        identity.started_at_ns is None
        or identity.started_at_ns != process.sample.started_at_ns
    ):
        return None
    if not daemon_custody.backend_daemon_command_matches_identity(
        process.sample.argv
        if process.sample.argv is not None
        else process.sample.command,
        backend_bin=identity.backend_bin,
        socket_path=identity.socket_path,
    ):
        return None
    return record


def _retire_idle_backend_daemon(
    process: daemon_custody.BackendDaemonObservation,
    records_by_pid: Mapping[int, daemon_custody.BackendDaemonIdentityRecord],
) -> bool:
    """Retire a daemon that reports no admitted jobs. A busy daemon keeps
    running: its builds would otherwise fail mid-compile. Retirement drains,
    so a build that connects after the health check still completes."""
    record = _verified_backend_daemon_record(process, records_by_pid)
    if record is None:
        return False
    health = _daemon_health(process.socket_path)
    # The socket may already belong to a successor; retire only this daemon.
    if (
        health is None
        or health.get("pid") != process.sample.pid
        or health.get("active_jobs") != 0
    ):
        return False
    from molt.cli.backend_execution import retire_backend_daemon

    return retire_backend_daemon(process.socket_path, timeout=_DAEMON_CONTROL_TIMEOUT_S)


def _prune_custody_event_path() -> Path:
    return memory_guard_state_root(_repo_root()) / "incidents" / "molt-diff-prune.jsonl"


def _record_prune_custody_event(
    process: _PrunableProcess,
    actions: Sequence[memory_guard.GuardTerminationAction],
) -> None:
    path = _prune_custody_event_path()
    payload = {
        "event": "molt_diff_prune_custody",
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "pid": process.pid,
        "reason": process.reason,
        "command": process.command,
        "actions": [
            {
                "target_kind": action.target_kind,
                "target_id": action.target_id,
                "signal": action.signal,
                "signal_name": action.signal_name,
                "result": action.result,
                "error": action.error,
            }
            for action in actions
        ],
    }
    with contextlib.suppress(OSError):
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(payload, sort_keys=True) + "\n")


def _actions_completed_cleanup(
    actions: Sequence[memory_guard.GuardTerminationAction],
) -> bool:
    return any(
        action.result in {"completed_or_missing", "sent", "missing"}
        for action in actions
    )


def _prune_verified_processes(
    processes: Sequence[_PrunableProcess],
    *,
    label: str,
    grace: float,
) -> None:
    if not processes:
        return
    pruned = 0
    skipped = 0
    for process in processes:
        actions = memory_guard.terminate_verified_pid(
            process.pid,
            process.identity,
            sampler=memory_guard.sample_processes,
            grace=grace,
        )
        if _actions_completed_cleanup(actions):
            pruned += 1
            continue
        skipped += 1
        _record_prune_custody_event(process, actions)
    if skipped:
        print(
            f"[INFO] Pruned {pruned} {label}(s); skipped {skipped} by custody "
            f"gate (events: {_prune_custody_event_path()})"
        )
    elif pruned:
        print(f"[INFO] Pruned {pruned} {label}(s)")


def _list_orphan_diff_workers() -> list[_PrunableProcess]:
    if os.name != "posix":
        return []
    repo_python = Path(__file__).resolve().parents[1] / ".venv" / "bin" / "python3"
    markers = (
        "from multiprocessing.spawn import spawn_main",
        "from multiprocessing.resource_tracker import main",
    )
    samples = memory_guard.sample_processes()
    candidates: list[_PrunableProcess] = []
    for sample in samples.values():
        if sample.ppid != 1:
            continue
        if str(repo_python) not in sample.command:
            continue
        if any(marker in sample.command for marker in markers):
            candidates.append(
                _PrunableProcess(
                    pid=sample.pid,
                    identity=memory_guard.process_identity(sample),
                    command=sample.command,
                    reason="orphan_diff_worker",
                )
            )
    return sorted(candidates, key=lambda process: process.pid)


def _prune_orphan_diff_workers() -> None:
    _prune_verified_processes(
        _list_orphan_diff_workers(),
        label="orphan multiprocessing worker",
        grace=0.75,
    )


def _is_diff_build_helper_command(cmd: str) -> bool:
    if "internal-batch-build-server" in cmd and (
        "-m molt.cli" in cmd or "src/molt/cli/__init__.py" in cmd
    ):
        return True
    # Restrict to helper processes tied to diff temp dirs so we don't interfere
    # with unrelated local build activity.
    if "/molt_diff_" not in cmd:
        return False
    if "molt-backend" in cmd and "--output" in cmd and "--daemon" not in cmd:
        return True
    if ("-m molt.cli build " in cmd) or ("src/molt/cli/__init__.py build " in cmd):
        return True
    if cmd.rstrip().endswith("_molt"):
        return True
    return False


def _list_orphan_build_helpers() -> list[_PrunableProcess]:
    if os.name != "posix":
        return []
    samples = memory_guard.sample_processes()
    if not samples:
        return []

    cmd_by_pid = {pid: sample.command for pid, sample in samples.items()}
    ppid_by_pid = {pid: sample.ppid for pid, sample in samples.items()}
    stale_sec = max(60, _parse_int_env("MOLT_DIFF_HELPER_STALE_SEC", 20 * 60))

    def _has_diff_ancestor(pid: int) -> bool:
        seen: set[int] = set()
        current = ppid_by_pid.get(pid, 0)
        while current > 1 and current not in seen:
            seen.add(current)
            cmd = cmd_by_pid.get(current, "")
            if "tests/molt_diff.py" in cmd or "molt_diff.py " in cmd:
                return True
            current = ppid_by_pid.get(current, 0)
        return False

    candidates: list[_PrunableProcess] = []
    for sample in samples.values():
        if not _is_diff_build_helper_command(sample.command):
            continue
        if sample.ppid == 1:
            candidates.append(
                _PrunableProcess(
                    pid=sample.pid,
                    identity=memory_guard.process_identity(sample),
                    command=sample.command,
                    reason="orphan_diff_build_helper",
                )
            )
            continue
        # A helper that has outlived any diff harness ancestry is stale and can
        # deadlock later runs by holding shared build locks.
        if (
            sample.elapsed_sec is not None
            and sample.elapsed_sec >= stale_sec
            and not _has_diff_ancestor(sample.pid)
        ):
            candidates.append(
                _PrunableProcess(
                    pid=sample.pid,
                    identity=memory_guard.process_identity(sample),
                    command=sample.command,
                    reason="stale_diff_build_helper",
                )
            )
    return sorted(candidates, key=lambda process: process.pid)


def _prune_orphan_build_helpers() -> None:
    _prune_verified_processes(
        _list_orphan_build_helpers(),
        label="orphan build helper process",
        grace=0.35,
    )


def _prune_backend_daemons() -> None:
    if os.name != "posix":
        return
    max_rss_kb = _parse_int_env("MOLT_DIFF_DAEMON_MAX_RSS_KB", 2_500_000)
    if max_rss_kb <= 0:
        return
    records_by_pid = None
    for process in _list_backend_daemon_processes():
        rss_kb = process.sample.rss_kb
        if rss_kb is None or rss_kb <= max_rss_kb:
            continue
        if not process_model.pid_alive(process.sample.pid):
            continue
        if records_by_pid is None:
            records_by_pid = _current_session_backend_daemon_records_by_pid()
        # Missing health cannot prove idleness or socket ownership. The
        # daemon owns autonomous retirement; this optional RSS policy asks
        # only a verified, idle responder to drain its accepted connections.
        if _retire_idle_backend_daemon(process, records_by_pid):
            print(
                "[INFO] Retired idle backend daemon pid="
                f"{process.sample.pid} rss={rss_kb}KB (> {max_rss_kb}KB)"
            )


def _prune_stale_build_locks() -> None:
    lock_root = _diff_build_lock_root()
    if not lock_root.exists():
        return
    now = time.time()
    max_age = _parse_int_env("MOLT_DIFF_BUILD_LOCK_MAX_AGE_SEC", 12 * 60 * 60)
    max_keep = _parse_int_env("MOLT_DIFF_BUILD_LOCK_MAX_FILES", 4096)
    removed = 0
    lock_entries: list[tuple[float, Path]] = []
    try:
        for lock_path in lock_root.glob("*.lock"):
            try:
                stat = lock_path.stat()
            except OSError:
                continue
            # Build-lock files are coordination sentinels and should remain empty.
            if stat.st_size > 0:
                continue
            lock_entries.append((stat.st_mtime, lock_path))
            if max_age > 0 and (now - stat.st_mtime) > max_age:
                with contextlib.suppress(OSError):
                    lock_path.unlink()
                    removed += 1
    except OSError:
        return

    if max_keep > 0 and len(lock_entries) - removed > max_keep:
        # Keep the newest N lock sentinels to avoid directory growth over long runs.
        stale = sorted(lock_entries, key=lambda item: item[0])[
            : max(0, len(lock_entries) - max_keep)
        ]
        for _mtime, lock_path in stale:
            if not lock_path.exists():
                continue
            with contextlib.suppress(OSError):
                lock_path.unlink()
                removed += 1
    if removed > 0:
        print(f"[INFO] Pruned {removed} stale build lock file(s) from {lock_root}")


def _diff_keep_artifacts() -> bool:
    return diff_output_layout.keep_artifacts(os.environ)


def _diff_log_passes() -> bool:
    raw = os.environ.get("MOLT_DIFF_LOG_PASSES", "").strip().lower()
    return raw in {"1", "true", "yes", "on"}


def _backend_execution_context(
    file_path: str,
    python_exe: PythonCommand,
    build_profile: str,
    target_python: str | TargetPythonVersion | None,
    *,
    metadata: test_policy.TestMetadata | None = None,
    budget: compat_backends.ItemBudget,
) -> compat_backends.BackendExecutionContext:
    resolved_target = _resolve_molt_target_python(
        python_exe, target_python, budget=budget
    )
    env = os.environ.copy()
    # Diff runs start at the explicit tier; a test that needs capabilities
    # declares them in its metadata, so local runs match CI.
    env["MOLT_CAPABILITY_TIER"] = EXPLICIT_CAPABILITY_TIER
    # The same import path as the CPython oracle: `src`, unless the test's
    # MOLT_ENV names its own.
    env["PYTHONPATH"] = "src"
    env.update(_collect_env_overrides(file_path))
    metadata_error = _apply_metadata_env_overrides(
        file_path,
        env,
        metadata=metadata,
    )
    if metadata_error is not None:
        raise ValueError(metadata_error)
    oracle = _python_command_metadata(
        _resolve_python_command(python_exe, budget=budget), budget=budget
    )
    if oracle is not None:
        env["MOLT_SYS_EXECUTABLE"] = oracle.executable
    diff_output_layout.enforce_child(env, repo_root=_repo_root())
    return compat_backends.BackendExecutionContext(
        target_python=resolved_target,
        build_profile=build_profile,
        capabilities=_diff_capabilities(env),
        environment=env,
        budget=budget,
    )


def _diff_measure_rss() -> bool:
    raw = os.environ.get("MOLT_DIFF_MEASURE_RSS", "").strip().lower()
    threshold_requested = (_diff_fail_rss_kb() or 0) > 0
    if not raw:
        return threshold_requested
    enabled = raw not in {"0", "false", "no", "off"}
    if not enabled and threshold_requested:
        raise ValueError(
            "MOLT_DIFF_MAX_RSS_KB requires RSS profiling; unset "
            "MOLT_DIFF_MEASURE_RSS or enable it with 1"
        )
    return enabled


def _diff_glob() -> str:
    raw = os.environ.get("MOLT_DIFF_GLOB", "").strip()
    return raw or "*.py"


def _diff_run_id() -> str:
    raw = os.environ.get("MOLT_DIFF_RUN_ID", "").strip()
    if raw:
        return raw
    ts = time.strftime("%Y%m%d_%H%M%S", time.gmtime())
    return f"{ts}_{os.getpid()}"


def _diff_warm_cache() -> bool:
    raw = os.environ.get("MOLT_DIFF_WARM_CACHE", "").strip().lower()
    return raw in {"1", "true", "yes", "on"}


def _diff_dyld_preflight_default() -> bool:
    explicit = _bool_env("MOLT_DIFF_DYLD_PREFLIGHT")
    if explicit is not None:
        return explicit
    return sys.platform == "darwin"


def _bool_env(name: str) -> bool | None:
    raw = os.environ.get(name, "").strip().lower()
    if not raw:
        return None
    if raw in {"1", "true", "yes", "on"}:
        return True
    if raw in {"0", "false", "no", "off"}:
        return False
    return None


def _diff_backend_daemon_default() -> bool:
    explicit = _bool_env("MOLT_DIFF_BACKEND_DAEMON")
    if explicit is not None:
        return explicit
    return _backend_daemon_enabled()


def _diff_batch_compile_server_enabled() -> bool:
    explicit = _bool_env("MOLT_DIFF_BATCH_COMPILE_SERVER")
    if explicit is not None:
        return explicit
    return False


def _diff_batch_compile_server_request_timeout(
    build_timeout: float | None = None,
) -> float:
    raw = os.environ.get("MOLT_DIFF_BATCH_COMPILE_SERVER_TIMEOUT_SEC", "").strip()
    if raw:
        try:
            parsed = float(raw)
            if parsed > 0:
                return parsed
        except ValueError:
            pass
    if build_timeout is not None and build_timeout > 0:
        return build_timeout
    return 60.0


def _diff_force_no_cache() -> bool:
    explicit = _bool_env("MOLT_DIFF_FORCE_NO_CACHE")
    if explicit is not None:
        return explicit
    # Throughput-first default: keep cache enabled unless explicitly overridden.
    return False


def _diff_force_rebuild() -> bool:
    explicit = _bool_env("MOLT_DIFF_FORCE_REBUILD")
    if explicit is not None:
        return explicit
    return False


def _dyld_preflight_error(
    binary_path: Path, *, budget: compat_backends.ItemBudget
) -> str | None:
    if not _diff_dyld_preflight_default() or sys.platform != "darwin":
        return None
    otool = shutil.which("otool")
    if not otool:
        return None
    timeout = budget.limit(20.0)
    if timeout == 0:
        return None
    try:
        probe = subprocess.run(
            [otool, "-l", str(binary_path)],
            capture_output=True,
            text=True,
            check=False,
            timeout=timeout,
            encoding="utf-8",
        )
    except OSError:
        # Preflight is best-effort; host tool failures are not binary corruption.
        return None
    except subprocess.TimeoutExpired:
        return None
    merged = "\n".join([probe.stdout or "", probe.stderr or ""]).lower()
    if probe.returncode != 0:
        # Only gate when otool output indicates the specific dyld corruption we care
        # about; generic preflight failures should not poison the run.
        if "unknown imports format" in merged:
            return "dyld: unknown imports format (preflight)"
        if "malformed load command" in merged:
            return "dyld: unknown imports format (preflight malformed load command)"
        return None
    if "unknown imports format" in merged:
        return "dyld: unknown imports format (preflight)"
    if "malformed load command" in merged:
        return "dyld: unknown imports format (preflight malformed load command)"
    return None


def _diff_allow_rustc_wrapper() -> bool:
    raw = os.environ.get("MOLT_DIFF_ALLOW_RUSTC_WRAPPER", "").strip().lower()
    return raw in {"1", "true", "yes", "on"}


def _diff_build_profile() -> str:
    raw = os.environ.get("MOLT_DIFF_BUILD_PROFILE", "").strip().lower()
    if not raw:
        return "dev"
    if raw not in {"dev", "release"}:
        raise ValueError("MOLT_DIFF_BUILD_PROFILE must be 'dev' or 'release'")
    return raw


def _diff_prune_every() -> int:
    return max(0, _parse_int_env("MOLT_DIFF_PRUNE_EVERY", 32))


def _diff_max_tasks_per_child() -> int | None:
    raw = _parse_int_env("MOLT_DIFF_MAX_TASKS_PER_CHILD", 0)
    return raw if raw > 0 else None


def _parse_float_env(name: str) -> float | None:
    raw = os.environ.get(name, "").strip()
    if not raw:
        return None
    try:
        return float(raw)
    except ValueError:
        return None


@dataclass(frozen=True, slots=True)
class _DiffMemoryGuardConfig:
    max_process_kb: int
    max_tree_kb: int
    global_kb: int
    poll_interval: float
    child_rlimit_kb: int | None = None
    dynamic_process_rss: bool = False
    dynamic_tree_rss: bool = False
    dynamic_global_rss: bool = False
    dynamic_child_rlimit: bool = False

    @property
    def max_process_gb(self) -> float:
        return self.max_process_kb / (1024 * 1024)

    @property
    def max_tree_gb(self) -> float:
        return self.max_tree_kb / (1024 * 1024)

    @property
    def global_gb(self) -> float:
        return self.global_kb / (1024 * 1024)

    @property
    def child_rlimit_gb(self) -> float | None:
        if self.child_rlimit_kb is None:
            return None
        return self.child_rlimit_kb / (1024 * 1024)


def _bounded_positive_float_env(
    name: str, *, default: float, upper: float | None = None
) -> float:
    value = _parse_float_env(name)
    if value is None or value <= 0:
        value = default
    if upper is not None:
        value = min(value, upper)
    return max(0.001, value)


def _diff_memory_guard_limits(
    env: Mapping[str, str] | None = None,
) -> harness_memory_guard.HarnessMemoryLimits:
    limits = harness_memory_guard.limits_from_env("MOLT_DIFF", env)
    if limits.enabled:
        return limits
    return harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=limits.max_process_rss_gb,
        max_total_rss_gb=limits.max_total_rss_gb,
        max_global_rss_gb=limits.max_global_rss_gb,
        poll_interval=limits.poll_interval,
        child_rlimit_gb=limits.child_rlimit_gb,
        adaptive_prefix=limits.adaptive_prefix,
        dynamic_process_rss=limits.dynamic_process_rss,
        dynamic_total_rss=limits.dynamic_total_rss,
        dynamic_global_rss=limits.dynamic_global_rss,
        dynamic_child_rlimit=limits.dynamic_child_rlimit,
    )


def _diff_memory_guard_config(*, accounted_rss_kb: int = 0) -> _DiffMemoryGuardConfig:
    limits = _diff_memory_guard_limits()
    accounted_rss_kb = max(0, accounted_rss_kb)
    current_limits = limits.current_memory_limits(accounted_rss_kb=accounted_rss_kb)
    global_limit_kb = (
        limits.max_global_rss_kb
        if current_limits.max_global_rss_kb is None
        else current_limits.max_global_rss_kb
    )
    tree_limit_kb = (
        limits.max_total_rss_kb
        if current_limits.max_total_rss_kb is None
        else current_limits.max_total_rss_kb
    )
    global_kb = min(global_limit_kb, _DIFF_MEMORY_GUARD_HARD_GLOBAL_KB)
    tree_kb = min(tree_limit_kb, global_kb)
    process_kb = min(current_limits.max_process_rss_kb, tree_kb)
    return _DiffMemoryGuardConfig(
        max_process_kb=process_kb,
        max_tree_kb=tree_kb,
        global_kb=global_kb,
        child_rlimit_kb=limits.current_child_rlimit_kb(
            accounted_rss_kb=accounted_rss_kb
        ),
        poll_interval=min(2.0, max(0.01, limits.poll_interval)),
        dynamic_process_rss=limits.dynamic_process_rss,
        dynamic_tree_rss=limits.dynamic_total_rss,
        dynamic_global_rss=limits.dynamic_global_rss,
        dynamic_child_rlimit=limits.dynamic_child_rlimit,
    )


def _diff_memory_guard_enabled() -> bool:
    return True


def _diff_memory_guard_sample_interval_sec() -> float:
    return _bounded_positive_float_env(
        "MOLT_DIFF_MEMORY_GUARD_SAMPLE_INTERVAL_SEC",
        default=_DIFF_MEMORY_GUARD_DEFAULT_SAMPLE_INTERVAL_SEC,
        upper=60.0,
    )


def _diff_memory_guard_write_samples() -> bool:
    explicit = _bool_env("MOLT_DIFF_MEMORY_GUARD_WRITE_SAMPLES")
    return True if explicit is None else explicit


def _diff_memory_guard_stream_mode() -> str:
    raw = os.environ.get("MOLT_DIFF_MEMORY_GUARD_STREAM", "").strip().lower()
    if raw in {"", "0", "false", "no", "off", "none"}:
        return ""
    if raw in {"1", "true", "yes", "on", "stderr"}:
        return "stderr"
    if raw in {"stdout", "json", "json-stdout", "stderr-json", "json-stderr"}:
        return raw
    return ""


def _diff_memory_guard_stream_target(mode: str) -> io.TextIOBase | None:
    if not mode:
        return None
    return sys.stdout if "stdout" in mode else sys.stderr


def _memory_limit_bytes() -> int | None:
    # The adaptive child RSS budget (MOLT_DIFF_CHILD_RLIMIT_GB overrides it).
    # Recursive process/tree/global sampling remains authoritative; this is
    # only the matching kernel backstop.
    child_rlimit_kb = _diff_memory_guard_config().child_rlimit_kb
    return None if child_rlimit_kb is None else child_rlimit_kb * 1024


_MEM_LIMIT_APPLIED = False


def _diff_fail_rss_kb() -> int | None:
    raw = os.environ.get("MOLT_DIFF_MAX_RSS_KB", "").strip()
    if not raw:
        return None
    try:
        value = int(raw)
    except ValueError:
        return None
    return value if value > 0 else None


def _rss_exceeded(
    metrics: dict[str, int] | None, threshold_kb: int | None
) -> tuple[bool, str | None]:
    if threshold_kb is None or not metrics:
        return False, None
    candidates = [
        ("max_rss", metrics.get("max_rss")),
        ("peak_footprint", metrics.get("peak_footprint")),
    ]
    for name, value in candidates:
        if isinstance(value, int) and value > threshold_kb:
            return True, f"{name}={value}KB exceeds {threshold_kb}KB"
    return False, None


def _apply_memory_limit() -> None:
    global _MEM_LIMIT_APPLIED
    if _MEM_LIMIT_APPLIED:
        return
    limit = _memory_limit_bytes()
    if limit is None:
        _MEM_LIMIT_APPLIED = True
        return
    memory_guard._apply_child_resource_limit(max(1, limit // 1024))
    _MEM_LIMIT_APPLIED = True


def _default_jobs() -> int:
    count = host_capacity.cpu_count()
    guard = _diff_memory_guard_config()
    count = min(count, _memory_guard_max_jobs(guard))
    max_jobs = os.environ.get("MOLT_DIFF_MAX_JOBS", "").strip()
    if max_jobs.isdigit():
        count = min(count, max(1, int(max_jobs)))
    return max(1, count)


def _memory_guard_scheduler_per_job_gb(config: _DiffMemoryGuardConfig) -> float:
    return _diff_resource_pressure_plan(config).diff_scheduler_per_job_gb


def _diff_resource_pressure_plan(
    config: _DiffMemoryGuardConfig,
) -> resource_pressure.ResourcePressurePlan:
    budget = memory_guard.AdaptiveMemoryBudget(
        max_process_rss_gb=config.max_process_gb,
        max_total_rss_gb=config.max_tree_gb,
        max_global_rss_gb=config.global_gb,
        reserve_gb=0.0,
        physical_gb=None,
        available_gb=config.global_gb,
        source="guard_limits",
    )
    return resource_pressure.plan_resource_pressure(
        prefix="MOLT_DIFF",
        environ=os.environ,
        cpu_count=host_capacity.cpu_count(),
        budget=budget,
        diff_tree_gb=config.max_tree_gb,
        diff_global_gb=config.global_gb,
        diff_mem_per_job_gb=_parse_float_env("MOLT_DIFF_MEM_PER_JOB_GB"),
    )


def _memory_guard_max_jobs(config: _DiffMemoryGuardConfig) -> int:
    return _diff_resource_pressure_plan(config).diff_max_jobs


def _config_payload(config: _DiffMemoryGuardConfig) -> dict[str, object]:
    pressure_plan = _diff_resource_pressure_plan(config)
    return {
        "max_process_gb": config.max_process_gb,
        "max_tree_gb": config.max_tree_gb,
        "global_gb": config.global_gb,
        "child_rlimit_gb": config.child_rlimit_gb,
        "poll_interval": config.poll_interval,
        "scheduler_per_job_gb": pressure_plan.diff_scheduler_per_job_gb,
        "resource_pressure": pressure_plan.to_json_dict(),
        "dynamic_process_rss": config.dynamic_process_rss,
        "dynamic_tree_rss": config.dynamic_tree_rss,
        "dynamic_global_rss": config.dynamic_global_rss,
        "dynamic_child_rlimit": config.dynamic_child_rlimit,
    }


def _refresh_memory_guard_config(
    config: _DiffMemoryGuardConfig, *, accounted_rss_kb: int
) -> _DiffMemoryGuardConfig:
    if not (
        config.dynamic_process_rss
        or config.dynamic_tree_rss
        or config.dynamic_global_rss
        or config.dynamic_child_rlimit
    ):
        return config
    return _diff_memory_guard_config(accounted_rss_kb=accounted_rss_kb)


def _constrain_jobs_for_memory_guard(
    jobs: int, *, config: _DiffMemoryGuardConfig, log: bool = True
) -> int:
    safe_jobs = _memory_guard_max_jobs(config)
    if jobs <= safe_jobs:
        return max(1, jobs)
    if log:
        print(
            "[MEMORY-GUARD] "
            f"Clamping molt_diff jobs from {jobs} to {safe_jobs} "
            f"(global={config.global_gb:.2f}GB "
            f"per_job={_memory_guard_scheduler_per_job_gb(config):.2f}GB)."
        )
    return safe_jobs


def _order_test_files(files: list[Path], jobs: int) -> list[Path]:
    mode = os.environ.get("MOLT_DIFF_ORDER", "auto").strip().lower()
    if mode not in {"auto", "name", "size-asc", "size-desc"}:
        mode = "auto"
    if mode == "auto":
        mode = "size-desc" if jobs > 1 else "name"
    if mode == "name":
        return files

    def size_key(path: Path) -> int:
        try:
            return path.stat().st_size
        except OSError:
            return 0

    reverse = mode == "size-desc"
    return sorted(files, key=size_key, reverse=reverse)


def _log_path_for_test(log_dir: Path, file_path: str) -> Path:
    identity = test_policy.normalize_repo_relative(file_path, repo_root=_repo_root())
    digest = hashlib.sha256(identity.encode("utf-8", "surrogatepass")).hexdigest()
    return log_dir / f"{digest}.log"


def _write_test_log(log_dir: Path, file_path: str, stdout: str, stderr: str) -> Path:
    log_path = _log_path_for_test(log_dir, file_path)
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open("w") as handle:
        if stdout:
            handle.write("STDOUT:\n")
            handle.write(stdout)
            if not stdout.endswith("\n"):
                handle.write("\n")
        if stderr:
            if stdout:
                handle.write("\n")
            handle.write("STDERR:\n")
            handle.write(stderr)
            if not stderr.endswith("\n"):
                handle.write("\n")
    return log_path


def _emit_line(
    line: str,
    log_handle: io.TextIOBase | None = None,
    *,
    echo: bool = True,
) -> None:
    if echo:
        print(line, flush=True)
    if log_handle is not None:
        log_handle.write(line + "\n")
        log_handle.flush()


@contextlib.contextmanager
def _open_log_file(path: Path | None):
    if path is None:
        yield None
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    handle = path.open("a", buffering=1)
    try:
        yield handle
    finally:
        handle.close()


def _diff_worker(
    file_path: str,
    python_exe: PythonCommand,
    build_profile: str,
    targets: tuple[str, ...] = ("native",),
    target_python: str | TargetPythonVersion | None = None,
    item_budget: compat_backends.ItemBudget | None = None,
) -> dict[str, object]:
    _install_worker_orphan_guard()
    started = time.monotonic()
    buffer_out = io.StringIO()
    buffer_err = io.StringIO()
    with contextlib.redirect_stdout(buffer_out), contextlib.redirect_stderr(buffer_err):
        record = diff_test(
            file_path,
            python_exe,
            build_profile=build_profile,
            targets=targets,
            target_python=target_python,
            item_budget=item_budget,
        )
    return {
        "path": file_path,
        "status": record["resolved_status"],
        "guard_signal": record["guard_signal"],
        "stdout": buffer_out.getvalue(),
        "stderr": buffer_err.getvalue(),
        "duration_s": time.monotonic() - started,
    }


def _install_worker_state(suite_stop: object) -> None:
    """Pool initializer: bind the suite's stop and the orphan guard."""
    compat_backends.bind_suite_stop(suite_stop)
    _install_worker_orphan_guard()


def _install_worker_orphan_guard() -> None:
    global _WORKER_ORPHAN_GUARD_INSTALLED
    if _WORKER_ORPHAN_GUARD_INSTALLED:
        return
    import multiprocessing

    parent = multiprocessing.parent_process()
    if parent is None:
        return
    _WORKER_ORPHAN_GUARD_INSTALLED = True

    def _watch_parent() -> None:
        while True:
            time.sleep(1.0)
            # Observe the creating process's sentinel, rather than inferring
            # its lifetime from a changing or reused OS parent identifier.
            if not parent.is_alive():
                os._exit(0)

    threading.Thread(
        target=_watch_parent, name="molt-diff-orphan-guard", daemon=True
    ).start()


class _TeeStream(io.TextIOBase):
    def __init__(self, *handles: io.TextIOBase) -> None:
        self._handles = handles

    def write(self, s: str) -> int:
        for handle in self._handles:
            handle.write(s)
        return len(s)

    def flush(self) -> None:
        for handle in self._handles:
            handle.flush()


def _diff_run_single(
    file_path: str,
    python_exe: PythonCommand,
    build_profile: str,
    targets: tuple[str, ...] = ("native",),
    target_python: str | TargetPythonVersion | None = None,
    item_budget: compat_backends.ItemBudget | None = None,
) -> dict[str, object]:
    started = time.monotonic()
    buffer_out = io.StringIO()
    buffer_err = io.StringIO()
    out_stream = _TeeStream(sys.stdout, buffer_out)
    err_stream = _TeeStream(sys.stderr, buffer_err)
    with contextlib.redirect_stdout(out_stream), contextlib.redirect_stderr(err_stream):
        record = diff_test(
            file_path,
            python_exe,
            build_profile=build_profile,
            targets=targets,
            target_python=target_python,
            item_budget=item_budget,
        )
    return {
        "path": file_path,
        "status": record["resolved_status"],
        "guard_signal": record["guard_signal"],
        "stdout": buffer_out.getvalue(),
        "stderr": buffer_err.getvalue(),
        "duration_s": time.monotonic() - started,
    }


def _append_aggregate_log(
    handle: io.TextIOBase,
    file_path: str,
    status: str,
    stdout: str,
    stderr: str,
) -> None:
    handle.write(f"=== [{status.upper()}] {file_path} ===\n")
    if stdout:
        handle.write("STDOUT:\n")
        handle.write(stdout)
        if not stdout.endswith("\n"):
            handle.write("\n")
    if stderr:
        if stdout:
            handle.write("\n")
        handle.write("STDERR:\n")
        handle.write(stderr)
        if not stderr.endswith("\n"):
            handle.write("\n")
    handle.write("\n")
    handle.flush()


def _time_tool() -> str | None:
    path = Path("/usr/bin/time")
    return str(path) if path.exists() else None


def _parse_time_metrics(path: Path) -> dict[str, int]:
    metrics: dict[str, int] = {}
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return metrics
    for line in text.splitlines():
        raw = line.strip()
        if not raw:
            continue
        value: int | None = None
        if ":" in raw:
            maybe = raw.split(":", 1)[1].strip().split()[0]
            if maybe.isdigit():
                value = int(maybe)
        else:
            parts = raw.split()
            if parts and parts[0].isdigit():
                value = int(parts[0])
        if value is None:
            continue
        if "maximum resident set size" in raw or "Maximum resident set size" in raw:
            if sys.platform == "darwin":
                value = max(1, value // 1024)
            metrics["max_rss"] = value
        elif "peak memory footprint" in raw:
            if sys.platform == "darwin":
                value = max(1, value // 1024)
            metrics["peak_footprint"] = value
    return metrics


def _popen_group_kwargs() -> dict[str, object]:
    return harness_memory_guard.batch_process_group_kwargs(_diff_memory_guard_limits())


def _diff_memory_guard_root() -> Path:
    root = _diff_root() / "memory_guard"
    root.mkdir(parents=True, exist_ok=True)
    return root


def _diff_memory_guard_trip_file() -> Path:
    raw = os.environ.get(_DIFF_MEMORY_GUARD_TRIP_FILE_ENV, "").strip()
    return (
        Path(raw).expanduser()
        if raw
        else _diff_root() / "memory_guard" / "tripped.json"
    )


def _diff_memory_guard_events_jsonl() -> Path:
    raw = os.environ.get(_DIFF_MEMORY_GUARD_EVENTS_JSONL_ENV, "").strip()
    path = Path(raw).expanduser() if raw else _diff_memory_guard_root() / "events.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    return path


def _diff_memory_guard_global_samples_jsonl() -> Path:
    raw = os.environ.get(_DIFF_MEMORY_GUARD_GLOBAL_SAMPLES_JSONL_ENV, "").strip()
    path = (
        Path(raw).expanduser()
        if raw
        else _diff_memory_guard_root() / "global_samples.jsonl"
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    return path


def _memory_guard_jsonl_max_bytes(path: Path) -> int | None:
    if path.name.startswith("global_samples"):
        env_name = "MOLT_DIFF_MEMORY_GUARD_MAX_SAMPLE_MB"
        default_mb = _DIFF_MEMORY_GUARD_DEFAULT_SAMPLE_MAX_MB
    else:
        env_name = "MOLT_DIFF_MEMORY_GUARD_MAX_EVENT_MB"
        default_mb = _DIFF_MEMORY_GUARD_DEFAULT_EVENT_MAX_MB
    value = _parse_float_env(env_name)
    if value is None:
        value = default_mb
    if value <= 0:
        return None
    return max(1024, int(value * 1024 * 1024))


def _rotate_memory_guard_jsonl(path: Path, incoming_bytes: int) -> None:
    max_bytes = _memory_guard_jsonl_max_bytes(path)
    if max_bytes is None:
        return
    try:
        current_size = path.stat().st_size
    except FileNotFoundError:
        return
    except OSError:
        return
    if current_size + incoming_bytes <= max_bytes:
        return
    rotated = path.with_name(f"{path.name}.1")
    with contextlib.suppress(OSError):
        rotated.unlink()
    with contextlib.suppress(OSError):
        path.replace(rotated)


def _append_memory_guard_jsonl(path: Path, payload: dict[str, object]) -> None:
    payload = {"ts": time.time(), **payload}
    try:
        line = json.dumps(payload, sort_keys=True) + "\n"
        _rotate_memory_guard_jsonl(path, len(line.encode("utf-8")))
        with path.open("a", encoding="utf-8") as handle:
            handle.write(line)
    except OSError:
        return


def _memory_guard_payload_gb(payload: dict[str, object], key: str) -> str:
    value = payload.get(key)
    if isinstance(value, (int, float)):
        return f"{value:.2f}GB"
    return "-"


def _memory_guard_record_gb(record: object) -> str:
    if not isinstance(record, dict):
        return "-"
    return _memory_guard_payload_gb(record, "rss_gb")


def _memory_guard_peak_tree_gb(payload: dict[str, object]) -> str:
    trees = payload.get("trees")
    if not isinstance(trees, list):
        return "-"
    values: list[float] = []
    for tree in trees:
        if not isinstance(tree, dict):
            continue
        total = tree.get("total")
        if not isinstance(total, dict):
            continue
        rss_gb = total.get("rss_gb")
        if isinstance(rss_gb, (int, float)):
            values.append(float(rss_gb))
    if not values:
        return "-"
    return f"{max(values):.2f}GB"


def _format_memory_guard_stream(payload: dict[str, object]) -> str:
    event = str(payload.get("event", "sample"))
    if event == "sample":
        roots = payload.get("active_roots")
        root_count = len(roots) if isinstance(roots, list) else 0
        return (
            "[MEMORY-GUARD] "
            f"sample total={_memory_guard_payload_gb(payload, 'total_gb')} "
            f"roots={root_count} peak_tree={_memory_guard_peak_tree_gb(payload)}"
        )
    if event in {"guard_tripped", "subprocess_guard_tripped", "memory_guard_trip"}:
        message = payload.get("message")
        if isinstance(message, str) and message:
            return f"[MEMORY-GUARD] TRIP {message}"
        return (
            "[MEMORY-GUARD] TRIP "
            f"violation={_memory_guard_record_gb(payload.get('violation'))}"
        )
    if event == "run_started":
        return (
            "[MEMORY-GUARD] "
            f"run_started global={_memory_guard_payload_gb(payload, 'global_gb')} "
            f"tree={_memory_guard_payload_gb(payload, 'max_tree_gb')} "
            f"process={_memory_guard_payload_gb(payload, 'max_process_gb')} "
            f"child_rlimit={_memory_guard_payload_gb(payload, 'child_rlimit_gb')} "
            f"poll={payload.get('poll_interval')}s "
            f"samples={'on' if _diff_memory_guard_write_samples() else 'off'}"
        )
    if event == "monitor_error":
        return f"[MEMORY-GUARD] monitor_error {payload.get('error', '')}"
    return f"[MEMORY-GUARD] {event}"


def _stream_memory_guard_payload(payload: dict[str, object]) -> None:
    mode = _diff_memory_guard_stream_mode()
    target = _diff_memory_guard_stream_target(mode)
    if target is None:
        return
    try:
        if "json" in mode:
            line = json.dumps({"ts": time.time(), **payload}, sort_keys=True)
        else:
            line = _format_memory_guard_stream(payload)
        print(line, file=target, flush=True)
    except Exception:
        return


def _record_memory_guard_event(payload: dict[str, object]) -> None:
    _append_memory_guard_jsonl(_diff_memory_guard_events_jsonl(), payload)
    _stream_memory_guard_payload(payload)


def _record_memory_guard_sample(payload: dict[str, object]) -> None:
    if _diff_memory_guard_write_samples():
        _append_memory_guard_jsonl(_diff_memory_guard_global_samples_jsonl(), payload)
    _stream_memory_guard_payload(payload)


def _memory_guard_record(
    record: memory_guard.RssViolation | None,
) -> dict[str, object] | None:
    if record is None:
        return None
    return {
        "pid": record.pid,
        "rss_kb": record.rss_kb,
        "rss_gb": record.rss_gb,
        "command": record.command,
        "scope": record.scope,
    }


def _memory_guard_message(
    violation: memory_guard.RssViolation,
    *,
    limit_gb: float,
    phase: str,
) -> str:
    return (
        "molt_diff memory guard: RSS limit exceeded "
        f"phase={phase} scope={violation.scope} pid={violation.pid} "
        f"rss={violation.rss_gb:.2f}GB limit={limit_gb:.2f}GB "
        f"command={violation.command}"
    )


def _mark_memory_guard_tripped(payload: dict[str, object]) -> None:
    data = {"ts": time.time(), **payload}
    harness_outcomes.publish_suite_trip(_diff_memory_guard_trip_file(), data)
    _record_memory_guard_event(data)


def _memory_guard_trip_outcome(
    sentinel: harness_memory_guard.RepoProcessMemorySentinel | None = None,
    *,
    environment: Mapping[str, str] | None = None,
) -> compat_backends.BackendResult | None:
    failure = getattr(sentinel, "infrastructure_failure", None)
    evidence = (
        harness_outcomes.SuiteTripEvidence(infrastructure_failure=failure)
        if failure is not None
        else harness_outcomes.read_suite_trip(
            os.environ if environment is None else environment
        )
    )
    if evidence is None and getattr(sentinel, "tripped", False):
        evidence = harness_outcomes.suite_trip_failure(
            "memory_guard: suite sentinel tripped without published victim evidence"
        )
    return compat_backends.suite_trip_outcome(evidence)


def _prepare_memory_guard_run(config: _DiffMemoryGuardConfig) -> None:
    global _LAST_SENTINEL_SAMPLE_WRITE
    _LAST_SENTINEL_SAMPLE_WRITE = 0.0
    guard_root = _diff_memory_guard_root()
    os.environ[_DIFF_MEMORY_GUARD_TRIP_FILE_ENV] = str(guard_root / "tripped.json")
    os.environ[_DIFF_MEMORY_GUARD_EVENTS_JSONL_ENV] = str(guard_root / "events.jsonl")
    os.environ[_DIFF_MEMORY_GUARD_GLOBAL_SAMPLES_JSONL_ENV] = str(
        guard_root / "global_samples.jsonl"
    )
    with contextlib.suppress(FileNotFoundError):
        Path(os.environ[_DIFF_MEMORY_GUARD_TRIP_FILE_ENV]).unlink()
    _record_memory_guard_event(
        {
            "event": "run_started",
            **_config_payload(config),
            "sample_interval": _diff_memory_guard_sample_interval_sec(),
            "write_samples": _diff_memory_guard_write_samples(),
            "stream_mode": _diff_memory_guard_stream_mode(),
            "event_max_bytes": _memory_guard_jsonl_max_bytes(
                Path(os.environ[_DIFF_MEMORY_GUARD_EVENTS_JSONL_ENV])
            ),
            "sample_max_bytes": _memory_guard_jsonl_max_bytes(
                Path(os.environ[_DIFF_MEMORY_GUARD_GLOBAL_SAMPLES_JSONL_ENV])
            ),
        }
    )


_LAST_SENTINEL_SAMPLE_WRITE = 0.0


def _process_sample_record(
    sample: memory_guard.ProcessSample | None,
) -> dict[str, object] | None:
    if sample is None:
        return None
    return _memory_guard_record(
        memory_guard.RssViolation(
            pid=sample.pid,
            rss_kb=sample.rss_kb,
            command=sample.command,
            scope="process",
        )
    )


def _process_group_record(
    group: process_sentinel.ProcessGroup,
) -> dict[str, object]:
    peak = group.peak
    command = group.command_text if peak is None else peak.command
    return {
        "pgid": group.pgid,
        "pids": group.pids,
        "peak": _process_sample_record(peak),
        "total": _memory_guard_record(
            memory_guard.RssViolation(
                pid=group.pgid,
                rss_kb=group.total_rss_kb,
                command=command,
                scope="process_group",
            )
        ),
    }


def _record_memory_guard_sentinel_sample(
    groups: Sequence[process_sentinel.ProcessGroup],
    limits: memory_guard.ResolvedMemoryLimits,
    _elapsed_s: float,
) -> None:
    global _LAST_SENTINEL_SAMPLE_WRITE
    if not groups:
        return
    now = time.monotonic()
    if now - _LAST_SENTINEL_SAMPLE_WRITE < _diff_memory_guard_sample_interval_sec():
        return
    _LAST_SENTINEL_SAMPLE_WRITE = now
    total_kb = sum(group.total_rss_kb for group in groups)
    _record_memory_guard_sample(
        {
            "event": "sample",
            "active_roots": [group.pgid for group in groups],
            "total_kb": total_kb,
            "total_gb": total_kb / (1024 * 1024),
            "limits": memory_guard.memory_limits_payload(limits),
            "trees": [_process_group_record(group) for group in groups],
        }
    )


def _sentinel_violation_as_rss(
    violation: process_sentinel.SentinelViolation,
    payload: Mapping[str, object],
) -> memory_guard.RssViolation:
    if violation.reason == "process_rss":
        return memory_guard.RssViolation(
            pid=0 if violation.peak_pid is None else violation.peak_pid,
            rss_kb=violation.total_rss_kb
            if violation.peak_rss_kb is None
            else violation.peak_rss_kb,
            command=violation.command,
            scope="process",
        )
    if violation.reason == "global_rss":
        global_total = payload.get("global_total_kb")
        rss_kb = violation.total_rss_kb
        if isinstance(global_total, int):
            rss_kb = global_total
        return memory_guard.RssViolation(
            pid=0,
            rss_kb=rss_kb,
            command="all repo-scoped molt_diff process groups",
            scope="diff_global_process_groups",
        )
    return memory_guard.RssViolation(
        pid=violation.pgid,
        rss_kb=violation.total_rss_kb,
        command=violation.command,
        scope="process_tree",
    )


def _record_memory_guard_sentinel_violation(
    violation: process_sentinel.SentinelViolation,
    limits: memory_guard.ResolvedMemoryLimits,
    payload: Mapping[str, object],
) -> None:
    rss_violation = _sentinel_violation_as_rss(violation, payload)
    if violation.reason == "global_rss":
        reason = "global memory guard tripped"
        phase = "global"
        limit_gb = limits.max_global_rss_gb
    elif violation.reason == "process_rss":
        reason = "per-process memory guard tripped"
        phase = f"pgid={violation.pgid}"
        limit_gb = limits.max_process_rss_gb
    else:
        reason = "per-tree memory guard tripped"
        phase = f"pgid={violation.pgid}"
        limit_gb = limits.max_total_rss_gb
    active_roots = payload.get("active_pgids")
    roots = active_roots if isinstance(active_roots, list) else [violation.pgid]
    global_total_kb = payload.get("global_total_kb", violation.total_rss_kb)
    if not isinstance(global_total_kb, int):
        global_total_kb = violation.total_rss_kb
    _mark_memory_guard_tripped(
        {
            "event": "guard_tripped",
            "reason": reason,
            "message": _memory_guard_message(
                rss_violation,
                limit_gb=limits.max_process_rss_gb if limit_gb is None else limit_gb,
                phase=phase,
            ),
            "violation": _memory_guard_record(rss_violation),
            "global_total_kb": global_total_kb,
            "global_total_gb": global_total_kb / (1024 * 1024),
            "active_roots": roots,
            "shared_sentinel_event": dict(payload),
        }
    )


def _force_close_diff_process_group(proc: subprocess.Popen[str]) -> None:
    harness_memory_guard.force_close_process_group(proc)


def _run_subprocess(
    cmd: list[str], *, env: dict[str, str], timeout: float | None
) -> compat_backends.BackendResult:
    trip = _memory_guard_trip_outcome(environment=env)
    if trip is not None:
        return trip
    stopped_by = compat_backends.suite_stop_signal()
    if stopped_by is not None:
        return compat_backends.suite_stop_outcome(stopped_by)

    guard_context = harness_memory_guard.HarnessExecutionContext.from_env(
        "MOLT_DIFF",
        env,
        repo_root=Path(_repo_root()),
        artifact_root=_materialize_artifact_root(
            _diff_artifact_layout(environment=env).diff_root
        ),
        limits=_diff_memory_guard_limits(env),
    )
    result = guard_context.run(
        cmd,
        cwd=Path(_repo_root()),
        timeout=timeout,
        capture_output=True,
        text=False,
        cancellation_requested=compat_backends.suite_stop_requested,
    )
    outcome = compat_backends.merge_suite_trip_result(
        compat_backends.with_suite_stop(
            compat_backends.BackendResult.from_process(result), result
        ),
        result,
        harness_outcomes.read_suite_trip(env),
    )
    if getattr(result, "violation", None) is not None:
        guard_message = (
            "molt_diff memory guard: RSS limit exceeded under shared harness "
            "subprocess guard; inspect preceding memory_guard diagnostic for "
            "pid/rss/limit details"
        )
        _record_memory_guard_event(
            {
                "event": "subprocess_guard_tripped",
                "message": guard_message,
                "command": cmd,
                "violation": _memory_guard_record(getattr(result, "violation", None)),
            }
        )
        outcome = replace(outcome, stderr=outcome.stderr + guard_message + "\n")
    return outcome


def _run_with_optional_time(
    cmd: list[str],
    *,
    env: dict[str, str],
    timeout: float | None,
    time_path: Path | None,
) -> compat_backends.BackendResult:
    run_cmd = cmd
    if time_path is not None:
        time_bin = _time_tool()
        if time_bin is not None:
            if sys.platform == "darwin":
                run_cmd = [time_bin, "-l", "-o", str(time_path), *cmd]
            else:
                run_cmd = [time_bin, "-v", "-o", str(time_path), *cmd]
    return _run_subprocess(run_cmd, env=env, timeout=timeout)


def _record_rss_metrics(
    file_path: str,
    *,
    build_metrics: dict[str, int] | None,
    run_metrics: dict[str, int] | None,
    build_rc: int | None,
    run_rc: int | None,
    status: str,
) -> None:
    if not _diff_measure_rss():
        return
    run_id = os.environ.get("MOLT_DIFF_RUN_ID", "").strip() or None
    payload = {
        "run_id": run_id,
        "timestamp": time.time(),
        "file": file_path,
        "status": status,
        "build_rc": build_rc,
        "run_rc": run_rc,
        "build": build_metrics or {},
        "run": run_metrics or {},
    }
    summary_path = _diff_root() / "rss_metrics.jsonl"
    try:
        with summary_path.open("a") as fh:
            fh.write(json.dumps(payload, sort_keys=True) + "\n")
    except OSError:
        return


class _BatchCompileServerClient(BatchCompileServerClient):
    def __init__(
        self, env: dict[str, str], *, budget: compat_backends.ItemBudget
    ) -> None:
        cmd = [
            _resolve_molt_cli_python(),
            "-m",
            "molt.cli",
            "internal-batch-build-server",
        ]
        guard_context = harness_memory_guard.HarnessExecutionContext.from_env(
            "MOLT_DIFF",
            env,
            repo_root=Path(_repo_root()),
            limits=_diff_memory_guard_limits(env),
        )
        budget.check(build_failed=True)
        super().__init__(
            cmd,
            cwd=Path(_repo_root()),
            env=env,
            reader_name="molt-diff-batch-server-reader",
            guard_context=guard_context,
            force_close=_force_close_diff_process_group,
        )

    def close(self, *, force: bool = False, timeout: float | None = None) -> None:
        request_timeout = timeout
        if request_timeout is None:
            request_timeout = _diff_batch_compile_server_request_timeout()
        super().close(force=force, timeout=request_timeout)


def _shutdown_batch_compile_server(*, force: bool = True) -> None:
    global _BATCH_COMPILE_SERVER_CLIENT
    global _BATCH_COMPILE_SERVER_CLIENT_PID
    client = _BATCH_COMPILE_SERVER_CLIENT
    if client is None:
        return
    _BATCH_COMPILE_SERVER_CLIENT = None
    _BATCH_COMPILE_SERVER_CLIENT_PID = 0
    client.close(force=force)


def _batch_compile_server_client(
    env: dict[str, str],
    *,
    request_timeout: float,
    budget: compat_backends.ItemBudget,
) -> tuple[_BatchCompileServerClient | None, Exception | None]:
    global _BATCH_COMPILE_SERVER_CLIENT
    global _BATCH_COMPILE_SERVER_CLIENT_PID
    pid = os.getpid()
    if _BATCH_COMPILE_SERVER_CLIENT_PID != pid:
        _shutdown_batch_compile_server(force=True)
    if _BATCH_COMPILE_SERVER_CLIENT is not None:
        return _BATCH_COMPILE_SERVER_CLIENT, None
    client: _BatchCompileServerClient | None = None
    try:
        if budget.expired():
            raise TimeoutError(
                "differential item budget exhausted before batch startup"
            )
        client = _BatchCompileServerClient(env, budget=budget)
        request_timeout = budget.limit(request_timeout)
        if request_timeout == 0:
            raise TimeoutError("differential item budget exhausted before batch ping")
        response = client.request(
            "ping",
            timeout=request_timeout,
        )
        if not bool(response.get("ok")) or not bool(response.get("pong")):
            raise RuntimeError("batch compile server ping failed")
    except Exception as exc:
        if client is not None:
            with contextlib.suppress(Exception):
                client.close(force=True)
        return None, exc
    _BATCH_COMPILE_SERVER_CLIENT = client
    _BATCH_COMPILE_SERVER_CLIENT_PID = pid
    atexit.register(_shutdown_batch_compile_server)
    return client, None


def _run_batch_compile_build(
    *,
    env: dict[str, str],
    file_path: str,
    output_root: Path,
    output_binary: Path,
    build_profile: str,
    target_python: TargetPythonVersion | None,
    no_cache: bool,
    rebuild: bool,
    request_timeout: float,
    budget: compat_backends.ItemBudget,
) -> compat_backends.BackendResult:
    diff_caps = _diff_capabilities(env)
    try:
        stdlib_profile = compat_backends.stdlib_profile_from_environment(env)
        request = BuildRequest(
            file_path=file_path,
            profile=cast(BuildProfile, build_profile),
            output=str(output_binary),
            out_dir=str(output_root),
            emit="bin",
            cache=not no_cache and not rebuild,
            respect_pythonpath=True,
            python_version=target_python.short if target_python is not None else None,
            stdlib_profile=stdlib_profile,
            capabilities=diff_caps or None,
        )
        if "MOLT_CODEC" in env:
            request = replace(request, parse_codec=cast(ParseCodec, env["MOLT_CODEC"]))
        params = build_request_to_payload(request)
    except ValueError as exc:
        return compat_backends.BackendResult(None, str(exc), 2, build_failed=True)
    build_cap = budget.policy.phase("native", "build")
    if build_cap is not None:
        request_timeout = (
            build_cap if request_timeout is None else min(request_timeout, build_cap)
        )
    admission = compat_backends.suite_trip_outcome(
        harness_outcomes.read_suite_trip(env)
    )
    if admission is not None:
        return admission.as_build_failure(
            detail="suite guard stopped batch admission", fallback="suite guard tripped"
        )
    if budget.expired():
        return budget.exhausted_result(build_failed=True)
    client, start_error = _batch_compile_server_client(
        env, request_timeout=budget.limit(request_timeout), budget=budget
    )
    if client is None:
        error = start_error or RuntimeError("batch compile server startup failed")
        if isinstance(error, (TimeoutError, compat_backends.ItemBudgetExpired)):
            return compat_backends.BackendResult.from_deadline(
                timeout=request_timeout, stderr=str(error), build_failed=True
            )
        admission = compat_backends.suite_trip_outcome(
            harness_outcomes.read_suite_trip(env)
        )
        if admission is not None:
            return admission.as_build_failure(
                detail="suite guard stopped batch startup",
                fallback="suite guard tripped",
            )
        return compat_backends.BackendResult(
            None,
            str(error),
            127,
            build_failed=True,
            detail="batch compile server startup failed",
        )
    request_timeout = budget.limit(request_timeout)
    if request_timeout == 0:
        return budget.exhausted_result(build_failed=True)
    try:
        response = client.request(
            "build", params=params, env_overrides=env, timeout=request_timeout
        )
    except Exception as error:
        # Capture the submitted request's outcome before shutdown can change
        # process observations. An indeterminate request is never resubmitted.
        failure = compat_backends.BackendResult(
            None, str(error), 127, build_failed=True
        )
        evidence = harness_outcomes.read_suite_trip(env)
        deadline = isinstance(error, (TimeoutError, compat_backends.ItemBudgetExpired))
        if not deadline:
            failure = compat_backends.merge_suite_trip_result(
                failure, getattr(error, "batch_request_custody", None), evidence
            )
        _shutdown_batch_compile_server(force=True)
        if failure.rss_limit_exceeded:
            return failure
        if deadline:
            return compat_backends.BackendResult.from_deadline(
                timeout=request_timeout, stderr=str(error), build_failed=True
            )
        if evidence is not None and evidence.infrastructure_failure is not None:
            return compat_backends.suite_trip_outcome(evidence).as_build_failure(
                detail="invalid suite trip evidence", fallback="suite guard failure"
            )
        return failure
    stdout = response.get("stdout")
    stderr = response.get("stderr")
    error = response.get("error")
    returncode = response.get("returncode")
    if not isinstance(returncode, int):
        returncode = 1 if not bool(response.get("ok")) else 0
    out_text = stdout if isinstance(stdout, str) else ""
    err_text = stderr if isinstance(stderr, str) else ""
    if isinstance(error, str) and error:
        err_text = f"{err_text}\n{error}" if err_text else error
    result = compat_backends.BackendResult(out_text, err_text, returncode)
    if returncode != 0:
        result = compat_backends.merge_suite_trip_result(
            result, response.custody, harness_outcomes.read_suite_trip(env)
        )
        return result.as_build_failure(
            detail="batch compile failed", fallback="batch compile failed"
        )
    return result


def run_cpython(
    file_path,
    python_exe=sys.executable,
    *,
    budget: compat_backends.ItemBudget | None = None,
) -> compat_backends.BackendResult:
    environment = os.environ.copy()
    if budget is None:
        budget = compat_backends.ItemBudget.start(
            compat_backends.TimeoutPolicy.from_environment(environment), ()
        )
    trip = _memory_guard_trip_outcome(environment=environment)
    if trip is not None:
        return trip
    if budget.expired():
        return budget.exhausted_result()
    tmp_root = _diff_tmp_root(environment)
    lease = diff_output_layout.new_guest_leaf(
        tmp_root,
        prefix="cpython_tmp_",
        boundary=tmp_root,
        environment=environment,
        repo_root=_repo_root(),
    )
    return compat_backends.run_with_guest_outputs(
        [lease],
        lambda: _run_cpython_owned(
            file_path,
            python_exe=python_exe,
            cpython_tmp=lease.path,
            environment=environment,
            budget=budget,
        ),
        environment=environment,
        repo_root=_repo_root(),
    )


def _run_cpython_owned(
    file_path,
    *,
    python_exe,
    cpython_tmp: Path,
    environment: Mapping[str, str],
    budget: compat_backends.ItemBudget,
) -> compat_backends.BackendResult:
    python_command = _resolve_python_command(python_exe, budget=budget)
    _apply_memory_limit()
    env = dict(environment)
    # Keep CPython baseline path resolution aligned with the Molt build/run env.
    env["PYTHONPATH"] = "src"
    env["PYTHONHASHSEED"] = "0"
    # Keep CPython tempfile roots aligned with Molt subprocess roots so path
    # semantics (especially macOS /var vs /private/var) are compared fairly.
    env["TMPDIR"] = str(cpython_tmp)
    env["TEMP"] = str(cpython_tmp)
    env["TMP"] = str(cpython_tmp)
    env.update(_collect_env_overrides(file_path))
    diff_output_layout.enforce_child(env, repo_root=_repo_root())
    bootstrap = """
def _molt_diff_execute_script():
    import builtins as _builtins
    import importlib.machinery as _machinery
    import os as _os
    import sys as _sys

    _script_arg = _sys.argv[1]
    _script_path = _os.path.abspath(_script_arg)
    _script_dir = _os.path.dirname(_script_path)
    # ``python -c`` makes the current working directory sys.path[0].  A
    # directly executed script instead owns that slot with its containing
    # directory, while argv[0] retains the spelling supplied by the caller.
    if _sys.path:
        _sys.path[0] = _script_dir
    else:
        _sys.path.insert(0, _script_dir)
    _sys.argv[:] = [_script_arg, *_sys.argv[2:]]

    if "importlib_resources_" in _script_path:
        # Keep zipfile available even when tests temporarily replace sys.path
        # with synthetic roots that omit the host stdlib path.
        import zipfile as _molt_diff_zipfile_preload  # noqa: F401
        try:
            from importlib.readers import MultiplexedPath as _MoltDiffMultiplexedPath
        except Exception:
            _MoltDiffMultiplexedPath = None
        if _MoltDiffMultiplexedPath is not None and not hasattr(
            _MoltDiffMultiplexedPath, "__fspath__"
        ):
            def _molt_diff_multiplexed_fspath(self):
                paths = getattr(self, "_paths", None)
                if isinstance(paths, (list, tuple)) and paths:
                    return str(paths[0])
                return str(self)
            _MoltDiffMultiplexedPath.__fspath__ = _molt_diff_multiplexed_fspath

    if "importlib_extension_exec_" in _script_path:
        _orig_create_module = _machinery.ExtensionFileLoader.create_module
        _orig_exec_module = _machinery.ExtensionFileLoader.exec_module

        def _strip_ext(path):
            for suffix in (".so", ".pyd", ".dll", ".dylib"):
                if path.endswith(suffix):
                    return path[: -len(suffix)]
            return None

        def _strip_cpython_tag(stem):
            marker = ".cpython-"
            if marker in stem:
                return stem.split(marker, 1)[0]
            return stem

        def _candidate_shim_paths(module_file):
            out = []
            if not module_file:
                return out

            def _append(candidate):
                if candidate and candidate not in out:
                    out.append(candidate)

            _append(f"{module_file}.molt.py")
            _append(f"{module_file}.py")

            stripped = _strip_ext(module_file)
            if stripped:
                _append(f"{stripped}.molt.py")
                _append(f"{stripped}.py")
                stripped_tag = _strip_cpython_tag(stripped)
                if stripped_tag != stripped:
                    _append(f"{stripped_tag}.molt.py")
                    _append(f"{stripped_tag}.py")

            dirname = _os.path.dirname(module_file)
            basename = _os.path.basename(module_file)
            base_noext = _strip_ext(basename) or basename
            base_tag = _strip_cpython_tag(base_noext)

            if dirname:
                if base_tag.startswith("__init__"):
                    _append(_os.path.join(dirname, "__init__.molt.py"))
                    _append(_os.path.join(dirname, "__init__.py"))

                if _os.path.basename(dirname) == "__pycache__":
                    parent = _os.path.dirname(dirname)
                    _append(_os.path.join(parent, f"{base_tag}.molt.py"))
                    _append(_os.path.join(parent, f"{base_tag}.py"))

            return out

        def _molt_diff_create_module(self, spec):
            try:
                return _orig_create_module(self, spec)
            except (ImportError, OSError, PermissionError):
                return None

        def _molt_diff_exec_module(self, module):
            module_file = getattr(module, "__file__", None)
            if not module_file:
                spec = getattr(module, "__spec__", None)
                module_file = (
                    getattr(spec, "origin", None) if spec is not None else None
                )
            shim_path = None
            for candidate in _candidate_shim_paths(module_file):
                if _os.path.exists(candidate):
                    shim_path = candidate
                    break
            shim_exists = bool(shim_path)
            exec_failed = False
            try:
                _orig_exec_module(self, module)
            except (ImportError, OSError, PermissionError):
                exec_failed = True
                if not shim_exists:
                    raise
            if shim_exists:
                with open(shim_path, "rb") as _shim_file:
                    _shim_src = _shim_file.read()
                try:
                    _shim_code = compile(_shim_src, shim_path, "exec")
                except SyntaxError:
                    _shim_text = _shim_src.decode("utf-8", "surrogateescape")
                    _shim_unescaped = bytes(_shim_text, "utf-8").decode(
                        "unicode_escape"
                    )
                    _shim_code = compile(_shim_unescaped, shim_path, "exec")
                exec(_shim_code, module.__dict__, module.__dict__)
            elif not exec_failed:
                # Match Molt runtime behavior where extension execution without
                # a loadable shim is treated as unavailable.
                raise ImportError("extension execution unavailable")
            return None

        _machinery.ExtensionFileLoader.create_module = _molt_diff_create_module
        _machinery.ExtensionFileLoader.exec_module = _molt_diff_exec_module

    _script_loader = _machinery.SourceFileLoader("__main__", _script_path)
    _script_code = compile(
        _script_loader.get_data(_script_path), _script_path, "exec"
    )
    _main_globals = vars(_sys.modules["__main__"])
    _main_globals.update(
        __name__="__main__",
        __file__=_script_path,
        __cached__=None,
        __doc__=None,
        __loader__=_script_loader,
        __package__=None,
        __spec__=None,
        __builtins__=_builtins,
    )
    # The helper frame retains everything needed to execute the script.  Drop
    # the sole bootstrap global first so user globals and atexit callbacks see
    # the same persistent __main__ namespace as direct CPython script entry.
    del _main_globals["_molt_diff_execute_script"]
    exec(_script_code, _main_globals, _main_globals)


_molt_diff_execute_script()
"""
    timeout = budget.limit(budget.policy.cpython)
    if timeout == 0:
        return budget.exhausted_result()
    result = _run_subprocess(
        [*python_command, "-c", bootstrap, file_path],
        env=env,
        timeout=timeout,
    )
    return compat_backends.BackendResult.from_process(result)


def run_molt(
    file_path: str,
    build_profile: str,
    *,
    daemon_enabled: bool | None = None,
    no_cache: bool = False,
    rebuild: bool = False,
    extra_env: dict[str, str] | None = None,
    execution_context: compat_backends.BackendExecutionContext | None = None,
) -> compat_backends.BackendResult:
    return _run_molt(
        file_path,
        build_only=False,
        build_profile=build_profile,
        daemon_enabled=daemon_enabled,
        no_cache=no_cache,
        rebuild=rebuild,
        extra_env=extra_env,
        execution_context=execution_context,
    )


def run_molt_build_only(
    file_path: str,
    build_profile: str,
    *,
    daemon_enabled: bool | None = None,
    no_cache: bool = False,
    rebuild: bool = False,
    extra_env: dict[str, str] | None = None,
    execution_context: compat_backends.BackendExecutionContext | None = None,
) -> compat_backends.BackendResult:
    return _run_molt(
        file_path,
        build_only=True,
        build_profile=build_profile,
        daemon_enabled=daemon_enabled,
        no_cache=no_cache,
        rebuild=rebuild,
        extra_env=extra_env,
        execution_context=execution_context,
    )


def _run_molt(
    file_path: str,
    *,
    build_only: bool,
    build_profile: str,
    daemon_enabled: bool | None,
    no_cache: bool,
    rebuild: bool,
    extra_env: dict[str, str] | None,
    execution_context: compat_backends.BackendExecutionContext | None,
) -> compat_backends.BackendResult:
    budget = (
        execution_context.budget
        if execution_context is not None
        else compat_backends.ItemBudget.start(
            compat_backends.TimeoutPolicy.from_environment(os.environ),
            ("native",),
            cpython=False,
            build_only=build_only,
        )
    )
    budget = budget.for_backend("native", build_only=build_only)
    environment = (
        dict(execution_context.environment)
        if execution_context is not None
        else os.environ.copy()
    )
    if extra_env:
        environment.update(extra_env)
    trip = _memory_guard_trip_outcome(environment=environment)
    if trip is not None:
        return trip
    if budget.expired():
        return budget.exhausted_result(build_failed=True)
    tmp_root = _diff_tmp_root(environment)
    lease = diff_output_layout.new_guest_leaf(
        tmp_root,
        prefix="molt_diff_",
        boundary=tmp_root,
        environment=environment,
        repo_root=_repo_root(),
    )
    return compat_backends.run_with_guest_outputs(
        [lease],
        lambda: _run_molt_owned(
            file_path,
            build_only=build_only,
            build_profile=build_profile,
            daemon_enabled=daemon_enabled,
            no_cache=no_cache,
            rebuild=rebuild,
            extra_env=extra_env,
            execution_context=execution_context,
            budget=budget,
            output_root=lease.path,
            environment=environment,
        ),
        environment=environment,
        repo_root=_repo_root(),
    )


def _run_molt_owned(
    file_path: str,
    *,
    build_only: bool,
    build_profile: str,
    daemon_enabled: bool | None,
    no_cache: bool,
    rebuild: bool,
    extra_env: dict[str, str] | None,
    execution_context: compat_backends.BackendExecutionContext | None,
    output_root: Path,
    environment: Mapping[str, str],
    budget: compat_backends.ItemBudget,
) -> compat_backends.BackendResult:
    _apply_memory_limit()
    tmp_root = output_root / "tmp"
    tmp_root.mkdir(parents=True, exist_ok=True)
    output_binary = output_root / f"{Path(file_path).stem}_molt"
    metrics_dir = output_root / "metrics" if _diff_measure_rss() else None
    if metrics_dir is not None:
        metrics_dir.mkdir(parents=True, exist_ok=True)
    build_time_path = metrics_dir / "build.time" if metrics_dir is not None else None
    run_time_path = metrics_dir / "run.time" if metrics_dir is not None else None
    build_metrics: dict[str, int] | None = None
    run_metrics: dict[str, int] | None = None

    # Build
    if (
        execution_context is not None
        and execution_context.build_profile != build_profile
    ):
        raise ValueError("backend execution context build profile drifted")
    env = dict(environment)
    layout = _diff_artifact_layout(environment=env)
    env["PYTHONHASHSEED"] = "0"
    # Keep differential builds hermetic to the configured diff roots so host
    # ~/.molt state and inherited shell paths cannot destabilize runs.
    selected = diff_output_layout.selected_for_compat(env, repo_root=_repo_root())
    diff_home = (
        selected / "guest-tmp" / ".molt_home"
        if selected is not None
        else layout.diff_root / ".molt_home"
    )
    diff_home.mkdir(parents=True, exist_ok=True)
    if selected is not None:
        env["MOLT_HOME"] = str(diff_home)
        env["MOLT_BIN"] = str(diff_home / "bin")
    else:
        env.setdefault("MOLT_HOME", str(diff_home))
        env.setdefault("MOLT_BIN", str(diff_home / "bin"))
    env.setdefault(
        "MOLT_BUILD_STATE_DIR",
        str(
            build_state_root(
                project_root=layout.repo_root,
                cargo_target=layout.cargo_target_root,
                environment=env,
            )
        ),
    )
    shared_cache = env.get("MOLT_CACHE")
    if shared_cache:
        Path(shared_cache).mkdir(parents=True, exist_ok=True)
    else:
        cache_root = layout.cache_root
        cache_root.mkdir(parents=True, exist_ok=True)
        env["MOLT_CACHE"] = str(cache_root)
    env["TMPDIR"] = str(tmp_root)
    env["TEMP"] = str(tmp_root)
    env["TMP"] = str(tmp_root)
    env.setdefault(
        "MOLT_DIAGNOSTICS_FILE", str(output_root / "runtime_diagnostics.log")
    )
    # Keep wrappers disabled by default for reproducibility; opt in explicitly
    # when the host wrapper cache is known-good for this environment.
    if not _diff_allow_rustc_wrapper():
        env.pop("RUSTC_WRAPPER", None)
        env.pop("CARGO_BUILD_RUSTC_WRAPPER", None)
        # Force-disable wrapper/caching helpers in this mode, even when parent
        # shells exported throughput-oriented defaults.
        env["SCCACHE_DISABLE"] = "1"
        env["MOLT_USE_SCCACHE"] = "0"
    # Always route through the diff target root (which itself honors
    # MOLT_DIFF_CARGO_TARGET_DIR) instead of inheriting unrelated shell state.
    env["CARGO_TARGET_DIR"] = str(layout.cargo_target_root)
    if execution_context is None:
        env["MOLT_CAPABILITY_TIER"] = EXPLICIT_CAPABILITY_TIER
        env["PYTHONPATH"] = "src"
        env.update(_collect_env_overrides(file_path))
        metadata_error = _apply_metadata_env_overrides(file_path, env)
        if metadata_error is not None:
            _record_rss_metrics(
                file_path,
                build_metrics=None,
                run_metrics=None,
                build_rc=2,
                run_rc=None,
                status="build_invalid_stdlib_profile",
            )
            return compat_backends.BackendResult(
                None, metadata_error, 2, build_failed=True
            )
    if extra_env:
        env.update(extra_env)
    diff_output_layout.enforce_child(env, repo_root=_repo_root())
    if daemon_enabled is None:
        daemon_enabled = _diff_backend_daemon_default()
    env["MOLT_BACKEND_DAEMON"] = "1" if daemon_enabled else "0"
    if _diff_force_no_cache():
        no_cache = True
    if _diff_force_rebuild():
        rebuild = True
    env.setdefault("MOLT_SYS_EXECUTABLE", sys.executable)
    try:
        stdlib_profile = compat_backends.stdlib_profile_from_environment(env)
        if (
            execution_context is not None
            and stdlib_profile != execution_context.stdlib_profile
        ):
            raise ValueError("backend execution context stdlib profile drifted")
    except ValueError as exc:
        _record_rss_metrics(
            file_path,
            build_metrics=None,
            run_metrics=None,
            build_rc=2,
            run_rc=None,
            status="build_invalid_stdlib_profile",
        )
        return compat_backends.BackendResult(None, str(exc), 2, build_failed=True)
    timeout = budget.policy.phase("native", "first_run")
    build_timeout = budget.policy.phase("native", "build")
    rss_limit_kb = _diff_fail_rss_kb()
    build_rc = 0
    batch_requested = _diff_batch_compile_server_enabled()
    batch_request_timeout = _diff_batch_compile_server_request_timeout(build_timeout)
    if batch_requested:
        try:
            batch_result = _run_batch_compile_build(
                env=env,
                file_path=file_path,
                output_root=output_root,
                output_binary=output_binary,
                build_profile=build_profile,
                target_python=(
                    execution_context.target_python
                    if execution_context is not None
                    else None
                ),
                no_cache=no_cache,
                rebuild=rebuild,
                request_timeout=batch_request_timeout,
                budget=budget,
            )
        except Exception as exc:
            message = f"Batch compile server failed: {exc}"
            _record_rss_metrics(
                file_path,
                build_metrics=None,
                run_metrics=None,
                build_rc=127,
                run_rc=None,
                status="build_batch_server_error",
            )
            return compat_backends.BackendResult(None, message, 127, build_failed=True)
        else:
            if (
                batch_result.timed_out
                or batch_result.infrastructure_failure is not None
            ):
                _record_rss_metrics(
                    file_path,
                    build_metrics=None,
                    run_metrics=None,
                    build_rc=batch_result.returncode,
                    run_rc=None,
                    status="build_infrastructure_error"
                    if batch_result.infrastructure_failure is not None
                    else "build_timeout",
                )
                return batch_result
            build_rc = batch_result.returncode
            build_outcome = batch_result

    else:
        build_cmd = [
            _resolve_molt_cli_python(),
            "-m",
            "molt.cli",
            "build",
            file_path,
            "--build-profile",
            build_profile,
            "--respect-pythonpath",
            "--out-dir",
            str(output_root),
            "--output",
            str(output_binary),
        ]
        if execution_context is not None:
            build_cmd.extend(
                ["--python-version", execution_context.target_python.short]
            )
        # Grant standard capabilities for CPython parity in differential tests.
        # Without these, compiled binaries cannot access the filesystem (tempfile,
        # pathlib, os.path), environment variables, or time functions — causing
        # spurious failures unrelated to the tested semantics.
        diff_caps = _diff_capabilities(env)
        if diff_caps:
            build_cmd.extend(["--capabilities", diff_caps])
        if no_cache:
            build_cmd.append("--no-cache")
        if rebuild:
            build_cmd.append("--rebuild")
        if stdlib_profile is not None:
            build_cmd.extend(["--stdlib-profile", stdlib_profile])
        codec = env.get("MOLT_CODEC")
        if codec:
            build_cmd.extend(["--codec", codec])
        build_timeout = budget.limit(build_timeout)
        if build_timeout == 0:
            return budget.exhausted_result(build_failed=True)
        build_res = _run_with_optional_time(
            build_cmd,
            env=env,
            timeout=build_timeout,
            time_path=build_time_path,
        )
        if build_time_path is not None:
            build_metrics = _parse_time_metrics(build_time_path)
        if getattr(build_res, "infrastructure_failure", None) is not None:
            return compat_backends.BackendResult.from_process(
                build_res
            ).as_build_failure(
                detail="build guard infrastructure failed",
                fallback="guard infrastructure failed",
            )
        build_rc = build_res.returncode
        build_outcome = compat_backends.BackendResult.from_process(build_res)
        if build_outcome.timed_out:
            _record_rss_metrics(
                file_path,
                build_metrics=build_metrics,
                run_metrics=None,
                build_rc=build_rc,
                run_rc=None,
                status="build_timeout",
            )
            return build_outcome.as_build_failure(
                detail="build timed out", fallback="build timed out"
            )
    exceeded, detail = _rss_exceeded(build_metrics, rss_limit_kb)
    if exceeded:
        message = f"Build RSS limit exceeded: {detail}"
        _record_rss_metrics(
            file_path,
            build_metrics=build_metrics,
            run_metrics=None,
            build_rc=125,
            run_rc=None,
            status="build_rss_exceeded",
        )
        failure = build_outcome.as_build_failure(
            detail="build RSS limit exceeded", fallback=message
        )
        return replace(
            failure,
            stderr="\n".join(part for part in (failure.stderr, message) if part),
            returncode=125,
            rss_limit_exceeded=True,
        )
    if build_rc != 0:
        _record_rss_metrics(
            file_path,
            build_metrics=build_metrics,
            run_metrics=None,
            build_rc=build_rc,
            run_rc=None,
            status="build_failed",
        )
        return build_outcome.as_build_failure(
            detail=build_outcome.detail, fallback="build failed"
        )

    if budget.expired():
        return budget.exhausted_result(build_failed=build_only)
    preflight_err = _dyld_preflight_error(output_binary, budget=budget)
    if preflight_err is not None:
        _record_rss_metrics(
            file_path,
            build_metrics=build_metrics,
            run_metrics=None,
            build_rc=126,
            run_rc=None,
            status="build_dyld_preflight_failed",
        )
        return compat_backends.BackendResult(
            None, preflight_err, 126, build_failed=True
        )

    if build_only:
        _record_rss_metrics(
            file_path,
            build_metrics=build_metrics,
            run_metrics=None,
            build_rc=build_rc,
            run_rc=None,
            status="build_only_ok",
        )
        return compat_backends.BackendResult("", "", 0)

    # Run
    timeout = budget.limit(timeout)
    if timeout == 0:
        return budget.exhausted_result()
    run_res = _run_with_optional_time(
        [str(output_binary)],
        env=env,
        timeout=timeout,
        time_path=run_time_path,
    )
    if run_time_path is not None:
        run_metrics = _parse_time_metrics(run_time_path)
    outcome = compat_backends.BackendResult.from_process(run_res)
    if outcome.infrastructure_failure is not None:
        return outcome
    if outcome.timed_out:
        _record_rss_metrics(
            file_path,
            build_metrics=build_metrics,
            run_metrics=run_metrics,
            build_rc=build_rc,
            run_rc=outcome.returncode,
            status="run_timeout",
        )
        return outcome
    exceeded, detail = _rss_exceeded(run_metrics, rss_limit_kb)
    if exceeded:
        message = f"Run RSS limit exceeded: {detail}"
        _record_rss_metrics(
            file_path,
            build_metrics=build_metrics,
            run_metrics=run_metrics,
            build_rc=build_rc,
            run_rc=125,
            status="run_rss_exceeded",
        )
        return replace(
            outcome,
            stderr="\n".join(part for part in (outcome.stderr, message) if part),
            returncode=125,
            rss_limit_exceeded=True,
        )
    run_status = "ok" if run_res.returncode == 0 else "run_failed"
    _record_rss_metrics(
        file_path,
        build_metrics=build_metrics,
        run_metrics=run_metrics,
        build_rc=build_rc,
        run_rc=run_res.returncode,
        status=run_status,
    )
    return outcome


def _aggregate_rss_metrics(run_id: str) -> dict[str, object]:
    if not _diff_measure_rss():
        return {}
    summary_path = _diff_root() / "rss_metrics.jsonl"
    if not summary_path.exists():
        return {}
    entries: list[dict[str, object]] = []
    try:
        for line in summary_path.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            try:
                payload = json.loads(line)
            except json.JSONDecodeError:
                continue
            if run_id and payload.get("run_id") != run_id:
                continue
            entries.append(payload)
    except OSError:
        return {}
    if not entries:
        return {}

    def metric_max(key: str, field: str) -> int | None:
        values: list[int] = []
        for item in entries:
            block = item.get(key) or {}
            if isinstance(block, dict):
                value = block.get(field)
                if isinstance(value, int):
                    values.append(value)
        return max(values) if values else None

    max_build_rss = metric_max("build", "max_rss")
    max_run_rss = metric_max("run", "max_rss")
    max_peak = metric_max("run", "peak_footprint")
    return {
        "entries": len(entries),
        "max_build_rss_kb": max_build_rss,
        "max_run_rss_kb": max_run_rss,
        "max_run_peak_footprint_kb": max_peak,
    }


def _aggregate_rss_entries_by_file(
    entries: list[dict[str, object]],
) -> list[dict[str, object]]:
    """Collapse warmup and execution RSS observations per test file.

    An explicit cache warmup and the selected execution can both report metrics.
    Keep their maximum RSS; the caller supplies the authoritative item status.
    """

    by_file: dict[str, dict[str, object]] = {}
    for payload in entries:
        file_path = payload.get("file")
        if not isinstance(file_path, str) or not file_path:
            continue

        current = by_file.get(file_path)
        if current is None:
            current = {
                "file": file_path,
                "status": payload.get("status"),
                "build": {},
                "run": {},
                "_last_ts": payload.get("timestamp", 0.0),
            }
            by_file[file_path] = current

        # Final status should come from the latest record for this file.
        ts = payload.get("timestamp", 0.0)
        prev_ts = current.get("_last_ts", 0.0)
        if isinstance(ts, (int, float)) and isinstance(prev_ts, (int, float)):
            if ts >= prev_ts:
                current["status"] = payload.get("status")
                current["_last_ts"] = ts

        def _merge_block(name: str) -> None:
            src = payload.get(name)
            if not isinstance(src, dict):
                return
            dst = current.get(name)
            if not isinstance(dst, dict):
                dst = {}
                current[name] = dst
            for key in ("max_rss", "peak_footprint"):
                value = src.get(key)
                if not isinstance(value, int):
                    continue
                old = dst.get(key)
                if not isinstance(old, int) or value > old:
                    dst[key] = value

        _merge_block("build")
        _merge_block("run")

    out: list[dict[str, object]] = []
    for payload in by_file.values():
        payload.pop("_last_ts", None)
        out.append(payload)
    return out


def _top_rss_entries(
    run_id: str, limit: int, *, phase: str = "run"
) -> list[dict[str, object]]:
    if not _diff_measure_rss():
        return []
    summary_path = _diff_root() / "rss_metrics.jsonl"
    if not summary_path.exists():
        return []
    entries: list[dict[str, object]] = []
    try:
        for line in summary_path.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            try:
                payload = json.loads(line)
            except json.JSONDecodeError:
                continue
            if run_id and payload.get("run_id") != run_id:
                continue
            entries.append(payload)
    except OSError:
        return []

    def metric(entry: dict[str, object]) -> int:
        block = entry.get(phase) or {}
        if isinstance(block, dict):
            value = block.get("max_rss")
            if isinstance(value, int):
                return value
        return 0

    aggregated = _aggregate_rss_entries_by_file(entries)
    ranked = [entry for entry in aggregated if metric(entry) > 0]
    ranked.sort(key=metric, reverse=True)
    return ranked[: max(0, limit)]


@lru_cache(maxsize=8192)
def _status_lookup_keys(path: str) -> tuple[str, ...]:
    keys: list[str] = []

    def _add(value: str) -> None:
        if value and value not in keys:
            keys.append(value)

    _add(path)
    parsed = Path(path)
    _add(parsed.as_posix())

    with contextlib.suppress(OSError, RuntimeError, ValueError):
        resolved = parsed.resolve()
        _add(str(resolved))
        _add(resolved.as_posix())
        with contextlib.suppress(ValueError):
            rel = resolved.relative_to(_repo_root())
            _add(rel.as_posix())
            _add(str(rel))

    if not parsed.is_absolute():
        with contextlib.suppress(OSError, RuntimeError, ValueError):
            from_cwd = (Path.cwd() / parsed).resolve()
            _add(str(from_cwd))
            _add(from_cwd.as_posix())
            with contextlib.suppress(ValueError):
                rel = from_cwd.relative_to(_repo_root())
                _add(rel.as_posix())
                _add(str(rel))

    return tuple(keys)


def _rss_display_status(
    entry: dict[str, object], status_by_path: dict[str, str] | None
) -> str:
    file_path = entry.get("file")
    if status_by_path and isinstance(file_path, str):
        for key in _status_lookup_keys(file_path):
            resolved = status_by_path.get(key)
            if isinstance(resolved, str) and resolved:
                return resolved
    raw = entry.get("status")
    if isinstance(raw, str):
        normalized = raw.strip().lower()
        if normalized in {"ok", "build_only_ok", "pass"}:
            return "pass"
        if normalized in {"skip", "skipped"}:
            return "skip"
        if normalized == "oom":
            return "oom"
        # RSS event statuses are runtime/build telemetry details. If we cannot
        # resolve a final diff status from status_by_path, present any
        # non-pass/non-skip raw status as fail to avoid misleading artifacts
        # like `run_failed` in otherwise green summaries.
        if normalized:
            return "fail"
    return ""


def _print_rss_top(
    run_id: str, limit: int, *, status_by_path: dict[str, str] | None = None
) -> None:
    if limit <= 0:
        return
    build_entries = _top_rss_entries(run_id, limit, phase="build")
    run_entries = _top_rss_entries(run_id, limit, phase="run")
    if not build_entries and not run_entries:
        return

    def fmt(value: object) -> str:
        return f"{value} KB" if isinstance(value, int) and value > 0 else "-"

    if build_entries:
        print(f"Top {len(build_entries)} RSS offenders (build phase):")
        for entry in build_entries:
            file_path = entry.get("file", "<unknown>")
            status = _rss_display_status(entry, status_by_path)
            run_block = entry.get("run") or {}
            build_block = entry.get("build") or {}
            run_rss = run_block.get("max_rss") if isinstance(run_block, dict) else None
            build_rss = (
                build_block.get("max_rss") if isinstance(build_block, dict) else None
            )
            print(
                f"- {file_path} | build={fmt(build_rss)} run={fmt(run_rss)} status={status}"
            )

    if run_entries:
        print(f"Top {len(run_entries)} RSS offenders (run phase):")
        for entry in run_entries:
            file_path = entry.get("file", "<unknown>")
            status = _rss_display_status(entry, status_by_path)
            run_block = entry.get("run") or {}
            build_block = entry.get("build") or {}
            run_rss = run_block.get("max_rss") if isinstance(run_block, dict) else None
            build_rss = (
                build_block.get("max_rss") if isinstance(build_block, dict) else None
            )
            print(
                f"- {file_path} | run={fmt(run_rss)} build={fmt(build_rss)} status={status}"
            )


# ---------------------------------------------------------------------------
# Multi-backend differential machinery (doc 66 FACT 2)
# ---------------------------------------------------------------------------


def _is_compile_error(err: str) -> bool:
    return any(tag in err for tag in ("SyntaxError", "IndentationError", "TabError"))


_COMPAT_BACKEND_REGISTRY: dict[str, compat_backends.BackendAdapter] | None = None


def _compat_backend_registry() -> dict[str, compat_backends.BackendAdapter]:
    """The backend adapter registry, wiring native to this module's run_molt.

    Built lazily (and cached) so the native adapter binds to the real run_molt
    without an import cycle.
    """
    global _COMPAT_BACKEND_REGISTRY
    if _COMPAT_BACKEND_REGISTRY is None:
        _COMPAT_BACKEND_REGISTRY = compat_backends.build_registry(run_molt)
    return _COMPAT_BACKEND_REGISTRY


def _run_backend_for_diff(
    *,
    backend: str,
    file_path: str,
    context: compat_backends.BackendExecutionContext,
) -> compat_backends.BackendResult | compat_backends.BackendAvailability:
    """Keep the adapter's complete outcome, including resource failure facts."""
    context = context.for_backend(backend)
    if context.budget.expired():
        return context.budget.exhausted_result(build_failed=True)
    try:
        adapter = _compat_backend_registry().get(backend)
        if adapter is None:
            raise ValueError(f"unknown differential backend {backend!r}")
        availability = adapter.availability(context.budget)
        context.budget.check(build_failed=True)
        if not availability.available:
            print(f"[UNCALIBRATED] {file_path} ({backend}: {availability.reason})")
            return availability
        result = adapter.build_and_run(file_path, context=context)
    except compat_backends.ItemBudgetExpired as error:
        return error.result()
    # Apply the fault-injection seam once, preserving all execution metadata.
    return compat_backends._apply_fault_injection(backend, result)


def _cross_backend_divergence(
    per_backend: dict[str, compat_backends.BackendResult],
    *,
    stdout_mode: str,
    stderr_mode: str,
) -> str | None:
    """Return a detail string if any two backends disagree, else None.

    The cross-backend divergence sub-oracle (doc 66 FACT 2): every backend that
    produced output must agree with every other under the SAME comparison law
    used against CPython. Build-failed backends are excluded from this check
    (their CPython verdict already covers them); a divergence among the rest is a
    molt-internal backend fork and is always a FAIL.
    """
    ran = {
        name: outcome
        for name, outcome in per_backend.items()
        if outcome.stdout is not None
        and not outcome.timed_out
        and outcome.infrastructure_failure is None
    }
    if len(ran) < 2:
        return None
    names = list(ran)
    reference_name = names[0]
    reference = ran[reference_name]
    mismatches: list[str] = []
    for name in names[1:]:
        candidate = ran[name]
        verdict = compat_comparison.compare_outputs(
            compat_comparison.Outputs(
                reference.stdout, reference.child_stderr or "", reference.returncode
            ),
            compat_comparison.Outputs(
                candidate.stdout, candidate.child_stderr or "", candidate.returncode
            ),
            mode=stdout_mode,
            stderr_mode=stderr_mode,
        )
        if not verdict.equal:
            mismatches.append(
                f"{reference_name} != {name} ({verdict.detail}); "
                f"{reference_name} stdout={reference.stdout!r:.120} rc={reference.returncode}; "
                f"{name} stdout={candidate.stdout!r:.120} rc={candidate.returncode}"
            )
    if not mismatches:
        return None
    return " | ".join(mismatches)


def _emit_backend_pass(file_path: str, backend: str, *, single: bool) -> None:
    if single:
        print(f"[PASS] {file_path}")
    else:
        print(f"[PASS] {file_path} ({backend})")


def _emit_backend_fail(
    file_path: str,
    backend: str,
    cp_out: str,
    cp_err: str,
    cp_ret: int,
    outcome: compat_backends.BackendResult,
    detail: str,
) -> None:
    if outcome.stdout is None:
        print(f"[FAIL] Molt failed to build {file_path} ({backend})")
        print(outcome.stderr)
        return
    print(f"[FAIL] {file_path} ({backend}) mismatch{(': ' + detail) if detail else ''}")
    print(f"  CPython stdout: {cp_out!r}")
    print(f"  Molt    stdout: {outcome.stdout!r}")
    print(f"  CPython return: {cp_ret} stderr: {cp_err!r}")
    child_stderr = outcome.child_stderr or ""
    print(f"  Molt    return: {outcome.returncode} stderr: {child_stderr!r}")
    if outcome.stderr != child_stderr:
        print(f"  Molt diagnostics (combined stderr): {outcome.stderr!r}")


def _record_backend_result(
    record: dict[str, object],
    file_path: str,
    backend: str,
    raw_status: str,
    expect_molt_fail: bool,
    outcome: compat_backends.BackendResult | compat_backends.BackendAvailability,
    context: compat_backends.BackendExecutionContext,
) -> None:
    """Append a per-backend honesty row so the (test x backend) matrix is fed.

    The aggregate `record` keeps the overall test status (consumed by the
    existing single-row honesty sink); this emits an ADDITIONAL row tagged with
    the backend + cpython_version so tools/check_suite_honesty.py can dimension
    the failure by backend exactly as it dimensions native today.
    """
    backend_rows = record.setdefault("backend_rows", [])
    assert isinstance(backend_rows, list)
    if isinstance(outcome, compat_backends.BackendResult):
        facts = {
            "returncode": outcome.returncode,
            "child_returncode": outcome.child_returncode,
            "infrastructure_failure": memory_guard.infrastructure_failure_payload(
                outcome.infrastructure_failure
            ),
            "build_failed": outcome.build_failed,
            "timed_out": outcome.timed_out,
            "resource_failure": outcome.resource_failure,
            "rss_limit_exceeded": outcome.rss_limit_exceeded,
            "guard_signal": outcome.guard_signal,
            "detail": outcome.detail,
        }
        stdout, stderr = outcome.stdout or "", outcome.child_stderr or ""
    else:
        facts = {
            "returncode": None,
            "child_returncode": None,
            "infrastructure_failure": None,
            "build_failed": False,
            "timed_out": False,
            "resource_failure": None,
            "rss_limit_exceeded": False,
            "guard_signal": None,
            "detail": outcome.reason,
        }
        stdout, stderr = "", outcome.reason
    backend_rows.append(
        {
            "record_type": "backend",
            "file": test_policy.normalize_repo_relative(file_path),
            "backend": backend,
            "compiler_target_python": context.target_python.short,
            "build_profile": context.build_profile,
            "stdlib_profile": context.stdlib_profile,
            "raw_status": raw_status,
            "expect_molt_fail": expect_molt_fail,
            **facts,
            "stdout_sha256": hashlib.sha256(
                stdout.encode("utf-8", errors="surrogateescape")
            ).hexdigest(),
            "stderr_sha256": hashlib.sha256(
                stderr.encode("utf-8", errors="surrogateescape")
            ).hexdigest(),
        }
    )


def diff_test(
    file_path,
    python_exe: PythonCommand = sys.executable,
    build_profile: str = "dev",
    targets: tuple[str, ...] = ("native",),
    target_python: str | TargetPythonVersion | None = None,
    item_budget: compat_backends.ItemBudget | None = None,
) -> dict[str, object]:
    """Run one differential test across `targets` and return its outcome record.

    The resolved status applies the expected-failure (xfail/xpass) overlay so the
    nightly suite stays green for by-design exclusions. The SUITE-HONESTY ratchet
    (tools/check_suite_honesty.py, task #46) needs the RAW status — whether Molt
    actually matched CPython, independent of any expectation overlay — so a
    tracked-but-failing test can never read as green. To avoid a second runner
    (which would drift from these exact comparison semantics), the raw outcome is
    surfaced here, at the single chokepoint that computes it, and appended to the
    optional results sink keyed by MOLT_DIFF_RESULTS_JSONL. The sink is off by
    default and never alters the returned outcome. The same record carries the
    typed guard signal to cohort admission; process exit codes cannot establish
    whether the operator interrupted the guard or the child exited itself.

    MULTI-BACKEND (doc 66): `targets` selects which backend(s) to validate
    (native / wasm / llvm / luau). Each backend is compared against CPython under
    the ONE comparison law (tools/compat/comparison.py); when two or more
    backends ran, they are also compared against EACH OTHER (the cross-backend
    divergence sub-oracle). The test FAILS if any backend disagrees with CPython
    OR any two backends disagree with each other — so a backend-specific
    divergence that was previously invisible (single-backend native) is caught.
    """
    if item_budget is None:
        item_budget = compat_backends.ItemBudget.start(
            compat_backends.TimeoutPolicy.from_environment(os.environ), targets
        )
    if not item_budget.expired() and os.environ.get(diff_output_layout.ROOT_ENV):
        if diff_output_layout.IDENTITY_ENV not in os.environ:
            diff_output_layout.admit(
                os.environ,
                repo_root=_repo_root(),
                custody_root=diff_output_layout.diff_custody_root(
                    os.environ, repo_root=_repo_root()
                ),
            )
        else:
            diff_output_layout.enforce_child(os.environ, repo_root=_repo_root())
    # Mutable record the inner finalizer fills in; emitted once on every exit
    # path (skip / oom / pass / fail). raw_status is authoritative for honesty.
    started = time.monotonic()
    record: dict[str, object] = {
        "record_type": "test",
        "file": test_policy.normalize_repo_relative(file_path),
        "raw_status": None,
        "resolved_status": None,
        "reason_tag": None,
        "expect_molt_fail": False,
        "guard_signal": None,
    }

    def _impl() -> None:
        nonlocal python_exe
        item_budget.check()
        python_exe = _resolve_python_command(python_exe, budget=item_budget)
        meta = test_policy.parse_metadata(file_path)
        expect_molt_fail = meta.expect_molt_fail
        expect_fail_reason = meta.expected_failure_reason
        record["expect_molt_fail"] = expect_molt_fail
        record["expected_failure_reason"] = expect_fail_reason
        python_version = _python_exe_version(python_exe, budget=item_budget)
        execution_context = _backend_execution_context(
            file_path,
            python_exe,
            build_profile,
            target_python,
            metadata=meta,
            budget=item_budget,
        )
        record["compiler_target_python"] = execution_context.target_python.short
        host_tags = test_policy.current_platform_tags()
        architecture = test_policy.current_architecture()
        exclusion_by_backend = {
            backend: test_policy.exclusion_reason(
                meta,
                python_version=python_version,
                platform_tags=host_tags,
                architecture=architecture,
                backend=backend,
            )
            for backend in targets
        }
        eligible_targets = tuple(
            backend for backend in targets if exclusion_by_backend[backend] is None
        )
        if not eligible_targets:
            reasons = sorted(
                {
                    reason
                    for reason in exclusion_by_backend.values()
                    if reason is not None
                }
            )
            reason = "; ".join(reasons) or "not applicable to requested backends"
            note = f" ({reason})" if reason else ""
            print(f"[SKIP] {file_path}{note}")
            # skip/oom never reach _finalize_status; record raw==resolved so the
            # honesty sink sees every test, not just the compared ones.
            record["raw_status"] = "skip"
            record["resolved_status"] = "skip"
            record["exclusion_reason"] = reason
            return

        stdout_mode = meta.stdout_mode
        stderr_mode = meta.stderr_mode

        print(f"Testing {file_path} against {_python_command_display(python_exe)}...")
        cpython = run_cpython(file_path, python_exe, budget=item_budget)
        cp_out, cp_err, cp_ret = (
            cpython.stdout or "",
            cpython.child_stderr or "",
            cpython.returncode,
        )
        record["cpython_returncode"] = cp_ret
        record["cpython_child_returncode"] = cpython.child_returncode
        record["cpython_resource_failure"] = cpython.resource_failure
        record["cpython_rss_limit_exceeded"] = cpython.rss_limit_exceeded
        record["cpython_guard_signal"] = cpython.guard_signal
        record["cpython_infrastructure_failure"] = (
            memory_guard.infrastructure_failure_payload(cpython.infrastructure_failure)
        )

        def _finalize_status(raw_status: str) -> None:
            resolved_status, reason_tag = test_policy.resolve_expected_failure_status(
                expect_molt_fail=expect_molt_fail,
                raw_status=raw_status,
                cpython_returncode=cp_ret,
            )
            record["raw_status"] = raw_status
            record["resolved_status"] = resolved_status
            record["reason_tag"] = reason_tag
            if reason_tag == "xfail":
                reason_text = expect_fail_reason or "expected dynamic-semantics gap"
                print(f"[XFAIL] {file_path} ({reason_text})")
            elif reason_tag == "xpass":
                reason_text = expect_fail_reason or "expected dynamic-semantics gap"
                print(f"[XPASS] {file_path} ({reason_text})")

        if cpython.guard_signal is not None:
            record["guard_signal"] = cpython.guard_signal
            record["raw_status"] = record["resolved_status"] = "uncalibrated"
            record["reason_tag"] = "guard_interrupted"
            print(f"[UNCALIBRATED] {file_path} (cpython: operator interrupted)")
            print(cpython.stderr)
            return
        if cpython.infrastructure_failure is not None:
            print(f"[UNCALIBRATED] {file_path} (cpython infrastructure failed)")
            print(cpython.stderr)
            print(cpython.detail)
            record["raw_status"] = record["resolved_status"] = "uncalibrated"
            record["reason_tag"] = "infrastructure_error"
            return
        if cpython.timed_out:
            print(f"[FAIL] {file_path} (cpython timed out)")
            print(cpython.stderr)
            record["raw_status"] = "fail"
            record["resolved_status"] = "fail"
            record["reason_tag"] = "timeout"
            return
        if cpython.resource_failure is not None:
            print(f"[OOM] {file_path} (cpython)")
            print(cpython.stderr)
            record["raw_status"] = "oom"
            record["resolved_status"] = "oom"
            return
        if cp_ret != 0 and (
            "msgpack is required for parse_msgpack fallback" in cp_err
            or "cbor2 is required for parse_cbor fallback" in cp_err
        ):
            print(f"[SKIP] {file_path} (missing msgpack/cbor2 in CPython env)")
            record["raw_status"] = "skip"
            record["resolved_status"] = "skip"
            return
        record["cpython_stdout_sha256"] = hashlib.sha256(
            cp_out.encode("utf-8", errors="surrogateescape")
        ).hexdigest()
        record["cpython_stderr_sha256"] = hashlib.sha256(
            cp_err.encode("utf-8", errors="surrogateescape")
        ).hexdigest()
        record["comparison_law"] = compat_comparison.COMPARISON_LAW_VERSION

        # The CPython oracle, the meta gating, the comparison law, and the
        # cross-backend divergence check are all backend-INDEPENDENT (doc 66
        # FACT 2). Only HOW a backend produces (stdout, stderr, rc) differs, and
        # that lives in the selected adapter's single guarded build+run attempt.
        # --- Per-backend verdict vs CPython (the single comparison law) -------
        def _verdict(outcome: compat_backends.BackendResult) -> tuple[bool, str]:
            """(ok, detail) for one backend vs CPython under the one law."""
            if outcome.timed_out:
                return False, "backend timed out"
            if outcome.stdout is None:
                # Build failure. Mirror the historical parity case: a CPython
                # compile error that Molt also rejects at build time is a PASS.
                if (
                    cp_ret != 0
                    and _is_compile_error(cp_err)
                    and _is_compile_error(outcome.stderr)
                ):
                    return True, ""
                return False, "molt failed to build"
            verdict = compat_comparison.compare_outputs(
                compat_comparison.Outputs(cp_out, cp_err, cp_ret),
                compat_comparison.Outputs(
                    outcome.stdout, outcome.child_stderr or "", outcome.returncode
                ),
                mode=stdout_mode,
                stderr_mode=stderr_mode,
            )
            return verdict.equal, verdict.detail

        any_cpython_fail = False
        any_timeout = False
        any_unavailable = False
        any_infrastructure_failure = False
        per_backend: dict[str, compat_backends.BackendResult] = {}
        for backend in eligible_targets:
            outcome = _run_backend_for_diff(
                backend=backend,
                file_path=file_path,
                context=execution_context,
            )
            if isinstance(outcome, compat_backends.BackendAvailability):
                any_unavailable = True
                _record_backend_result(
                    record,
                    file_path,
                    backend,
                    "uncalibrated",
                    expect_molt_fail,
                    outcome,
                    execution_context,
                )
                continue
            if outcome.guard_signal is not None:
                _record_backend_result(
                    record,
                    file_path,
                    backend,
                    "uncalibrated",
                    expect_molt_fail,
                    outcome,
                    execution_context,
                )
                record["guard_signal"] = outcome.guard_signal
                record["raw_status"] = record["resolved_status"] = "uncalibrated"
                record["reason_tag"] = "guard_interrupted"
                print(f"[UNCALIBRATED] {file_path} ({backend}: operator interrupted)")
                print(outcome.stderr)
                return
            if outcome.infrastructure_failure is not None:
                any_infrastructure_failure = True
                _record_backend_result(
                    record,
                    file_path,
                    backend,
                    "uncalibrated",
                    expect_molt_fail,
                    outcome,
                    execution_context,
                )
                print(f"[UNCALIBRATED] {file_path} ({backend}: infrastructure failed)")
                print(outcome.stderr)
                print(outcome.detail)
                continue
            # Record and diagnose each completed backend immediately. A later
            # resource failure must never discard already observed results.
            if outcome.resource_failure is not None:
                _record_backend_result(
                    record,
                    file_path,
                    backend,
                    "oom",
                    expect_molt_fail,
                    outcome,
                    execution_context,
                )
                print(f"[OOM] {file_path} ({backend}) rc={outcome.returncode}")
                print(outcome.stderr)
                record["raw_status"] = "oom"
                record["resolved_status"] = "oom"
                return
            per_backend[backend] = outcome
            any_timeout |= outcome.timed_out
            ok, detail = _verdict(outcome)
            if ok:
                _emit_backend_pass(file_path, backend, single=len(targets) == 1)
            else:
                any_cpython_fail = True
                _emit_backend_fail(
                    file_path, backend, cp_out, cp_err, cp_ret, outcome, detail
                )
            # Per-backend honesty row so the matrix join (test x backend) is fed
            # for every backend that ran, not just the aggregate.
            _record_backend_result(
                record,
                file_path,
                backend,
                "pass" if ok else "fail",
                expect_molt_fail,
                outcome,
                execution_context,
            )

        # Infrastructure failures are not expected language divergences. Resolve
        # them before any semantic/xfail overlay, including cross-backend checks.
        if any_infrastructure_failure:
            record["raw_status"] = record["resolved_status"] = "uncalibrated"
            record["reason_tag"] = "infrastructure_error"
            return
        if any_timeout:
            record["raw_status"] = "fail"
            record["resolved_status"] = "fail"
            record["reason_tag"] = "timeout"
            return
        if any_unavailable:
            record["raw_status"] = "uncalibrated"
            record["resolved_status"] = "uncalibrated"
            return

        # --- Cross-backend divergence sub-oracle (doc 66 FACT 2) --------------
        # When >=2 backends produced output, they must agree with EACH OTHER
        # under the same law. A divergence here is a molt-internal backend fork
        # that requires zero CPython runs to localize, and is the direct witness
        # of a "semantic authority is shared" violation. It is a FAIL even if
        # (pathologically) every backend also disagreed with CPython the same
        # way — backends disagreeing among themselves is never acceptable.
        divergence_detail = _cross_backend_divergence(
            per_backend, stdout_mode=stdout_mode, stderr_mode=stderr_mode
        )
        if divergence_detail is not None:
            print(f"[FAIL] {file_path} CROSS-BACKEND DIVERGENCE")
            print(f"  {divergence_detail}")
            return _finalize_status("fail")

        if any_cpython_fail:
            return _finalize_status("fail")
        return _finalize_status("pass")

    try:
        try:
            _impl()
        except compat_backends.ItemBudgetExpired as error:
            print(f"[FAIL] {file_path} ({error})")
            record["raw_status"] = record["resolved_status"] = "fail"
            record["reason_tag"] = "timeout"
        return record
    finally:
        record["duration_s"] = round(time.monotonic() - started, 6)
        _record_diff_result(record)


def _parallel_diff_results(
    executor: concurrent.futures.Executor,
    test_files: Sequence[Path],
    python_exe: PythonCommand,
    build_profile: str,
    targets: tuple[str, ...],
    target_python: TargetPythonVersion,
    *,
    jobs: int,
    fail_fast: bool,
    trip_outcome: Callable[[], compat_backends.BackendResult | None] | None = None,
    new_budget: Callable[[], compat_backends.ItemBudget],
    admits: Callable[[], bool] = lambda: True,
) -> Iterator[dict[str, object]]:
    """Bound admission, stop replenishing on failure, and retain running results."""
    trip_outcome = _memory_guard_trip_outcome if trip_outcome is None else trip_outcome
    remaining = iter(test_files)
    pending: dict[concurrent.futures.Future, str] = {}
    stopped = False
    errors: list[Exception] = []
    while True:
        stopped |= trip_outcome() is not None
        while not stopped and len(pending) < jobs:
            if trip_outcome() is not None or not admits():
                stopped = True
                break
            file_path = next(remaining, None)
            if file_path is None:
                break
            future = executor.submit(
                _diff_worker,
                str(file_path),
                python_exe,
                build_profile,
                targets,
                target_python,
                new_budget(),
            )
            pending[future] = str(file_path)
        if not pending:
            break
        done, _ = concurrent.futures.wait(
            pending, return_when=concurrent.futures.FIRST_COMPLETED
        )
        for future in sorted(done, key=pending.__getitem__):
            path = pending.pop(future)
            if future.cancelled():
                continue
            try:
                result = future.result()
            except Exception as exc:
                exc.add_note(f"differential worker: {path}")
                errors.append(exc)
                stopped = True
                continue
            stopped |= result["guard_signal"] is not None
            stopped |= fail_fast and result["status"] in _FAILURE_STATUSES
            yield result
        stopped |= trip_outcome() is not None
        if stopped:
            # Cancelling a future never kills a running child; their results
            # drain above through the normal receipt/log path. An operator
            # signal also ends the running commands through the suite stop
            # (compat_backends.suite_stop_signal), so that drain is prompt.
            for future in list(pending):
                if future.cancel():
                    del pending[future]
    if errors:
        raise ExceptionGroup("differential workers failed", errors)


def run_diff(
    target: Path | Sequence[Path],
    python_exe: PythonCommand,
    build_profile: str = "dev",
    *,
    targets: tuple[str, ...] = ("native",),
    target_python: str | None = None,
    jobs: int | None = None,
    log_dir: Path | None = None,
    log_file: Path | None = None,
    log_aggregate: Path | None = None,
    live: bool = False,
    fail_fast: bool = False,
    failures_output: Path | None = None,
    warm_cache: bool = False,
    deadline_s: float | None = None,
    oracle_version: str | None = None,
) -> dict:
    admission = _AdmissionDeadline.for_run(deadline_s, targets)
    diff_output_layout.admit(
        os.environ,
        repo_root=_repo_root(),
        custody_root=diff_output_layout.diff_custody_root(
            os.environ, repo_root=_repo_root()
        ),
    )
    preflight_timed_out = False
    preflight_budget = compat_backends.ItemBudget(admission.policy, admission.deadline)
    try:
        try:
            python_exe = (
                _resolve_diff_python_command(oracle_version, budget=preflight_budget)
                if oracle_version is not None
                else _resolve_python_command(python_exe, budget=preflight_budget)
            )
        finally:
            preflight_budget.check()
        compiler_target_python = _resolve_molt_target_python(
            python_exe,
            target_python,
            budget=preflight_budget,
        )
    except compat_backends.ItemBudgetExpired:
        # Preserve the ordinary all-selected/not-run summary even when startup
        # consumed the suite's remaining time before the first item dispatch.
        compiler_target_python = None
        preflight_timed_out = True
    if not preflight_timed_out:
        _ensure_diff_run_lock()
        _prune_orphan_diff_workers()
        _prune_orphan_build_helpers()
        _prune_backend_daemons()
        _prune_stale_build_locks()
    results: list[tuple[str, str]] = []
    duration_by_path: dict[str, float] = {}
    selected_targets = (target,) if isinstance(target, Path) else tuple(target)
    test_files = list(
        test_policy.collect_test_files(selected_targets, pattern=_diff_glob())
    )
    if jobs is None:
        jobs = _default_jobs() if len(test_files) > 1 else 1
    run_id = _diff_run_id()
    os.environ["MOLT_DIFF_RUN_ID"] = run_id
    guard_config = _diff_memory_guard_config()
    _prepare_memory_guard_run(guard_config)
    jobs = _constrain_jobs_for_memory_guard(jobs, config=guard_config)
    suite_guard_context = harness_memory_guard.HarnessExecutionContext.from_env(
        "MOLT_DIFF",
        os.environ,
        repo_root=Path(_repo_root()),
        artifact_root=_diff_root(),
        limits=_diff_memory_guard_limits(os.environ),
    )
    suite_sentinel = suite_guard_context.start_repo_sentinel(
        label="molt_diff_suite",
        drain_on_exit=True,
        drain_grace_sec=0.35,
        drain_until_clean_sec=0.5,
        drain_max_runtime_sec=5.0,
        on_scan=_record_memory_guard_sentinel_sample,
        on_violation=_record_memory_guard_sentinel_violation,
    )
    sentinel_env_key = harness_memory_guard.repo_sentinel_active_env_key("MOLT_DIFF")
    sentinel_env_previous = os.environ.get(sentinel_env_key)
    sentinel_closed = False
    guard_signal: int | None = None
    signal_by_path: dict[str, int] = {}
    previous_handlers: dict[int, object] = {}
    # Stopping the suite also stops the commands already running, here and in
    # every worker, so they return interrupted receipts at once and the suite
    # writes its summary instead of waiting out their budgets (HF-232).
    worker_context = compiler_process.worker_context()
    suite_stop = worker_context.RawValue("i", 0)
    compat_backends.bind_suite_stop(suite_stop)

    def defer_suite_signal(signum: int, _frame: object) -> None:
        nonlocal guard_signal
        if guard_signal is None:
            guard_signal = signum
            suite_stop.value = signum

    def suite_trip_outcome() -> compat_backends.BackendResult | None:
        trip = _memory_guard_trip_outcome(suite_sentinel)
        if trip is not None:
            return trip
        if guard_signal is not None:
            return compat_backends.BackendResult(
                None,
                f"Operator interrupted cohort with {signal.Signals(guard_signal).name}; "
                "new case admission stopped and running cases drained.\n",
                128 + guard_signal,
                guard_signal=guard_signal,
            )
        return None

    def close_suite_sentinel() -> None:
        nonlocal sentinel_closed
        if sentinel_closed:
            return
        sentinel_closed = True
        try:
            if suite_sentinel is not None:
                suite_sentinel.__exit__(None, None, None)
        finally:
            atexit.unregister(close_suite_sentinel)
            if sentinel_env_previous is None:
                os.environ.pop(sentinel_env_key, None)
            else:
                os.environ[sentinel_env_key] = sentinel_env_previous

    try:
        # A command guard temporarily owns these handlers while closing its
        # children. Between commands and while waiting for parallel cases, defer
        # the suite's stop until already-admitted cases have returned receipts.
        if threading.current_thread() is threading.main_thread():
            for signum in (
                signal.SIGINT,
                signal.SIGTERM,
                getattr(signal, "SIGHUP", None),
            ):
                if signum is not None:
                    previous_handlers[signum] = signal.getsignal(signum)
                    signal.signal(signum, defer_suite_signal)
        if suite_sentinel is not None:
            os.environ[sentinel_env_key] = "1"
            atexit.register(close_suite_sentinel)
        os.environ.setdefault("CARGO_TARGET_DIR", str(_diff_cargo_target_root()))
        test_files = _order_test_files(test_files, jobs)
        if warm_cache:
            shared_cache = os.environ.get("MOLT_CACHE")
            if not shared_cache:
                shared_cache = str(_diff_cache_root())
                os.environ["MOLT_CACHE"] = shared_cache
            for file_path in test_files:
                if suite_trip_outcome() is not None or not admission.admits():
                    break
                try:
                    context = _backend_execution_context(
                        str(file_path),
                        python_exe,
                        build_profile,
                        compiler_target_python,
                        budget=admission.new_budget(build_only=True),
                    )
                    build_result = run_molt_build_only(
                        str(file_path),
                        build_profile,
                        execution_context=context,
                    )
                except compat_backends.ItemBudgetExpired as error:
                    build_result = error.result()
                if build_result.guard_signal is not None:
                    defer_suite_signal(build_result.guard_signal, None)
                    print(build_result.stderr)
                if build_result.infrastructure_failure is not None:
                    raise RuntimeError(
                        f"warm-cache infrastructure failed for {file_path}: "
                        + "; ".join(build_result.infrastructure_failure.details)
                    )
                if build_result.guard_signal is not None:
                    break
                if build_result.returncode != 0:
                    print(
                        f"[WARM-CACHE FAIL] {file_path}: {build_result.stderr.strip()}"
                    )
        if jobs <= 1:
            with _open_log_file(log_file) as log_handle:
                with _open_log_file(log_aggregate) as aggregate_handle:
                    for file_path in test_files:
                        if suite_trip_outcome() is not None or not admission.admits():
                            break
                        _emit_line(f"[RUN] {file_path}", log_handle, echo=True)
                        payload = _diff_run_single(
                            str(file_path),
                            python_exe,
                            build_profile,
                            targets,
                            compiler_target_python,
                            admission.new_budget(),
                        )
                        if payload["guard_signal"] is not None:
                            defer_suite_signal(payload["guard_signal"], None)
                            signal_by_path[payload["path"]] = payload["guard_signal"]
                        path = payload["path"]
                        status = payload["status"]
                        assert isinstance(path, str) and isinstance(status, str)
                        duration_by_path[path] = float(payload["duration_s"])
                        results.append((path, status))
                        if log_handle is not None:
                            _emit_line(
                                f"[{status.upper()}] {path}",
                                log_handle,
                                echo=False,
                            )
                        if aggregate_handle is not None and (
                            status != "pass" or _diff_log_passes()
                        ):
                            _append_aggregate_log(
                                aggregate_handle,
                                path,
                                status,
                                payload["stdout"],
                                payload["stderr"],
                            )
                        if fail_fast and status in _FAILURE_STATUSES:
                            break
                        if suite_trip_outcome() is not None:
                            break
        else:
            if log_dir is not None:
                try:
                    log_dir.mkdir(parents=True, exist_ok=True)
                except OSError as exc:
                    print(f"Warning: failed to create log dir {log_dir}: {exc}")
                    log_dir = None
            if not live:
                live = True
            prune_every = _diff_prune_every()
            completed = 0
            with _open_log_file(log_file) as log_handle:
                with _open_log_file(log_aggregate) as aggregate_handle:
                    executor_kwargs: dict[str, int] = {"max_workers": jobs}
                    max_tasks_per_child = _diff_max_tasks_per_child()
                    if max_tasks_per_child is not None:
                        executor_kwargs["max_tasks_per_child"] = max_tasks_per_child
                    executor_params = {
                        "mp_context": worker_context,
                        "initializer": _install_worker_state,
                        "initargs": (suite_stop,),
                    }
                    try:
                        executor_ctx = concurrent.futures.ProcessPoolExecutor(
                            **executor_kwargs, **executor_params
                        )
                    except TypeError:
                        executor_kwargs.pop("max_tasks_per_child", None)
                        executor_ctx = concurrent.futures.ProcessPoolExecutor(
                            **executor_kwargs, **executor_params
                        )
                    with executor_ctx as executor:
                        for result in _parallel_diff_results(
                            executor,
                            test_files,
                            python_exe,
                            build_profile,
                            targets,
                            compiler_target_python,
                            jobs=jobs,
                            fail_fast=fail_fast,
                            trip_outcome=suite_trip_outcome,
                            admits=admission.admits,
                            new_budget=admission.new_budget,
                        ):
                            if result["guard_signal"] is not None:
                                defer_suite_signal(result["guard_signal"], None)
                                signal_by_path[result["path"]] = result["guard_signal"]
                            path = result["path"]
                            status = result["status"]
                            assert isinstance(path, str) and isinstance(status, str)
                            duration_by_path[path] = float(result["duration_s"])
                            completed += 1
                            results.append((path, status))
                            log_path = None
                            if log_dir is not None:
                                persist_log = status != "pass" or _diff_log_passes()
                                candidate_log_path = _log_path_for_test(log_dir, path)
                                if persist_log:
                                    log_path = _write_test_log(
                                        log_dir,
                                        path,
                                        result["stdout"],
                                        result["stderr"],
                                    )
                                else:
                                    with contextlib.suppress(OSError):
                                        candidate_log_path.unlink()
                            _emit_line(
                                f"[{status.upper()}] {path}",
                                log_handle,
                                echo=live,
                            )
                            if status == "fail" and log_path is not None:
                                _emit_line(f"  log: {log_path}", log_handle, echo=live)
                            if aggregate_handle is not None and (
                                status != "pass" or _diff_log_passes()
                            ):
                                _append_aggregate_log(
                                    aggregate_handle,
                                    path,
                                    status,
                                    result["stdout"],
                                    result["stderr"],
                                )
                            # Workers capture diagnostics; unlike the serial Tee,
                            # they cannot print them live. Publish each failed
                            # completion unless a detail sink already owns it.
                            if (
                                status in _FAILURE_STATUSES
                                and log_path is None
                                and aggregate_handle is None
                            ):
                                if result["stdout"]:
                                    print(result["stdout"], end="", flush=True)
                                if result["stderr"]:
                                    print(
                                        result["stderr"],
                                        end="",
                                        file=sys.stderr,
                                        flush=True,
                                    )
                            if prune_every > 0 and completed % prune_every == 0:
                                _prune_orphan_diff_workers()
                                _prune_orphan_build_helpers()
                                _prune_backend_daemons()
        _prune_orphan_diff_workers()
        _prune_orphan_build_helpers()
        _prune_backend_daemons()
        guard_trip_outcome = suite_trip_outcome()
        guard_trip_message = (
            None
            if guard_trip_outcome is None
            else guard_trip_outcome.stderr.rstrip("\n")
        )
        status_by_path = {path: status for path, status in results}
        guard_trip_outcome = suite_trip_outcome()
        guard_trip_message = (
            None
            if guard_trip_outcome is None
            else guard_trip_outcome.stderr.rstrip("\n")
        )
        discovered = len(status_by_path)
        failed_files = [
            path
            for path, status in status_by_path.items()
            if status in _FAILURE_STATUSES
        ]
        skipped_files = [
            path for path, status in status_by_path.items() if status == "skip"
        ]
        # A deadline, fail-fast or a guard trip can stop admission; name every
        # selected test that never started instead of dropping it.
        not_run_files = [
            str(path) for path in test_files if str(path) not in status_by_path
        ]
        not_run_cause = "deadline" if not admission.admits() else "admission stopped"
        if not_run_files:
            print(f"[NOT RUN] {len(not_run_files)} test(s): {not_run_cause}")
        failed = len(failed_files)
        passed = len([None for status in status_by_path.values() if status == "pass"])
        skipped = len(skipped_files)
        oom = len([None for status in status_by_path.values() if status == "oom"])
        guard_infrastructure_failures = 0
        if guard_trip_outcome is not None and (
            guard_trip_outcome.guard_signal is None or not signal_by_path
        ):
            failed_files.append(
                "<operator_signal>"
                if guard_trip_outcome.guard_signal is not None
                else "<memory_guard>"
            )
            failed += 1
            oom += int(guard_trip_outcome.resource_failure is not None)
            guard_infrastructure_failures += int(
                guard_trip_outcome.infrastructure_failure is not None
            )
        total = passed + failed
        try:
            limit = int(os.environ.get("MOLT_DIFF_RSS_TOP", "5"))
        except ValueError:
            limit = 5
        rss_top_run = [
            {
                "file": entry.get("file"),
                "status": _rss_display_status(entry, status_by_path),
                "run_max_rss_kb": (entry.get("run") or {}).get("max_rss")
                if isinstance(entry.get("run"), dict)
                else None,
                "build_max_rss_kb": (entry.get("build") or {}).get("max_rss")
                if isinstance(entry.get("build"), dict)
                else None,
            }
            for entry in _top_rss_entries(
                run_id, limit if _diff_measure_rss() else 0, phase="run"
            )
        ]
        rss_top_build = [
            {
                "file": entry.get("file"),
                "status": _rss_display_status(entry, status_by_path),
                "run_max_rss_kb": (entry.get("run") or {}).get("max_rss")
                if isinstance(entry.get("run"), dict)
                else None,
                "build_max_rss_kb": (entry.get("build") or {}).get("max_rss")
                if isinstance(entry.get("build"), dict)
                else None,
            }
            for entry in _top_rss_entries(
                run_id, limit if _diff_measure_rss() else 0, phase="build"
            )
        ]
        summary = {
            "discovered": discovered,
            "total": total,
            "passed": passed,
            "failed": failed,
            "oom": oom,
            "guard_infrastructure_failures": guard_infrastructure_failures,
            "guard_signal": guard_signal,
            "skipped": skipped,
            "not_run": len(not_run_files),
            "failed_files": failed_files,
            "skipped_files": skipped_files,
            "item_results": [
                {
                    "path": test_policy.normalize_repo_relative(path),
                    "status": status,
                    "duration_s": duration_by_path[path],
                    **(
                        {"guard_signal": signal_by_path[path]}
                        if path in signal_by_path
                        else {}
                    ),
                }
                for path, status in sorted(status_by_path.items())
                if path in duration_by_path
            ]
            + [
                {
                    "path": test_policy.normalize_repo_relative(path),
                    "status": "not_run",
                    "duration_s": None,
                    "reason": not_run_cause,
                }
                for path in not_run_files
            ],
            "python_exe": python_exe
            if isinstance(python_exe, str)
            else " ".join(python_exe),
            "jobs": jobs,
            "run_id": run_id,
            "config": {
                "measure_rss": _diff_measure_rss(),
                "mem_limit_bytes": _memory_limit_bytes(),
                "mem_per_job_gb": _memory_guard_scheduler_per_job_gb(guard_config),
                "order": os.environ.get("MOLT_DIFF_ORDER", "auto"),
                "cargo_target_dir": os.environ.get("CARGO_TARGET_DIR", ""),
                "build_profile": build_profile,
                "compiler_target_python": (
                    None
                    if compiler_target_python is None
                    else compiler_target_python.short
                ),
                "preflight_timed_out": preflight_timed_out,
                "stdlib_profile": compat_backends.stdlib_profile_from_environment(
                    os.environ
                )
                or "",
                "warm_cache": warm_cache,
                "batch_compile_server": _diff_batch_compile_server_enabled(),
                "memory_guard": {
                    "enabled": True,
                    **_config_payload(guard_config),
                    "sample_interval": _diff_memory_guard_sample_interval_sec(),
                    "write_samples": _diff_memory_guard_write_samples(),
                    "stream_mode": _diff_memory_guard_stream_mode(),
                    "event_max_bytes": _memory_guard_jsonl_max_bytes(
                        _diff_memory_guard_events_jsonl()
                    ),
                    "sample_max_bytes": _memory_guard_jsonl_max_bytes(
                        _diff_memory_guard_global_samples_jsonl()
                    ),
                    "tripped": guard_trip_outcome is not None
                    and guard_trip_outcome.resource_failure is not None,
                    "admission_stopped": guard_trip_outcome is not None,
                    "infrastructure_failure": guard_trip_outcome.infrastructure_failure.json_payload()
                    if guard_trip_outcome is not None
                    and guard_trip_outcome.infrastructure_failure is not None
                    else None,
                    "trip_message": guard_trip_message,
                    "trip_file": str(_diff_memory_guard_trip_file()),
                },
            },
            "rss": {
                **_aggregate_rss_metrics(run_id),
                "top": rss_top_run,
                "top_run": rss_top_run,
                "top_build": rss_top_build,
            },
        }
        if failures_output is None:
            env_path = os.environ.get("MOLT_DIFF_FAILURES", "").strip()
            if env_path:
                failures_output = Path(env_path).expanduser()
            else:
                failures_output = _diff_root() / "failures.txt"
        failures_output.parent.mkdir(parents=True, exist_ok=True)
        payload = ("\n".join(failed_files) + "\n") if failed_files else ""
        failures_output.write_text(payload, encoding="utf-8")
        summary_output = os.environ.get("MOLT_DIFF_SUMMARY", "").strip()
        if summary_output:
            _emit_json(summary, summary_output, stdout=False)
        else:
            summary_path = _diff_root() / "summary.json"
            _emit_json(summary, str(summary_path), stdout=False)
        _print_rss_top(
            run_id,
            limit if _diff_measure_rss() else 0,
            status_by_path=status_by_path,
        )
        return summary
    finally:
        try:
            close_suite_sentinel()
        finally:
            compat_backends.bind_suite_stop(None)
            for signum, handler in previous_handlers.items():
                signal.signal(signum, handler)


def _emit_json(payload: dict, output_path: str | None, stdout: bool) -> None:
    text = json.dumps(payload, indent=2, sort_keys=True)
    if output_path:
        Path(output_path).write_text(text, encoding="utf-8")
    if stdout:
        print(text)


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Molt Differential Test Harness")
    parser.add_argument(
        "file",
        nargs="*",
        help="Python file(s) or directory(ies) to test",
    )
    parser.add_argument(
        "--files-from",
        action="append",
        default=[],
        help=(
            "Read additional test paths from a newline-delimited file. "
            "Can be provided multiple times."
        ),
    )
    parser.add_argument(
        "--python-version", help="Python version to test against (e.g. 3.13)"
    )
    parser.add_argument(
        "--molt-target-python",
        help=(
            "Explicit compiler target Python minor. It must match the selected "
            "CPython oracle and is forwarded to every backend build."
        ),
    )
    parser.add_argument(
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help=(
            "Build profile forwarded to `molt build` for the Molt side "
            "(default: MOLT_DIFF_BUILD_PROFILE or dev)."
        ),
    )
    parser.add_argument(
        "--compiler-profile",
        choices=["dev", "release"],
        default=None,
        help=(
            "Host compiler profile selected through MOLT_BACKEND_PROFILE, "
            "independent of --build-profile (default: preserve the environment)."
        ),
    )
    parser.add_argument(
        "--target",
        action="append",
        default=[],
        help=(
            "Backend(s) to validate against CPython: native (default), wasm, "
            "llvm, luau, or 'all'. Repeatable and comma-separated "
            "(e.g. --target native,wasm). When two or more backends run, they "
            "are ALSO compared against each other (cross-backend divergence is a "
            "FAIL). A backend whose toolchain is unavailable is reported as a "
            "loud 'uncalibrated', never a silent skip."
        ),
    )
    parser.add_argument(
        "--stdlib-profile",
        choices=["micro", "full"],
        default=None,
        help=(
            "Stdlib profile forwarded to `molt build` for the Molt side "
            "(default: MOLT_DIFF_STDLIB_PROFILE or build default)."
        ),
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit JSON summary to stdout.",
    )
    parser.add_argument(
        "--json-output",
        help="Write JSON summary to a file.",
    )
    parser.add_argument(
        "--evidence-dir",
        help="Write summary, results and aggregate log below run scratch for this relative purpose.",
    )
    parser.add_argument(
        "--jobs",
        type=int,
        default=None,
        help="Number of parallel workers (default: auto for multi-test runs).",
    )
    parser.add_argument(
        "--log-dir",
        help="Write per-test logs to a directory when running in parallel.",
    )
    parser.add_argument(
        "--log-file",
        help="Append live status lines to a central log file.",
    )
    parser.add_argument(
        "--log-aggregate",
        help="Append per-test stdout/stderr to a single log file.",
    )
    parser.add_argument(
        "--live",
        action="store_true",
        help="Emit per-test status lines as tests complete.",
    )
    parser.add_argument(
        "--deadline-s",
        type=float,
        default=None,
        help="Wall-clock budget from process start: admit only tests that can "
        "finish within it (requires MOLT_DIFF_TIMEOUT); report the rest as not_run.",
    )
    parser.add_argument(
        "--fail-fast",
        action="store_true",
        help="Stop admitting tests after a failure, OOM, or unavailable target; "
        "record every already-running result.",
    )
    parser.add_argument(
        "--failures-output",
        help="Write failed test paths to a file (default: MOLT_DIFF_ROOT/failures.txt).",
    )
    parser.add_argument(
        "--warm-cache",
        action="store_true",
        help="Warm shared MOLT_CACHE with build-only pass before running tests.",
    )

    args = parser.parse_args()

    if args.compiler_profile is not None:
        os.environ["MOLT_BACKEND_PROFILE"] = args.compiler_profile
    python_exe = (sys.executable,)
    if args.stdlib_profile is not None:
        os.environ["MOLT_DIFF_STDLIB_PROFILE"] = args.stdlib_profile
    build_profile = args.build_profile or _diff_build_profile()
    try:
        backend_targets = tuple(compat_backends.normalize_targets(args.target))
    except ValueError as exc:
        print(f"Error: {exc}", file=sys.stderr)
        sys.exit(2)

    target_paths: list[str] = list(args.file)
    for list_path in args.files_from:
        try:
            entries = test_policy.read_test_list(Path(list_path))
        except OSError as exc:
            print(f"Failed to read --files-from {list_path}: {exc}", file=sys.stderr)
            sys.exit(2)
        target_paths.extend(entries)

    _configure_diff_artifact_environment(
        repo_root=_repo_root(),
        environment=os.environ,
    )
    try:
        _configure_diff_evidence(args, repo_root=_repo_root(), environment=os.environ)
    except ValueError as exc:
        parser.error(str(exc))

    log_dir = Path(args.log_dir).expanduser() if args.log_dir else None
    log_file = Path(args.log_file).expanduser() if args.log_file else None
    log_aggregate = (
        Path(args.log_aggregate).expanduser() if args.log_aggregate else None
    )
    failures_output = (
        Path(args.failures_output).expanduser() if args.failures_output else None
    )

    if target_paths:
        targets = [Path(path) for path in target_paths]
        try:
            summary = run_diff(
                targets,
                python_exe,
                build_profile=build_profile,
                targets=backend_targets,
                target_python=args.molt_target_python,
                jobs=args.jobs,
                log_dir=log_dir,
                log_file=log_file,
                log_aggregate=log_aggregate,
                live=args.live,
                fail_fast=args.fail_fast,
                failures_output=failures_output,
                warm_cache=args.warm_cache or _diff_warm_cache(),
                deadline_s=args.deadline_s,
                oracle_version=args.python_version,
            )
        except RuntimeError as exc:
            print(f"[LOCK] {exc}", file=sys.stderr)
            sys.exit(2)
        _emit_json(summary, args.json_output, args.json)
        sys.exit(
            128 + summary["guard_signal"]
            if summary["guard_signal"] is not None
            else 0
            if summary["failed"] == 0 and summary["not_run"] == 0
            else 1
        )
    # Default test
    with open("temp_test.py", "w", encoding="utf-8") as f:
        f.write("print(1 + 2)\n")
    try:
        summary = run_diff(
            Path("temp_test.py"),
            python_exe,
            build_profile=build_profile,
            targets=backend_targets,
            target_python=args.molt_target_python,
            jobs=args.jobs,
            log_dir=log_dir,
            log_file=log_file,
            log_aggregate=log_aggregate,
            live=args.live,
            fail_fast=args.fail_fast,
            failures_output=failures_output,
            warm_cache=args.warm_cache or _diff_warm_cache(),
            deadline_s=args.deadline_s,
            oracle_version=args.python_version,
        )
    except RuntimeError as exc:
        print(f"[LOCK] {exc}", file=sys.stderr)
        os.remove("temp_test.py")
        sys.exit(2)
    _emit_json(summary, args.json_output, args.json)
    os.remove("temp_test.py")
    sys.exit(
        128 + summary["guard_signal"]
        if summary["guard_signal"] is not None
        else 0
        if summary["failed"] == 0 and summary["not_run"] == 0
        else 1
    )
