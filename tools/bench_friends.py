import argparse
import datetime as dt
import json
import os
import platform
import re
import signal
import statistics
import subprocess
import sys
import threading
import tomllib
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
_SRC_ROOT = REPO_ROOT / "src"
if _SRC_ROOT.exists() and str(_SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(_SRC_ROOT))

import harness_memory_guard  # noqa: E402
import bench as bench_tool  # noqa: E402
from molt import backend_daemon_custody as daemon_custody  # noqa: E402


SUPPORTED_SEMANTIC_MODES = {
    "runs_unmodified",
    "requires_adapter",
    "unsupported_by_molt",
}

SUPPORTED_RUNNER_ROLES = {
    "workload",
    "custody_audit",
    "c_api_scan",
}

RUNNER_NAME_RE = re.compile(r"^[A-Za-z0-9_.-]+$")
MAX_FAILURE_DETAIL_RECORDS = 32
MAX_FAILURE_MESSAGE_CHARS = 4000


def _emit_progress(message: str) -> None:
    stamp = dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds")
    # Windows PowerShell surfaces native stderr as noisy error records even when
    # the child exits successfully. Keep heartbeat lines calm there.
    stream = sys.stdout if os.name == "nt" else sys.stderr
    print(f"bench_friends: {stamp} {message}", file=stream, flush=True)


_PASSTHROUGH_ENV_KEYS = {
    "CC",
    "COMSPEC",
    "CFLAGS",
    "CommonProgramFiles",
    "CommonProgramFiles(x86)",
    "CommonProgramW6432",
    "CXX",
    "CXXFLAGS",
    "DevEnvDir",
    "ExtensionSdkDir",
    "Framework40Version",
    "FrameworkDir",
    "FrameworkDir64",
    "FrameworkVersion",
    "FrameworkVersion64",
    "HOME",
    "IFCPATH",
    "INCLUDE",
    "LANG",
    "LC_ALL",
    "LD_LIBRARY_PATH",
    "LIB",
    "LIBRARY_PATH",
    "LIBPATH",
    "LOCALAPPDATA",
    "NETFXSDKDir",
    "PATH",
    "PATHEXT",
    "Platform",
    "PROCESSOR_ARCHITECTURE",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "REQUESTS_CA_BUNDLE",
    "SDKROOT",
    "SHELL",
    "SSL_CERT_FILE",
    "SystemRoot",
    "TERM",
    "USER",
    "UCRTVersion",
    "UniversalCRTSdkDir",
    "VCIDEInstallDir",
    "VCINSTALLDIR",
    "VCToolsInstallDir",
    "VCToolsRedistDir",
    "VCToolsVersion",
    "VisualStudioVersion",
    "VSINSTALLDIR",
    "VSCMD_ARG_HOST_ARCH",
    "VSCMD_ARG_TGT_ARCH",
    "VSCMD_VER",
    "windir",
    "WindowsLibPath",
    "WindowsSdkBinPath",
    "WindowsSdkDir",
    "WindowsSdkVerBinPath",
    "WindowsSDKLibVersion",
    "WindowsSDKVersion",
}
_PASSTHROUGH_ENV_KEY_NAMES = {key.upper() for key in _PASSTHROUGH_ENV_KEYS}

_PASSTHROUGH_ENV_PREFIXES = (
    "MOLT_BENCH_",
    "MOLT_MEMORY_GUARD_",
)
_PASSTHROUGH_ENV_PREFIX_NAMES = tuple(
    prefix.upper() for prefix in _PASSTHROUGH_ENV_PREFIXES
)


@dataclass(frozen=True)
class RunnerSpec:
    name: str
    role: str
    build_cmd: list[str] | None
    run_cmd: list[str] | None
    env: dict[str, str]
    skip_reason: str | None
    json_stdout: bool


@dataclass(frozen=True)
class SourceCustody:
    source: str
    requested_ref: str | None
    expected_ref: str | None
    head_ref: str | None
    ref_verified: bool | None
    git_clean: bool | None
    git_status_porcelain: str | None
    git_ignored_artifacts: str | None
    suite_root_overridden: bool
    verification: str


@dataclass(frozen=True)
class SuiteAcquisition:
    suite_root: Path
    suite_workdir: Path
    custody: SourceCustody


@dataclass(frozen=True)
class SuiteSpec:
    id: str
    friend: str
    display_name: str
    enabled: bool
    source: str
    repo_url: str | None
    repo_ref: str | None
    local_path: str | None
    workdir: str | None
    semantic_mode: str
    adapter_notes: str | None
    tags: list[str]
    timeout_sec: int
    repeat: int
    env: dict[str, str]
    prepare_cmds: list[list[str]]
    runners: dict[str, RunnerSpec]


@dataclass
class PhaseResult:
    cmd: list[str]
    returncode: int
    elapsed_s: float
    timed_out: bool
    stdout_path: str
    stderr_path: str
    stdout_json: Any | None = None
    stdout_json_error: str | None = None
    guard_status: str | None = None
    guard_violation: dict[str, Any] | None = None
    guard_limit_at_violation: dict[str, Any] | None = None
    guard_orphaned_process_groups: list[int] = field(default_factory=list)
    guard_exit_signal: dict[str, Any] | None = None
    guard_cargo_incremental_quarantine: dict[str, Any] | None = None
    molt_failure: dict[str, Any] | None = None

    @property
    def ok(self) -> bool:
        return self.returncode == 0 and not self.timed_out


@dataclass
class RunnerResult:
    name: str
    role: str
    status: str
    reason: str | None = None
    build: PhaseResult | None = None
    runs: list[PhaseResult] = field(default_factory=list)
    run_samples_s: list[float] = field(default_factory=list)
    run_median_s: float | None = None
    run_mean_s: float | None = None
    run_stdev_s: float | None = None
    structured_outputs: list[Any] = field(default_factory=list)
    structured_samples_s: dict[str, list[float]] = field(default_factory=dict)
    structured_median_s: dict[str, float] = field(default_factory=dict)
    molt_failure: dict[str, Any] | None = None


@dataclass
class SuiteResult:
    id: str
    friend: str
    display_name: str
    semantic_mode: str
    source: str
    suite_root: str
    suite_workdir: str
    resolved_ref: str | None
    requested_ref: str | None
    source_custody: SourceCustody
    status: str
    reason: str | None
    adapter_notes: str | None
    tags: list[str]
    runners: dict[str, RunnerResult]
    metrics: dict[str, float | None]


class BenchInterrupted(BaseException):
    def __init__(self, signum: int) -> None:
        self.signum = signum
        self.signame = signal.Signals(signum).name
        super().__init__(f"interrupted by {self.signame}")


class BenchSignalScope:
    def __init__(self, signals: tuple[int, ...] = (signal.SIGTERM, signal.SIGINT)):
        self._signals = signals
        self._previous: dict[int, Any] = {}

    def __enter__(self) -> "BenchSignalScope":
        for signum in self._signals:
            self._previous[signum] = signal.getsignal(signum)
            signal.signal(signum, self._handle_signal)
        return self

    def __exit__(self, exc_type: object, exc: object, tb: object) -> None:
        for signum, previous in self._previous.items():
            signal.signal(signum, previous)

    def _handle_signal(self, signum: int, _frame: object) -> None:
        raise BenchInterrupted(signum)


def _git_rev() -> str | None:
    try:
        res = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if res.returncode != 0:
        return None
    return res.stdout.strip() or None


def _external_root() -> Path | None:
    configured = os.environ.get("MOLT_EXT_ROOT", "").strip()
    if configured:
        root = Path(configured).expanduser().resolve()
        if root.is_dir():
            return root
    return None


def _default_output_root() -> Path:
    timestamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    return REPO_ROOT / "bench" / "results" / "friends" / timestamp


def _project_python() -> str:
    suffix = "Scripts/python.exe" if os.name == "nt" else "bin/python"
    venv = os.environ.get("VIRTUAL_ENV", "").strip()
    candidates: list[Path] = []
    if venv:
        candidates.append(Path(venv) / suffix)
    candidates.append(REPO_ROOT / ".venv" / suffix)
    if sys.prefix != getattr(sys, "base_prefix", sys.prefix):
        candidates.append(Path(sys.prefix) / suffix)
    for candidate in candidates:
        if candidate.exists():
            return str(candidate)
    return sys.executable


def _load_manifest(path: Path) -> tuple[dict[str, Any], list[SuiteSpec]]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    schema_version = int(data.get("schema_version", 1))
    defaults = data.get("defaults", {})
    suites_raw = data.get("suite", [])
    if not isinstance(suites_raw, list):
        raise ValueError("manifest `suite` must be an array of tables")
    suites = [_parse_suite(raw, defaults) for raw in suites_raw]
    return {"schema_version": schema_version}, suites


def _parse_suite(raw: dict[str, Any], defaults: dict[str, Any]) -> SuiteSpec:
    suite_id = str(raw.get("id", "")).strip()
    if not suite_id:
        raise ValueError("suite id is required")
    friend = str(raw.get("friend", "")).strip()
    if not friend:
        raise ValueError(f"suite {suite_id}: friend is required")
    source = str(raw.get("source", "local")).strip()
    if source not in {"local", "git"}:
        raise ValueError(f"suite {suite_id}: unsupported source {source!r}")

    semantic_mode = str(raw.get("semantic_mode", "requires_adapter")).strip()
    if semantic_mode not in SUPPORTED_SEMANTIC_MODES:
        raise ValueError(
            f"suite {suite_id}: semantic_mode must be one of "
            f"{sorted(SUPPORTED_SEMANTIC_MODES)}"
        )

    timeout_sec = int(raw.get("timeout_sec", defaults.get("timeout_sec", 900)))
    repeat = int(raw.get("repeat", defaults.get("repeat", 3)))
    if timeout_sec <= 0:
        raise ValueError(f"suite {suite_id}: timeout_sec must be positive")
    if repeat <= 0:
        raise ValueError(f"suite {suite_id}: repeat must be positive")

    runners = _parse_runners(suite_id, raw.get("runners", {}))
    if not runners:
        raise ValueError(f"suite {suite_id}: at least one runner is required")

    prepare_cmds = _parse_command_list(raw.get("prepare_cmds", []), "prepare_cmds")
    suite_env = _parse_env(raw.get("env", defaults.get("env", {})))

    return SuiteSpec(
        id=suite_id,
        friend=friend,
        display_name=str(raw.get("display_name", suite_id)),
        enabled=bool(raw.get("enabled", False)),
        source=source,
        repo_url=_optional_str(raw.get("repo_url")),
        repo_ref=_optional_str(raw.get("repo_ref")),
        local_path=_optional_str(raw.get("local_path")),
        workdir=_optional_str(raw.get("workdir")),
        semantic_mode=semantic_mode,
        adapter_notes=_optional_str(raw.get("adapter_notes")),
        tags=[str(v) for v in raw.get("tags", [])],
        timeout_sec=timeout_sec,
        repeat=repeat,
        env=suite_env,
        prepare_cmds=prepare_cmds,
        runners=runners,
    )


def _optional_str(value: Any) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    return text or None


def _parse_env(raw_env: Any) -> dict[str, str]:
    if not raw_env:
        return {}
    if not isinstance(raw_env, dict):
        raise ValueError("env must be a table/object of string values")
    parsed: dict[str, str] = {}
    for key, value in raw_env.items():
        parsed[str(key)] = str(value)
    return parsed


def _parse_command_list(raw: Any, field_name: str) -> list[list[str]]:
    if raw in (None, []):
        return []
    if not isinstance(raw, list):
        raise ValueError(f"{field_name} must be an array")
    parsed: list[list[str]] = []
    for idx, entry in enumerate(raw):
        if not isinstance(entry, list) or not entry:
            raise ValueError(f"{field_name}[{idx}] must be a non-empty command array")
        parsed.append([str(part) for part in entry])
    return parsed


def _parse_runners(suite_id: str, raw_runners: Any) -> dict[str, RunnerSpec]:
    if not isinstance(raw_runners, dict):
        raise ValueError(f"suite {suite_id}: runners must be a table/object")
    runners: dict[str, RunnerSpec] = {}
    for runner_name, runner_raw in raw_runners.items():
        runner_name = str(runner_name)
        if not RUNNER_NAME_RE.fullmatch(runner_name):
            raise ValueError(
                f"suite {suite_id}: invalid runner name {runner_name!r}; "
                "use letters, digits, '_', '-', or '.'"
            )
        if not isinstance(runner_raw, dict):
            raise ValueError(
                f"suite {suite_id} runner {runner_name}: runner must be a table/object"
            )
        build_cmd = runner_raw.get("build_cmd")
        run_cmd = runner_raw.get("run_cmd")
        cmd = runner_raw.get("cmd")
        if run_cmd is None and cmd is not None:
            run_cmd = cmd
        json_stdout = bool(runner_raw.get("json_stdout", False))
        structured_stdout = _optional_str(runner_raw.get("structured_stdout"))
        if structured_stdout is not None:
            if structured_stdout != "json":
                raise ValueError(
                    f"suite {suite_id} runner {runner_name}: "
                    "structured_stdout must be 'json'"
                )
            json_stdout = True
        role = str(runner_raw.get("role", "workload")).strip()
        if role not in SUPPORTED_RUNNER_ROLES:
            raise ValueError(
                f"suite {suite_id} runner {runner_name}: role must be one of "
                f"{sorted(SUPPORTED_RUNNER_ROLES)}"
            )
        parsed_build = _parse_single_command(build_cmd, "build_cmd")
        parsed_run = _parse_single_command(run_cmd, "run_cmd")
        runners[runner_name] = RunnerSpec(
            name=runner_name,
            role=role,
            build_cmd=parsed_build,
            run_cmd=parsed_run,
            env=_parse_env(runner_raw.get("env", {})),
            skip_reason=_optional_str(runner_raw.get("skip_reason")),
            json_stdout=json_stdout,
        )
    return runners


def _parse_single_command(raw: Any, field_name: str) -> list[str] | None:
    if raw in (None, []):
        return None
    if not isinstance(raw, list) or not raw:
        raise ValueError(f"{field_name} must be a non-empty command array")
    return [str(part) for part in raw]


def _resolve_tokenized(parts: list[str], tokens: dict[str, str]) -> list[str]:
    return [part.format_map(tokens) for part in parts]


def _resolve_env(raw_env: dict[str, str], tokens: dict[str, str]) -> dict[str, str]:
    return {key: value.format_map(tokens) for key, value in raw_env.items()}


_FILE_ENV_PATH_KEYS = {
    "CACHEDB",
    "MOLT_GUARD_PROFILE_LOG",
}
_FILE_ENV_PATH_SUFFIXES = (
    "_DB",
    "_FILE",
    "_JSON",
    "_LOG",
    "_SQLITE",
    "_SQLITE3",
)
_DIR_ENV_PATH_KEYS = {
    "MOLT_CACHE",
    "TMP",
    "TEMP",
    "TMPDIR",
    "UV_CACHE_DIR",
    "XDG_CACHE_HOME",
}
_DIR_ENV_PATH_SUFFIXES = (
    "_DIR",
    "_DIRS",
    "_HOME",
    "_ROOT",
    "_ROOTS",
)


def _path_is_under(path: Path, root: Path) -> bool:
    try:
        path.resolve().relative_to(root.resolve())
    except ValueError:
        return False
    return True


def _materialize_output_env_paths(env: dict[str, str], *, output_root: Path) -> None:
    """Create output-root env path custody before a suite process starts."""
    output_root = output_root.resolve()
    for key, value in env.items():
        if not value:
            continue
        normalized_key = key.upper()
        is_file_path = normalized_key in _FILE_ENV_PATH_KEYS or normalized_key.endswith(
            _FILE_ENV_PATH_SUFFIXES
        )
        is_dir_path = normalized_key in _DIR_ENV_PATH_KEYS or normalized_key.endswith(
            _DIR_ENV_PATH_SUFFIXES
        )
        if not is_file_path and not is_dir_path:
            continue
        candidates = (
            [part for part in value.split(os.pathsep) if part]
            if is_dir_path
            else [value]
        )
        for candidate in candidates:
            candidate_path = Path(candidate)
            if not candidate_path.is_absolute():
                candidate_path = (REPO_ROOT / candidate_path).resolve()
            if not _path_is_under(candidate_path, output_root):
                continue
            if is_file_path:
                candidate_path.parent.mkdir(parents=True, exist_ok=True)
            else:
                candidate_path.mkdir(parents=True, exist_ok=True)


def _parse_stdout_json(stdout: str) -> tuple[Any | None, str | None]:
    text = stdout.strip()
    if not text:
        return None, "stdout was empty"
    try:
        return json.loads(text), None
    except json.JSONDecodeError as exc:
        return None, f"stdout was not valid JSON: {exc}"


def _metric_slug(value: str) -> str:
    slug = re.sub(r"[^A-Za-z0-9_]+", "_", value).strip("_").lower()
    return slug or "metric"


def _as_float(value: Any) -> float | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    return None


def _extract_structured_elapsed(payload: Any) -> dict[str, float]:
    if not isinstance(payload, dict):
        return {}

    metrics: dict[str, float] = {}
    workloads = payload.get("workloads")
    if isinstance(workloads, dict):
        for workload_name, entry in workloads.items():
            if not isinstance(entry, dict):
                continue
            elapsed = _as_float(entry.get("elapsed_s"))
            if elapsed is not None:
                metrics[_metric_slug(str(workload_name))] = elapsed

    results = payload.get("results")
    if isinstance(results, list):
        for idx, entry in enumerate(results, start=1):
            if not isinstance(entry, dict):
                continue
            elapsed = _as_float(entry.get("elapsed_s"))
            if elapsed is None:
                continue
            label = entry.get("benchmark") or entry.get("workload") or f"result_{idx}"
            metrics[_metric_slug(str(label))] = elapsed

    top_elapsed = _as_float(payload.get("elapsed_s"))
    if top_elapsed is not None:
        metrics.setdefault("total", top_elapsed)
    total_elapsed = _as_float(payload.get("total_elapsed_s"))
    if total_elapsed is not None:
        metrics["total"] = total_elapsed
    return metrics


def _rss_record_payload(record: Any | None) -> dict[str, Any] | None:
    if record is None:
        return None
    return {
        "pid": getattr(record, "pid", None),
        "rss_kb": getattr(record, "rss_kb", None),
        "rss_gb": getattr(record, "rss_gb", None),
        "command": getattr(record, "command", None),
        "scope": getattr(record, "scope", None),
    }


def _guard_status(
    *,
    returncode: int,
    violation: Any | None,
    timed_out: bool,
    orphaned_process_groups: list[int],
) -> str:
    if violation is not None:
        return "rss_limit_exceeded"
    if timed_out:
        return "timeout"
    if harness_memory_guard.memory_guard.exit_signal_payload(returncode) is not None:
        return "signal_exit"
    if returncode != 0:
        return "failed"
    if orphaned_process_groups:
        return "pass_with_orphan_cleanup"
    return "pass"


def _guarded_phase_diagnostics(
    res: subprocess.CompletedProcess[str],
) -> dict[str, Any]:
    orphaned_process_groups = [
        int(pgid) for pgid in getattr(res, "orphaned_process_groups", ()) or ()
    ]
    violation = getattr(res, "violation", None)
    timed_out = bool(getattr(res, "timed_out", False))
    limit_at_violation = getattr(res, "limit_at_violation", None)
    cargo_quarantine = getattr(res, "cargo_incremental_quarantine", None)
    return {
        "guard_status": _guard_status(
            returncode=res.returncode,
            violation=violation,
            timed_out=timed_out,
            orphaned_process_groups=orphaned_process_groups,
        ),
        "guard_violation": _rss_record_payload(violation),
        "guard_limit_at_violation": (
            None
            if limit_at_violation is None
            else harness_memory_guard.memory_guard.memory_limits_payload(
                limit_at_violation
            )
        ),
        "guard_orphaned_process_groups": orphaned_process_groups,
        "guard_exit_signal": (
            None
            if violation is not None or timed_out
            else harness_memory_guard.memory_guard.exit_signal_payload(res.returncode)
        ),
        "guard_cargo_incremental_quarantine": (
            None
            if cargo_quarantine is None
            else harness_memory_guard.memory_guard._cargo_incremental_quarantine_payload(
                cargo_quarantine
            )
        ),
    }


def _molt_failure_reason_suffix(payload: dict[str, Any] | None) -> str:
    if not payload:
        return ""
    detail = payload.get("detail")
    detail_text = f" ({detail})" if detail else ""
    return f": {payload.get('status', 'failed')}{detail_text}"


def _bounded_failure_text(value: Any) -> str | None:
    if value is None:
        return None
    text = str(value)
    if not text:
        return None
    if len(text) <= MAX_FAILURE_MESSAGE_CHARS:
        return text
    return (
        f"... <truncated to last {MAX_FAILURE_MESSAGE_CHARS} chars>\n"
        f"{text[-MAX_FAILURE_MESSAGE_CHARS:]}"
    )


def _molt_failure_with_log_refs(
    payload: dict[str, Any],
    *,
    stdout_path: Path,
    stderr_path: Path,
) -> dict[str, Any]:
    enriched = dict(payload)
    enriched["message"] = _bounded_failure_text(enriched.get("message"))
    enriched["log_refs"] = [
        {"kind": "stdout", "path": str(stdout_path)},
        {"kind": "stderr", "path": str(stderr_path)},
    ]
    return enriched


def _run_command(
    cmd: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout_sec: int,
    stdout_path: Path,
    stderr_path: Path,
    dry_run: bool,
    limits: harness_memory_guard.HarnessMemoryLimits,
    parse_stdout_json: bool = False,
    molt_failure_phase: str | None = None,
    progress_label: str | None = None,
) -> PhaseResult:
    stdout_path.parent.mkdir(parents=True, exist_ok=True)
    stderr_path.parent.mkdir(parents=True, exist_ok=True)
    if dry_run:
        if progress_label is not None:
            _emit_progress(
                f"start {progress_label} dry_run=true timeout_s={timeout_sec} "
                f"stdout={stdout_path} stderr={stderr_path}"
            )
        stdout_path.write_text(
            f"[dry-run] cwd={cwd}\n$ {' '.join(cmd)}\n", encoding="utf-8"
        )
        stderr_path.write_text("", encoding="utf-8")
        if progress_label is not None:
            _emit_progress(f"finish {progress_label} status=ok elapsed_s=0.000")
        return PhaseResult(
            cmd=cmd,
            returncode=0,
            elapsed_s=0.0,
            timed_out=False,
            stdout_path=str(stdout_path),
            stderr_path=str(stderr_path),
        )

    start = dt.datetime.now(dt.timezone.utc)
    if progress_label is not None:
        _emit_progress(
            f"start {progress_label} timeout_s={timeout_sec} argv_count={len(cmd)} "
            f"stdout={stdout_path} stderr={stderr_path}"
        )
    timed_out = False
    guard_elapsed_s: float | None = None
    diagnostics: dict[str, Any] = {}
    res: subprocess.CompletedProcess[str] | None = None
    try:
        res = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix="MOLT_BENCH",
            cwd=str(cwd),
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout_sec,
            limits=limits,
        )
        guard_elapsed_s = res.elapsed_s
        timed_out = (
            res.returncode == harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE
        )
        rc = -9 if timed_out else res.returncode
        stdout = res.stdout or ""
        stderr = res.stderr or ""
        diagnostics = _guarded_phase_diagnostics(res)
    except subprocess.TimeoutExpired as exc:
        timed_out = True
        rc = -9
        stdout = (exc.stdout or "") if isinstance(exc.stdout, str) else ""
        stderr = (exc.stderr or "") if isinstance(exc.stderr, str) else ""
        stderr = f"{stderr}\n[timeout] command exceeded {timeout_sec}s\n"
        diagnostics = {
            "guard_status": "timeout",
            "guard_violation": None,
            "guard_limit_at_violation": None,
            "guard_orphaned_process_groups": [],
            "guard_exit_signal": None,
            "guard_cargo_incremental_quarantine": None,
        }
    end = dt.datetime.now(dt.timezone.utc)
    elapsed = (
        guard_elapsed_s
        if guard_elapsed_s is not None
        else (end - start).total_seconds()
    )
    stdout_path.write_text(stdout, encoding="utf-8")
    stderr_path.write_text(stderr, encoding="utf-8")
    stdout_json = None
    stdout_json_error = None
    if parse_stdout_json:
        stdout_json, stdout_json_error = _parse_stdout_json(stdout)
    molt_failure = None
    if molt_failure_phase is not None and (
        rc != 0
        or timed_out
        or diagnostics.get("guard_violation") is not None
        or diagnostics.get("guard_orphaned_process_groups")
    ):
        failure = bench_tool.classify_molt_process_failure(
            phase=molt_failure_phase,
            returncode=rc,
            stdout=stdout,
            stderr=stderr,
            elapsed_s=elapsed,
            timed_out=timed_out,
            violation=getattr(res, "violation", None) if res is not None else None,
            orphaned_process_groups=tuple(
                int(pgid)
                for pgid in diagnostics.get("guard_orphaned_process_groups", []) or []
            ),
        )
        molt_failure = _molt_failure_with_log_refs(
            bench_tool.molt_failure_payload(failure),
            stdout_path=stdout_path,
            stderr_path=stderr_path,
        )
    phase_result = PhaseResult(
        cmd=cmd,
        returncode=rc,
        elapsed_s=elapsed,
        timed_out=timed_out,
        stdout_path=str(stdout_path),
        stderr_path=str(stderr_path),
        stdout_json=stdout_json,
        stdout_json_error=stdout_json_error,
        molt_failure=molt_failure,
        **diagnostics,
    )
    if progress_label is not None:
        status = "ok" if phase_result.ok else "failed"
        timeout_suffix = " timed_out=true" if timed_out else ""
        _emit_progress(
            f"finish {progress_label} status={status} rc={rc} "
            f"elapsed_s={elapsed:.3f}{timeout_suffix}"
        )
    return phase_result


def _base_run_env() -> dict[str, str]:
    canonical_key_names = {
        key.upper() for key in harness_memory_guard.CANONICAL_RUN_ENV_KEYS
    }
    inherited = {
        key: value
        for key, value in os.environ.items()
        if (normalized_key := key.upper()) in _PASSTHROUGH_ENV_KEY_NAMES
        or normalized_key in canonical_key_names
        or any(
            normalized_key.startswith(prefix)
            for prefix in _PASSTHROUGH_ENV_PREFIX_NAMES
        )
    }
    env = harness_memory_guard.canonical_harness_env(
        inherited,
        repo_root=REPO_ROOT,
    )
    env["PYTHONHASHSEED"] = "0"
    env["PYTHONUNBUFFERED"] = "1"
    env["PYTHONNOUSERSITE"] = "1"
    if tmpdir := env.get("TMPDIR"):
        env["TMP"] = tmpdir
        env["TEMP"] = tmpdir
    env.pop("PYTHONPATH", None)
    return env


def _run_git(
    args: list[str],
    *,
    cwd: Path | None,
    timeout_sec: int,
    dry_run: bool,
    limits: harness_memory_guard.HarnessMemoryLimits,
) -> tuple[int, str, str]:
    cmd = ["git", *args]
    if dry_run:
        return 0, "[dry-run]\n", ""
    res = harness_memory_guard.guarded_completed_process(
        cmd,
        prefix="MOLT_BENCH",
        cwd=str(cwd) if cwd is not None else None,
        capture_output=True,
        text=True,
        timeout=timeout_sec,
        limits=limits,
    )
    return res.returncode, res.stdout or "", res.stderr or ""


def _is_placeholder_ref(ref: str) -> bool:
    upper = ref.upper()
    return "PINNED" in upper or "REQUIRED" in upper or "PLACEHOLDER" in upper


def _verify_git_source_custody(
    suite: SuiteSpec,
    *,
    repo_dir: Path,
    requested_ref: str,
    timeout_sec: int,
    dry_run: bool,
    limits: harness_memory_guard.HarnessMemoryLimits,
    suite_root_overridden: bool,
    verification: str = "git_ref_and_clean_tree",
    raise_on_dirty: bool = True,
) -> SourceCustody:
    if dry_run:
        return SourceCustody(
            source=suite.source,
            requested_ref=requested_ref,
            expected_ref=None,
            head_ref=None,
            ref_verified=None,
            git_clean=None,
            git_status_porcelain=None,
            git_ignored_artifacts=None,
            suite_root_overridden=suite_root_overridden,
            verification="dry_run",
        )

    rc, out, err = _run_git(
        ["rev-parse", "--is-inside-work-tree"],
        cwd=repo_dir,
        timeout_sec=timeout_sec,
        dry_run=False,
        limits=limits,
    )
    if rc != 0 or out.strip() != "true":
        detail = err.strip() or out.strip()
        raise RuntimeError(
            f"suite {suite.id}: suite root is not a git checkout: {detail}"
        )

    rc, head_out, err = _run_git(
        ["rev-parse", "HEAD"],
        cwd=repo_dir,
        timeout_sec=timeout_sec,
        dry_run=False,
        limits=limits,
    )
    if rc != 0:
        raise RuntimeError(
            f"suite {suite.id}: git rev-parse HEAD failed: {err.strip()}"
        )
    head_ref = head_out.strip()

    rc, expected_out, err = _run_git(
        ["rev-parse", "--verify", f"{requested_ref}^{{commit}}"],
        cwd=repo_dir,
        timeout_sec=timeout_sec,
        dry_run=False,
        limits=limits,
    )
    if rc != 0:
        raise RuntimeError(
            f"suite {suite.id}: requested repo_ref {requested_ref!r} does not resolve: "
            f"{err.strip()}"
        )
    expected_ref = expected_out.strip()
    ref_verified = bool(expected_ref and expected_ref == head_ref)
    if not ref_verified:
        raise RuntimeError(
            f"suite {suite.id}: checked-out HEAD {head_ref} does not match "
            f"requested repo_ref {requested_ref!r} ({expected_ref})"
        )

    rc, status_out, err = _run_git(
        ["status", "--porcelain"],
        cwd=repo_dir,
        timeout_sec=timeout_sec,
        dry_run=False,
        limits=limits,
    )
    if rc != 0:
        raise RuntimeError(f"suite {suite.id}: git status failed: {err.strip()}")
    git_status = status_out.strip()

    rc, ignored_out, err = _run_git(
        ["ls-files", "--others", "--ignored", "--exclude-standard"],
        cwd=repo_dir,
        timeout_sec=timeout_sec,
        dry_run=False,
        limits=limits,
    )
    if rc != 0:
        raise RuntimeError(
            f"suite {suite.id}: git ignored-file custody scan failed: {err.strip()}"
        )
    ignored_files = ignored_out.strip()
    git_clean = not git_status and not ignored_files
    if raise_on_dirty and git_status:
        raise RuntimeError(
            f"suite {suite.id}: git checkout is dirty; refusing off-the-shelf "
            f"benchmark custody:\n{git_status}"
        )
    if raise_on_dirty and ignored_files:
        raise RuntimeError(
            f"suite {suite.id}: git checkout contains ignored artifacts; refusing "
            f"off-the-shelf benchmark custody:\n{ignored_files}"
        )

    return SourceCustody(
        source=suite.source,
        requested_ref=requested_ref,
        expected_ref=expected_ref,
        head_ref=head_ref,
        ref_verified=True,
        git_clean=git_clean,
        git_status_porcelain=git_status,
        git_ignored_artifacts=ignored_files,
        suite_root_overridden=suite_root_overridden,
        verification=verification,
    )


def _acquire_suite(
    suite: SuiteSpec,
    *,
    repos_root: Path,
    suite_root_override: Path | None,
    checkout: bool,
    fetch: bool,
    timeout_sec: int,
    dry_run: bool,
    limits: harness_memory_guard.HarnessMemoryLimits,
) -> SuiteAcquisition:
    if suite.source == "local":
        local_path = (
            str(suite_root_override) if suite_root_override else suite.local_path
        )
        if not local_path:
            raise ValueError(
                f"suite {suite.id}: local_path is required for source=local"
            )
        suite_root = Path(local_path).expanduser()
        if not dry_run and not suite_root.exists():
            raise FileNotFoundError(
                f"suite {suite.id}: local path not found: {suite_root}"
            )
        suite_workdir = (
            (suite_root / suite.workdir).resolve()
            if suite.workdir
            else suite_root.resolve()
        )
        return SuiteAcquisition(
            suite_root=suite_root,
            suite_workdir=suite_workdir,
            custody=SourceCustody(
                source=suite.source,
                requested_ref=None,
                expected_ref=None,
                head_ref=None,
                ref_verified=None,
                git_clean=None,
                git_status_porcelain=None,
                git_ignored_artifacts=None,
                suite_root_overridden=suite_root_override is not None,
                verification="local_path_exists" if not dry_run else "dry_run",
            ),
        )

    if suite.source != "git":
        raise ValueError(f"suite {suite.id}: unsupported source {suite.source}")
    if not suite.repo_url:
        raise ValueError(f"suite {suite.id}: repo_url is required for source=git")
    if not suite.repo_ref:
        raise ValueError(f"suite {suite.id}: repo_ref is required for source=git")
    if _is_placeholder_ref(suite.repo_ref) and not dry_run:
        raise ValueError(
            f"suite {suite.id}: repo_ref must be set to a pinned commit/tag, "
            "not a placeholder"
        )

    repo_dir = (
        suite_root_override.expanduser()
        if suite_root_override is not None
        else repos_root / suite.id
    )
    if checkout and suite_root_override is None:
        if not repo_dir.exists():
            repo_dir.parent.mkdir(parents=True, exist_ok=True)
            rc, _out, err = _run_git(
                ["clone", suite.repo_url, str(repo_dir)],
                cwd=None,
                timeout_sec=timeout_sec,
                dry_run=dry_run,
                limits=limits,
            )
            if rc != 0:
                raise RuntimeError(f"suite {suite.id}: git clone failed: {err.strip()}")
        if fetch:
            rc, _out, err = _run_git(
                ["fetch", "--all", "--tags", "--prune"],
                cwd=repo_dir,
                timeout_sec=timeout_sec,
                dry_run=dry_run,
                limits=limits,
            )
            if rc != 0:
                raise RuntimeError(f"suite {suite.id}: git fetch failed: {err.strip()}")
        rc, _out, err = _run_git(
            ["checkout", "--detach", suite.repo_ref],
            cwd=repo_dir,
            timeout_sec=timeout_sec,
            dry_run=dry_run,
            limits=limits,
        )
        if rc != 0:
            raise RuntimeError(
                f"suite {suite.id}: git checkout {suite.repo_ref} failed: {err.strip()}"
            )

    if not dry_run and not repo_dir.exists():
        raise FileNotFoundError(
            f"suite {suite.id}: repo checkout missing at {repo_dir}; "
            "run with --checkout or --suite-root <suite>=<path>"
        )
    custody = _verify_git_source_custody(
        suite,
        repo_dir=repo_dir,
        requested_ref=suite.repo_ref,
        timeout_sec=timeout_sec,
        dry_run=dry_run,
        limits=limits,
        suite_root_overridden=suite_root_override is not None,
    )
    suite_workdir = (
        (repo_dir / suite.workdir).resolve() if suite.workdir else repo_dir.resolve()
    )
    return SuiteAcquisition(
        suite_root=repo_dir,
        suite_workdir=suite_workdir,
        custody=custody,
    )


def _post_run_source_custody_failure_reason(
    suite: SuiteSpec,
    custody: SourceCustody,
) -> str | None:
    details: list[str] = []
    if custody.git_status_porcelain:
        details.append(
            "git checkout is dirty after suite execution:\n"
            f"{custody.git_status_porcelain}"
        )
    if custody.git_ignored_artifacts:
        details.append(
            "git checkout contains ignored artifacts after suite execution:\n"
            f"{custody.git_ignored_artifacts}"
        )
    if not details:
        return None
    return f"suite {suite.id}: post-run source custody check failed; " + "\n".join(
        details
    )


def _combine_suite_reasons(left: str | None, right: str | None) -> str | None:
    if not left:
        return right
    if not right:
        return left
    return f"{left}; {right}"


def _run_prepare_steps(
    suite: SuiteSpec,
    *,
    suite_workdir: Path,
    suite_env: dict[str, str],
    tokens: dict[str, str],
    timeout_sec: int,
    logs_dir: Path,
    dry_run: bool,
    limits: harness_memory_guard.HarnessMemoryLimits,
) -> tuple[bool, str | None]:
    for idx, prepare_cmd in enumerate(suite.prepare_cmds, start=1):
        resolved_cmd = _resolve_tokenized(prepare_cmd, tokens)
        out = logs_dir / f"prepare_{idx}.stdout.log"
        err = logs_dir / f"prepare_{idx}.stderr.log"
        phase = _run_command(
            resolved_cmd,
            cwd=suite_workdir,
            env=suite_env,
            timeout_sec=timeout_sec,
            stdout_path=out,
            stderr_path=err,
            dry_run=dry_run,
            limits=limits,
            progress_label=f"suite={suite.id} phase=prepare step={idx}/{len(suite.prepare_cmds)}",
        )
        if not phase.ok:
            return False, f"prepare step {idx} failed"
    return True, None


def _run_runner(
    runner: RunnerSpec,
    *,
    suite: SuiteSpec,
    suite_workdir: Path,
    suite_env: dict[str, str],
    tokens: dict[str, str],
    logs_dir: Path,
    dry_run: bool,
    limits: harness_memory_guard.HarnessMemoryLimits,
) -> RunnerResult:
    if runner.skip_reason:
        return RunnerResult(
            name=runner.name,
            role=runner.role,
            status="skipped",
            reason=runner.skip_reason,
        )
    if not runner.run_cmd:
        return RunnerResult(
            name=runner.name,
            role=runner.role,
            status="skipped",
            reason="run_cmd not configured",
        )

    env = suite_env.copy()
    env.update(_resolve_env(runner.env, tokens))
    if not dry_run and (output_root := tokens.get("output_root")):
        _materialize_output_env_paths(env, output_root=Path(output_root))
    result = RunnerResult(name=runner.name, role=runner.role, status="ok")

    if runner.build_cmd:
        build_cmd = _resolve_tokenized(runner.build_cmd, tokens)
        build = _run_command(
            build_cmd,
            cwd=suite_workdir,
            env=env,
            timeout_sec=suite.timeout_sec,
            stdout_path=logs_dir / f"{runner.name}.build.stdout.log",
            stderr_path=logs_dir / f"{runner.name}.build.stderr.log",
            dry_run=dry_run,
            limits=limits,
            molt_failure_phase="build" if runner.name == "molt" else None,
            progress_label=f"suite={suite.id} runner={runner.name} phase=build",
        )
        result.build = build
        if not build.ok:
            result.status = "failed"
            result.molt_failure = build.molt_failure
            result.reason = (
                f"build failed{_molt_failure_reason_suffix(build.molt_failure)}"
            )
            return result

    run_cmd = _resolve_tokenized(runner.run_cmd, tokens)
    for run_idx in range(1, suite.repeat + 1):
        phase = _run_command(
            run_cmd,
            cwd=suite_workdir,
            env=env,
            timeout_sec=suite.timeout_sec,
            stdout_path=logs_dir / f"{runner.name}.run{run_idx}.stdout.log",
            stderr_path=logs_dir / f"{runner.name}.run{run_idx}.stderr.log",
            dry_run=dry_run,
            limits=limits,
            parse_stdout_json=runner.json_stdout,
            molt_failure_phase="run" if runner.name == "molt" else None,
            progress_label=(
                f"suite={suite.id} runner={runner.name} "
                f"phase=run repeat={run_idx}/{suite.repeat}"
            ),
        )
        result.runs.append(phase)
        if not phase.ok:
            result.status = "failed"
            result.molt_failure = phase.molt_failure
            result.reason = (
                f"run {run_idx} failed{_molt_failure_reason_suffix(phase.molt_failure)}"
            )
            return result
        if runner.json_stdout and not dry_run:
            if phase.stdout_json_error is not None:
                result.status = "failed"
                result.reason = (
                    f"run {run_idx} JSON parse failed: {phase.stdout_json_error}"
                )
                return result
            if phase.stdout_json is None:
                result.status = "failed"
                result.reason = f"run {run_idx} did not emit JSON stdout"
                return result
            if isinstance(phase.stdout_json, dict) and phase.stdout_json.get(
                "status"
            ) not in (None, "ok"):
                result.status = "failed"
                result.reason = (
                    f"run {run_idx} emitted non-ok JSON status: "
                    f"{phase.stdout_json.get('status')!r}"
                )
                return result
            result.structured_outputs.append(phase.stdout_json)
            for metric_name, elapsed_s in _extract_structured_elapsed(
                phase.stdout_json
            ).items():
                result.structured_samples_s.setdefault(metric_name, []).append(
                    elapsed_s
                )
        result.run_samples_s.append(phase.elapsed_s)

    if result.run_samples_s:
        result.run_median_s = statistics.median(result.run_samples_s)
        result.run_mean_s = statistics.mean(result.run_samples_s)
        if len(result.run_samples_s) > 1:
            result.run_stdev_s = statistics.stdev(result.run_samples_s)
        else:
            result.run_stdev_s = 0.0
    for metric_name, samples in result.structured_samples_s.items():
        if samples:
            result.structured_median_s[metric_name] = statistics.median(samples)
    return result


def _suite_metrics(runners: dict[str, RunnerResult]) -> dict[str, float | None]:
    def _runner_median(name: str) -> float | None:
        runner = runners.get(name)
        if runner and runner.status == "ok" and runner.role == "workload":
            return runner.run_median_s
        return None

    def _speedup(baseline_s: float | None, candidate_s: float | None) -> float | None:
        if (
            baseline_s is None
            or candidate_s is None
            or baseline_s <= 0.0
            or candidate_s <= 0.0
        ):
            return None
        return baseline_s / candidate_s

    cp_s = _runner_median("cpython")
    pp_s = _runner_median("pypy")
    mt_s = _runner_median("molt")
    codon_s = _runner_median("codon")
    friend_s = _runner_median("friend")
    nuitka_s = _runner_median("nuitka")
    pyodide_s = _runner_median("pyodide")
    tinygrad_s = _runner_median("tinygrad")
    numpy_s = _runner_median("numpy")

    # Standardized lane keys align with tools/bench.py JSON naming.
    metrics: dict[str, float | None] = {
        "cpython_median_s": cp_s,
        "pypy_median_s": pp_s,
        "molt_median_s": mt_s,
        "codon_median_s": codon_s,
        "friend_median_s": friend_s,
        "nuitka_median_s": nuitka_s,
        "pyodide_median_s": pyodide_s,
        "tinygrad_median_s": tinygrad_s,
        "numpy_median_s": numpy_s,
        "cpython_time_s": cp_s,
        "pypy_time_s": pp_s,
        "molt_time_s": mt_s,
        "codon_time_s": codon_s,
        "nuitka_time_s": nuitka_s,
        "pyodide_time_s": pyodide_s,
        "tinygrad_time_s": tinygrad_s,
        "numpy_time_s": numpy_s,
        "molt_vs_cpython_speedup": _speedup(cp_s, mt_s),
        "molt_vs_pypy_speedup": _speedup(pp_s, mt_s),
        "molt_vs_codon_speedup": _speedup(codon_s, mt_s),
        "molt_vs_friend_speedup": _speedup(friend_s, mt_s),
        "friend_vs_molt_speedup": _speedup(mt_s, friend_s),
        "molt_vs_nuitka_speedup": _speedup(nuitka_s, mt_s),
        "nuitka_vs_molt_speedup": _speedup(mt_s, nuitka_s),
        "molt_vs_pyodide_speedup": _speedup(pyodide_s, mt_s),
        "pyodide_vs_molt_speedup": _speedup(mt_s, pyodide_s),
        "molt_vs_tinygrad_speedup": _speedup(tinygrad_s, mt_s),
        "tinygrad_vs_molt_speedup": _speedup(mt_s, tinygrad_s),
        "molt_vs_numpy_speedup": _speedup(numpy_s, mt_s),
        "numpy_vs_molt_speedup": _speedup(mt_s, numpy_s),
        "molt_speedup": _speedup(cp_s, mt_s),
        "molt_cpython_ratio": _speedup(mt_s, cp_s),
        "molt_pypy_ratio": _speedup(mt_s, pp_s),
        "molt_codon_ratio": _speedup(mt_s, codon_s),
        "molt_nuitka_ratio": _speedup(mt_s, nuitka_s),
        "molt_pyodide_ratio": _speedup(mt_s, pyodide_s),
        "molt_tinygrad_ratio": _speedup(mt_s, tinygrad_s),
        "molt_numpy_ratio": _speedup(mt_s, numpy_s),
    }
    structured_by_metric: dict[str, dict[str, float]] = {}
    for runner_name, runner in runners.items():
        if runner.status != "ok" or runner.role != "workload":
            continue
        runner_slug = _metric_slug(runner_name)
        metrics[f"{runner_slug}_median_s"] = runner.run_median_s
        metrics[f"{runner_slug}_time_s"] = runner.run_median_s
        for metric_name, median_s in runner.structured_median_s.items():
            metric_slug = _metric_slug(metric_name)
            metrics[f"{runner_slug}_{metric_slug}_median_s"] = median_s
            structured_by_metric.setdefault(metric_slug, {})[runner_name] = median_s

    for metric_slug, by_runner in structured_by_metric.items():
        molt_metric_s = by_runner.get("molt")
        cpython_metric_s = by_runner.get("cpython")
        friend_metric_s = by_runner.get("friend")
        tinygrad_metric_s = by_runner.get("tinygrad")
        numpy_metric_s = by_runner.get("numpy")
        metrics[f"molt_vs_cpython_{metric_slug}_speedup"] = _speedup(
            cpython_metric_s, molt_metric_s
        )
        metrics[f"molt_vs_friend_{metric_slug}_speedup"] = _speedup(
            friend_metric_s, molt_metric_s
        )
        metrics[f"molt_vs_tinygrad_{metric_slug}_speedup"] = _speedup(
            tinygrad_metric_s, molt_metric_s
        )
        metrics[f"molt_vs_numpy_{metric_slug}_speedup"] = _speedup(
            numpy_metric_s, molt_metric_s
        )
    return metrics


def _suite_status(runners: dict[str, RunnerResult]) -> tuple[str, str | None]:
    failed = [name for name, runner in runners.items() if runner.status == "failed"]
    if failed:
        return "failed", f"runner failures: {', '.join(sorted(failed))}"
    ok_count = sum(1 for runner in runners.values() if runner.status == "ok")
    if ok_count == 0:
        return "skipped", "no runnable runners"
    return "ok", None


def _format_optional(value: float | None) -> str:
    if value is None:
        return "-"
    return f"{value:.4f}"


def _render_summary_markdown(
    *,
    run_started_at: str,
    manifest_path: Path,
    json_rel: str,
    suites: list[SuiteResult],
    interrupted: dict[str, Any] | None = None,
    backend_daemon_cleanup: list[dict[str, Any]] | None = None,
    memory_guard_incidents: list[dict[str, Any]] | None = None,
    custody_artifacts: dict[str, str] | None = None,
    molt_failure_details: dict[str, Any] | None = None,
) -> str:
    lines: list[str] = []
    lines.append("# Friend Benchmark Summary")
    lines.append("")
    lines.append(f"Generated: {run_started_at}")
    lines.append(f"Manifest: `{manifest_path}`")
    lines.append(f"JSON: `{json_rel}`")
    lines.append("")
    lines.append(
        "| Suite | Semantic Mode | Status | CPython s | PyPy s | Codon s | "
        "Nuitka s | Pyodide s | Friend s | Tinygrad s | NumPy s | Molt s | "
        "Molt/CPython | Molt/PyPy | Molt/Codon | Molt/Nuitka | Molt/Pyodide | "
        "Molt/Friend | Molt/NumPy |"
    )
    lines.append(
        "| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | "
        "---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
    )
    for suite in suites:
        m = suite.metrics
        lines.append(
            "| "
            f"{suite.id} | {suite.semantic_mode} | {suite.status} | "
            f"{_format_optional(m.get('cpython_median_s'))} | "
            f"{_format_optional(m.get('pypy_median_s'))} | "
            f"{_format_optional(m.get('codon_median_s'))} | "
            f"{_format_optional(m.get('nuitka_median_s'))} | "
            f"{_format_optional(m.get('pyodide_median_s'))} | "
            f"{_format_optional(m.get('friend_median_s'))} | "
            f"{_format_optional(m.get('tinygrad_median_s'))} | "
            f"{_format_optional(m.get('numpy_median_s'))} | "
            f"{_format_optional(m.get('molt_median_s'))} | "
            f"{_format_optional(m.get('molt_cpython_ratio'))} | "
            f"{_format_optional(m.get('molt_pypy_ratio'))} | "
            f"{_format_optional(m.get('molt_codon_ratio'))} | "
            f"{_format_optional(m.get('molt_nuitka_ratio'))} | "
            f"{_format_optional(m.get('molt_pyodide_ratio'))} | "
            f"{_format_optional(m.get('molt_vs_friend_speedup'))} | "
            f"{_format_optional(m.get('molt_vs_numpy_speedup'))} |"
        )

    lines.append("")
    lines.append("## Notes")
    lines.append(
        "- Semantic mode values: `runs_unmodified`, `requires_adapter`, "
        "`unsupported_by_molt`."
    )
    lines.append(
        "- Ratio columns (`Molt/*`) > 1.0 indicate Molt is faster on the suite median."
    )
    lines.append(
        "- Compile-vs-run separation is recorded per runner when build commands "
        "are configured."
    )

    artifacts = custody_artifacts or {}
    if artifacts:
        lines.append("")
        lines.append("## Custody Artifacts")
        for key in (
            "molt_failure_details_jsonl",
            "harness_command_profile_jsonl",
            "repo_process_sentinel_jsonl",
            "backend_daemon_cleanup_jsonl",
        ):
            value = artifacts.get(key)
            if value:
                lines.append(f"- `{key}`: `{value}`")

    failure_details = molt_failure_details or {}
    failure_records = failure_details.get("records", [])
    if isinstance(failure_records, list) and failure_records:
        lines.append("")
        lines.append("## Molt Failure Details")
        for record in failure_records:
            if not isinstance(record, dict):
                continue
            detail = record.get("detail")
            detail_text = f" detail=`{detail}`" if detail else ""
            lines.append(
                f"- `{record.get('suite')}` runner=`{record.get('runner')}` "
                f"phase=`{record.get('phase')}` status=`{record.get('status')}`"
                f"{detail_text}"
            )
            log_refs = record.get("log_refs")
            if isinstance(log_refs, list):
                for ref in log_refs[:4]:
                    if isinstance(ref, dict) and ref.get("path"):
                        lines.append(
                            f"  - {ref.get('kind', 'log')}: `{ref.get('path')}`"
                        )
        if failure_details.get("truncated"):
            lines.append(
                f"- Failure detail list truncated at {MAX_FAILURE_DETAIL_RECORDS} records."
            )

    failures = [s for s in suites if s.status != "ok"]
    if failures:
        lines.append("")
        lines.append("## Non-OK Suites")
        for suite in failures:
            reason = suite.reason or "no reason provided"
            lines.append(f"- `{suite.id}`: {suite.status} ({reason})")

    incidents = memory_guard_incidents or []
    if incidents:
        lines.append("")
        lines.append("## Memory Guard Incidents")
        for incident in incidents:
            violation = incident.get("violation")
            if not isinstance(violation, dict):
                violation = {}
            lines.append(
                f"- `{incident.get('event', 'incident')}` "
                f"reason=`{violation.get('reason', 'unknown')}` "
                f"pgid=`{violation.get('pgid', '')}` "
                f"rss=`{violation.get('peak_rss_gb', violation.get('total_rss_gb', ''))}`"
            )
    if interrupted is not None:
        lines.append("")
        lines.append("## Interruption")
        lines.append(
            f"- Signal: `{interrupted['signame']}` "
            f"({interrupted['signum']}); partial results were written."
        )
    cleanup_events = backend_daemon_cleanup or []
    if cleanup_events:
        lines.append("")
        lines.append("## Backend Daemon Cleanup")
        for event in cleanup_events:
            status = event.get("status", "unknown")
            reason = event.get("reason", "unknown")
            terminated = event.get("terminated_count", 0)
            lines.append(
                f"- `{status}` reason=`{reason}` terminated={terminated} "
                f"session=`{event.get('session_id', '')}`"
            )
    lines.append("")
    lines.append("Generated by `tools/bench_friends.py`.")
    return "\n".join(lines) + "\n"


def _runner_to_dict(result: RunnerResult) -> dict[str, Any]:
    return {
        "name": result.name,
        "role": result.role,
        "status": result.status,
        "reason": result.reason,
        "build": _phase_to_dict(result.build) if result.build else None,
        "runs": [_phase_to_dict(phase) for phase in result.runs],
        "run_samples_s": result.run_samples_s,
        "run_median_s": result.run_median_s,
        "run_mean_s": result.run_mean_s,
        "run_stdev_s": result.run_stdev_s,
        "structured_outputs": result.structured_outputs,
        "structured_samples_s": result.structured_samples_s,
        "structured_median_s": result.structured_median_s,
        "molt_failure": result.molt_failure,
    }


def _phase_from_dict(payload: dict[str, Any] | None) -> PhaseResult | None:
    if payload is None:
        return None
    return PhaseResult(
        cmd=list(payload.get("cmd") or []),
        returncode=int(payload.get("returncode", 0)),
        elapsed_s=float(payload.get("elapsed_s", 0.0)),
        timed_out=bool(payload.get("timed_out", False)),
        stdout_path=str(payload.get("stdout_path", "")),
        stderr_path=str(payload.get("stderr_path", "")),
        stdout_json=payload.get("stdout_json"),
        stdout_json_error=payload.get("stdout_json_error"),
        guard_status=payload.get("guard_status"),
        guard_violation=payload.get("guard_violation"),
        guard_limit_at_violation=payload.get("guard_limit_at_violation"),
        guard_orphaned_process_groups=list(
            payload.get("guard_orphaned_process_groups") or []
        ),
        guard_exit_signal=payload.get("guard_exit_signal"),
        guard_cargo_incremental_quarantine=payload.get(
            "guard_cargo_incremental_quarantine"
        ),
    )


def _phase_to_dict(phase: PhaseResult) -> dict[str, Any]:
    return {
        "cmd": phase.cmd,
        "returncode": phase.returncode,
        "elapsed_s": phase.elapsed_s,
        "timed_out": phase.timed_out,
        "stdout_path": phase.stdout_path,
        "stderr_path": phase.stderr_path,
        "stdout_json": phase.stdout_json,
        "stdout_json_error": phase.stdout_json_error,
        "guard_status": phase.guard_status,
        "guard_violation": phase.guard_violation,
        "guard_limit_at_violation": phase.guard_limit_at_violation,
        "guard_orphaned_process_groups": phase.guard_orphaned_process_groups,
        "guard_exit_signal": phase.guard_exit_signal,
        "guard_cargo_incremental_quarantine": phase.guard_cargo_incremental_quarantine,
        "molt_failure": phase.molt_failure,
    }


def _runner_from_dict(payload: dict[str, Any]) -> RunnerResult:
    return RunnerResult(
        name=str(payload["name"]),
        role=str(payload["role"]),
        status=str(payload["status"]),
        reason=payload.get("reason"),
        build=_phase_from_dict(payload.get("build")),
        runs=[
            phase
            for item in payload.get("runs", [])
            if (phase := _phase_from_dict(item)) is not None
        ],
        run_samples_s=[float(value) for value in payload.get("run_samples_s", [])],
        run_median_s=payload.get("run_median_s"),
        run_mean_s=payload.get("run_mean_s"),
        run_stdev_s=payload.get("run_stdev_s"),
        structured_outputs=list(payload.get("structured_outputs") or []),
        structured_samples_s=dict(payload.get("structured_samples_s") or {}),
        structured_median_s=dict(payload.get("structured_median_s") or {}),
    )


def _source_custody_to_dict(custody: SourceCustody) -> dict[str, Any]:
    return {
        "source": custody.source,
        "requested_ref": custody.requested_ref,
        "expected_ref": custody.expected_ref,
        "head_ref": custody.head_ref,
        "ref_verified": custody.ref_verified,
        "git_clean": custody.git_clean,
        "git_status_porcelain": custody.git_status_porcelain,
        "git_ignored_artifacts": custody.git_ignored_artifacts,
        "suite_root_overridden": custody.suite_root_overridden,
        "verification": custody.verification,
    }


def _source_custody_from_dict(payload: dict[str, Any]) -> SourceCustody:
    return SourceCustody(
        source=str(payload["source"]),
        requested_ref=payload.get("requested_ref"),
        expected_ref=payload.get("expected_ref"),
        head_ref=payload.get("head_ref"),
        ref_verified=payload.get("ref_verified"),
        git_clean=payload.get("git_clean"),
        git_status_porcelain=payload.get("git_status_porcelain"),
        git_ignored_artifacts=payload.get("git_ignored_artifacts"),
        suite_root_overridden=bool(payload.get("suite_root_overridden", False)),
        verification=str(payload["verification"]),
    )


def _suite_to_dict(suite: SuiteResult) -> dict[str, Any]:
    return {
        "id": suite.id,
        "friend": suite.friend,
        "display_name": suite.display_name,
        "semantic_mode": suite.semantic_mode,
        "source": suite.source,
        "suite_root": suite.suite_root,
        "suite_workdir": suite.suite_workdir,
        "resolved_ref": suite.resolved_ref,
        "requested_ref": suite.requested_ref,
        "source_custody": _source_custody_to_dict(suite.source_custody),
        "status": suite.status,
        "reason": suite.reason,
        "adapter_notes": suite.adapter_notes,
        "tags": suite.tags,
        "metrics": suite.metrics,
        "runners": {
            name: _runner_to_dict(result) for name, result in suite.runners.items()
        },
    }


def _suite_from_dict(payload: dict[str, Any]) -> SuiteResult:
    return SuiteResult(
        id=str(payload["id"]),
        friend=str(payload["friend"]),
        display_name=str(payload["display_name"]),
        semantic_mode=str(payload["semantic_mode"]),
        source=str(payload["source"]),
        suite_root=str(payload.get("suite_root", "")),
        suite_workdir=str(payload.get("suite_workdir", "")),
        resolved_ref=payload.get("resolved_ref"),
        requested_ref=payload.get("requested_ref"),
        source_custody=_source_custody_from_dict(payload["source_custody"]),
        status=str(payload["status"]),
        reason=payload.get("reason"),
        adapter_notes=payload.get("adapter_notes"),
        tags=list(payload.get("tags") or []),
        runners={
            str(name): _runner_from_dict(result)
            for name, result in dict(payload.get("runners") or {}).items()
        },
        metrics=dict(payload.get("metrics") or {}),
    )


def _render_existing_results_json(
    *,
    results_json: Path,
    summary_out: Path | None,
    update_doc: bool,
) -> tuple[Path, str]:
    payload = json.loads(results_json.read_text(encoding="utf-8"))
    if payload.get("schema_version") != 1:
        raise ValueError(
            f"unsupported friend benchmark results schema: {payload.get('schema_version')!r}"
        )
    suites = [_suite_from_dict(item) for item in payload.get("suites", [])]
    generated_at = str(payload["generated_at"])
    manifest_path = Path(str(payload["manifest_path"]))
    summary_path = (summary_out or (results_json.parent / "summary.md")).resolve()
    summary_path.parent.mkdir(parents=True, exist_ok=True)
    summary_text = _render_summary_markdown(
        run_started_at=generated_at,
        manifest_path=manifest_path,
        json_rel=str(results_json.resolve()),
        suites=suites,
        interrupted=payload.get("interrupted"),
        backend_daemon_cleanup=list(payload.get("backend_daemon_cleanup") or []),
        memory_guard_incidents=list(payload.get("memory_guard_incidents") or []),
    )
    summary_path.write_text(summary_text, encoding="utf-8")
    if update_doc:
        doc_out = Path("docs/benchmarks/friend_summary.md").resolve()
        doc_out.parent.mkdir(parents=True, exist_ok=True)
        doc_out.write_text(summary_text, encoding="utf-8")
    return summary_path, summary_text


def _append_event_jsonl(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(payload, sort_keys=True) + "\n")


def _daemon_record_to_dict(
    record: daemon_custody.BackendDaemonIdentityRecord,
) -> dict[str, Any]:
    identity = record.identity
    return {
        "identity_path": str(record.path),
        "pid": identity.pid,
        "socket_path": str(identity.socket_path),
        "project_root": str(identity.project_root),
        "cargo_profile": identity.cargo_profile,
        "config_digest": identity.config_digest,
        "backend_bin": str(identity.backend_bin),
        "created_at": identity.created_at,
        "command": identity.command,
    }


def _cleanup_backend_daemons(
    *,
    run_env: dict[str, str],
    output_root: Path,
    reason: str,
) -> dict[str, Any]:
    event: dict[str, Any] = {
        "schema_version": 1,
        "event": "bench_friends_backend_daemon_cleanup",
        "recorded_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "reason": reason,
        "session_id": run_env.get("MOLT_SESSION_ID", ""),
        "project_root": str(REPO_ROOT),
        "status": "ok",
        "terminated": [],
        "terminated_count": 0,
    }
    try:
        terminated = daemon_custody.terminate_backend_daemons_for_session(
            run_env,
            project_root=REPO_ROOT,
            grace=1.0,
        )
    except Exception as exc:  # noqa: BLE001
        event["status"] = "failed"
        event["error"] = str(exc)
        print(
            "bench_friends: backend daemon cleanup failed: "
            f"reason={reason} error={exc}",
            file=sys.stderr,
        )
    else:
        event["terminated"] = [_daemon_record_to_dict(record) for record in terminated]
        event["terminated_count"] = len(terminated)
        if terminated:
            pids = ",".join(str(record.identity.pid) for record in terminated)
            print(
                "bench_friends: cleaned backend daemons: "
                f"reason={reason} count={len(terminated)} pids={pids}",
                file=sys.stderr,
            )
    _append_event_jsonl(
        output_root / "memory_guard" / "backend_daemon_cleanup.jsonl", event
    )
    return event


def _interrupted_payload(interrupted: BenchInterrupted | None) -> dict[str, Any] | None:
    if interrupted is None:
        return None
    return {
        "signum": interrupted.signum,
        "signame": interrupted.signame,
        "returncode": 128 + interrupted.signum,
        "recorded_at": dt.datetime.now(dt.timezone.utc).isoformat(),
    }


def _failure_details_path(json_out: Path) -> Path:
    if json_out.name == "results.json":
        return json_out.with_name("molt_failure_details.jsonl")
    return json_out.with_name(f"{json_out.stem}_molt_failure_details.jsonl")


def _custody_artifacts(
    *,
    output_root: Path,
    json_out: Path,
    summary_out: Path,
    failure_details_path: Path,
    run_env: dict[str, str],
) -> dict[str, str]:
    memory_guard_root = output_root / "memory_guard"
    return {
        "results_json": str(json_out),
        "summary_md": str(summary_out),
        "molt_failure_details_jsonl": str(failure_details_path),
        "harness_command_profile_jsonl": str(
            harness_memory_guard.command_profile_log_path(run_env, repo_root=REPO_ROOT)
        ),
        "repo_process_sentinel_jsonl": str(
            memory_guard_root / "bench_friends_sentinel.jsonl"
        ),
        "backend_daemon_cleanup_jsonl": str(
            memory_guard_root / "backend_daemon_cleanup.jsonl"
        ),
    }


def _molt_failure_detail_records(
    suites: list[SuiteResult],
) -> dict[str, Any]:
    records: list[dict[str, Any]] = []
    total = 0
    for suite in suites:
        for runner_name, runner in sorted(suite.runners.items()):
            failure = runner.molt_failure
            if not isinstance(failure, dict):
                continue
            total += 1
            if len(records) >= MAX_FAILURE_DETAIL_RECORDS:
                continue
            records.append(
                {
                    "suite": suite.id,
                    "runner": runner_name,
                    "phase": failure.get("phase"),
                    "status": failure.get("status"),
                    "detail": failure.get("detail"),
                    "returncode": failure.get("returncode"),
                    "timed_out": failure.get("timed_out"),
                    "elapsed_s": failure.get("elapsed_s"),
                    "message": _bounded_failure_text(failure.get("message")),
                    "guard_violation": failure.get("guard_violation"),
                    "signal": failure.get("signal"),
                    "orphaned_process_groups": failure.get("orphaned_process_groups"),
                    "log_refs": failure.get("log_refs", []),
                }
            )
    return {
        "schema_version": 1,
        "total": total,
        "truncated": total > len(records),
        "max_records": MAX_FAILURE_DETAIL_RECORDS,
        "records": records,
    }


def _write_failure_details_jsonl(
    path: Path,
    failure_details: dict[str, Any],
) -> None:
    records = failure_details.get("records", [])
    if not isinstance(records, list):
        records = []
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for record in records:
            if isinstance(record, dict):
                handle.write(json.dumps(record, sort_keys=True) + "\n")


def _write_run_outputs(
    *,
    output_root: Path,
    args: argparse.Namespace,
    metadata: dict[str, Any],
    manifest_path: Path,
    run_started: dt.datetime,
    runner_filters: set[str],
    suite_root_overrides: dict[str, Path],
    repo_ref_overrides: dict[str, str],
    suite_results: list[SuiteResult],
    limits: harness_memory_guard.HarnessMemoryLimits,
    interrupted: BenchInterrupted | None,
    backend_daemon_cleanup: list[dict[str, Any]],
    memory_guard_incidents: list[dict[str, Any]],
    run_env: dict[str, str],
) -> tuple[Path, Path, str]:
    json_out = (args.json_out or (output_root / "results.json")).resolve()
    json_out.parent.mkdir(parents=True, exist_ok=True)
    summary_out = (args.summary_out or (output_root / "summary.md")).resolve()
    failure_details_path = _failure_details_path(json_out).resolve()
    custody_artifact_refs = _custody_artifacts(
        output_root=output_root,
        json_out=json_out,
        summary_out=summary_out,
        failure_details_path=failure_details_path,
        run_env=run_env,
    )
    molt_failure_details = _molt_failure_detail_records(suite_results)
    interrupt_payload = _interrupted_payload(interrupted)
    payload = {
        "schema_version": 1,
        "manifest_schema_version": metadata["schema_version"],
        "generated_at": run_started.isoformat(),
        "manifest_path": str(manifest_path),
        "git_rev": _git_rev(),
        "dry_run": args.dry_run,
        "partial": interrupted is not None,
        "interrupted": interrupt_payload,
        "backend_daemon_cleanup": backend_daemon_cleanup,
        "memory_guard_incidents": memory_guard_incidents,
        "custody_artifacts": custody_artifact_refs,
        "molt_failure_details": molt_failure_details,
        "memory_guard": harness_memory_guard.limits_summary(limits),
        "host": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": platform.python_version(),
            "cpu_count": os.cpu_count(),
        },
        "options": {
            "include_disabled": args.include_disabled,
            "checkout": args.checkout,
            "fetch": args.fetch,
            "repeat_override": args.repeat,
            "timeout_override": args.timeout_sec,
            "runner_filter": sorted(runner_filters),
            "suite_root_overrides": {
                suite_id: str(path)
                for suite_id, path in sorted(suite_root_overrides.items())
            },
            "repo_ref_overrides": dict(sorted(repo_ref_overrides.items())),
        },
        "suites": [_suite_to_dict(suite) for suite in suite_results],
    }
    _write_failure_details_jsonl(failure_details_path, molt_failure_details)
    json_out.write_text(json.dumps(payload, indent=2, sort_keys=True), encoding="utf-8")

    summary_out.parent.mkdir(parents=True, exist_ok=True)
    summary_text = _render_summary_markdown(
        run_started_at=run_started.isoformat(),
        manifest_path=manifest_path,
        json_rel=str(json_out),
        suites=suite_results,
        interrupted=interrupt_payload,
        backend_daemon_cleanup=backend_daemon_cleanup,
        memory_guard_incidents=memory_guard_incidents,
        custody_artifacts=custody_artifact_refs,
        molt_failure_details=molt_failure_details,
    )
    summary_out.write_text(summary_text, encoding="utf-8")
    return json_out, summary_out, summary_text


def _select_suites(
    suites: list[SuiteSpec], *, suite_filters: set[str], include_disabled: bool
) -> list[SuiteSpec]:
    if suite_filters:
        selected = [suite for suite in suites if suite.id in suite_filters]
    else:
        selected = suites
    if include_disabled:
        return selected
    return [suite for suite in selected if suite.enabled]


def _parse_keyed_path_overrides(values: list[str], option_name: str) -> dict[str, Path]:
    overrides: dict[str, Path] = {}
    for raw in values:
        if "=" not in raw:
            raise ValueError(
                f"{option_name} entries must be <suite-id>=<path>: {raw!r}"
            )
        suite_id, value = raw.split("=", 1)
        suite_id = suite_id.strip()
        value = value.strip()
        if not suite_id or not value:
            raise ValueError(
                f"{option_name} entries must be <suite-id>=<path>: {raw!r}"
            )
        if suite_id in overrides:
            raise ValueError(f"{option_name} specified multiple times for {suite_id!r}")
        overrides[suite_id] = Path(value).expanduser()
    return overrides


def _parse_keyed_str_overrides(values: list[str], option_name: str) -> dict[str, str]:
    overrides: dict[str, str] = {}
    for raw in values:
        if "=" not in raw:
            raise ValueError(
                f"{option_name} entries must be <suite-id>=<value>: {raw!r}"
            )
        suite_id, value = raw.split("=", 1)
        suite_id = suite_id.strip()
        value = value.strip()
        if not suite_id or not value:
            raise ValueError(
                f"{option_name} entries must be <suite-id>=<value>: {raw!r}"
            )
        if suite_id in overrides:
            raise ValueError(f"{option_name} specified multiple times for {suite_id!r}")
        overrides[suite_id] = value
    return overrides


def _validate_override_targets(
    *,
    suites_by_id: dict[str, SuiteSpec],
    suite_root_overrides: dict[str, Path],
    repo_ref_overrides: dict[str, str],
) -> None:
    for option_name, values in (
        ("--suite-root", suite_root_overrides),
        ("--repo-ref", repo_ref_overrides),
    ):
        unknown = sorted(set(values) - set(suites_by_id))
        if unknown:
            raise ValueError(
                f"{option_name} references unknown suite id(s): {', '.join(unknown)}"
            )
    for suite_id in repo_ref_overrides:
        if suites_by_id[suite_id].source != "git":
            raise ValueError(f"--repo-ref is only valid for git suites: {suite_id}")


def _apply_runner_filter(suite: SuiteSpec, runner_filters: set[str]) -> SuiteSpec:
    if not runner_filters:
        return suite
    runners = {
        name: runner for name, runner in suite.runners.items() if name in runner_filters
    }
    if not runners:
        raise ValueError(
            f"suite {suite.id}: --runner filter selected no configured runners"
        )
    return replace(suite, runners=runners)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Run Molt against friend-owned benchmark suites using a pinned "
            "manifest and reproducible command protocol."
        )
    )
    parser.add_argument(
        "--manifest",
        type=Path,
        default=Path("bench/friends/manifest.toml"),
        help="Path to friend benchmark manifest TOML.",
    )
    parser.add_argument(
        "--suite",
        action="append",
        default=[],
        help="Run only selected suite id (repeatable).",
    )
    parser.add_argument(
        "--include-disabled",
        action="store_true",
        help="Include suites marked enabled=false in manifest.",
    )
    parser.add_argument(
        "--runner",
        action="append",
        default=[],
        help="Run only selected runner name (repeatable).",
    )
    parser.add_argument(
        "--output-root",
        type=Path,
        default=None,
        help="Output root directory. Default: bench/results/friends/<timestamp>.",
    )
    parser.add_argument(
        "--repos-root",
        type=Path,
        default=Path("bench/friends/repos"),
        help="Local cache root for git-based friend suites.",
    )
    parser.add_argument(
        "--suite-root",
        action="append",
        default=[],
        metavar="SUITE=PATH",
        help=(
            "Override a suite root with an explicit checkout/path. Repeatable; "
            "git suites still require clean-tree and repo-ref verification."
        ),
    )
    parser.add_argument(
        "--repo-ref",
        action="append",
        default=[],
        metavar="SUITE=REF",
        help=(
            "Override a git suite repo_ref without editing the manifest. Repeatable; "
            "the resolved ref must match checked-out HEAD."
        ),
    )
    parser.add_argument(
        "--repeat",
        type=int,
        default=None,
        help="Override repeat count for all suites.",
    )
    parser.add_argument(
        "--timeout-sec",
        type=int,
        default=None,
        help="Override command timeout for all suites.",
    )
    parser.add_argument(
        "--checkout",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Clone/checkout/fetch git suites as needed.",
    )
    parser.add_argument(
        "--fetch",
        action="store_true",
        help="Fetch updates before checkout for git suites.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Plan and emit artifacts without executing real commands.",
    )
    parser.add_argument(
        "--fail-fast",
        action="store_true",
        help="Stop after the first suite failure.",
    )
    parser.add_argument(
        "--summary-out",
        type=Path,
        default=None,
        help="Override summary markdown output path.",
    )
    parser.add_argument(
        "--json-out",
        type=Path,
        default=None,
        help="Override JSON output path.",
    )
    parser.add_argument(
        "--render-existing-json",
        type=Path,
        default=None,
        help=(
            "Render summary markdown from an existing friend results.json without "
            "running benchmark workloads or rewriting the JSON artifact."
        ),
    )
    parser.add_argument(
        "--update-doc",
        action="store_true",
        help="Also write docs/benchmarks/friend_summary.md from this run.",
    )
    return parser


def main() -> int:
    args = _parser().parse_args()
    if args.render_existing_json is not None:
        incompatible: list[str] = []
        if args.suite:
            incompatible.append("--suite")
        if args.include_disabled:
            incompatible.append("--include-disabled")
        if args.runner:
            incompatible.append("--runner")
        if args.output_root is not None:
            incompatible.append("--output-root")
        if args.suite_root:
            incompatible.append("--suite-root")
        if args.repo_ref:
            incompatible.append("--repo-ref")
        if args.repeat is not None:
            incompatible.append("--repeat")
        if args.timeout_sec is not None:
            incompatible.append("--timeout-sec")
        if args.checkout is not None:
            incompatible.append("--checkout/--no-checkout")
        if args.fetch:
            incompatible.append("--fetch")
        if args.dry_run:
            incompatible.append("--dry-run")
        if args.fail_fast:
            incompatible.append("--fail-fast")
        if args.json_out is not None:
            incompatible.append("--json-out")
        if incompatible:
            joined = ", ".join(sorted(incompatible))
            print(
                f"--render-existing-json cannot be combined with workload options: {joined}",
                file=sys.stderr,
            )
            return 2
        try:
            summary_out, _summary_text = _render_existing_results_json(
                results_json=args.render_existing_json.resolve(),
                summary_out=args.summary_out,
                update_doc=args.update_doc,
            )
        except (OSError, ValueError, KeyError, TypeError, json.JSONDecodeError) as exc:
            print(f"failed to render existing friend results: {exc}", file=sys.stderr)
            return 2
        print(f"Rendered summary: {summary_out}")
        if args.update_doc:
            print("Updated docs/benchmarks/friend_summary.md")
        return 0

    if args.checkout is None:
        args.checkout = True

    manifest_path = args.manifest.resolve()
    metadata, suites = _load_manifest(manifest_path)
    suites_by_id = {suite.id: suite for suite in suites}
    try:
        suite_root_overrides = _parse_keyed_path_overrides(
            args.suite_root, "--suite-root"
        )
        repo_ref_overrides = _parse_keyed_str_overrides(args.repo_ref, "--repo-ref")
        _validate_override_targets(
            suites_by_id=suites_by_id,
            suite_root_overrides=suite_root_overrides,
            repo_ref_overrides=repo_ref_overrides,
        )
    except ValueError as exc:
        print(str(exc), file=sys.stderr)
        return 2
    selected = _select_suites(
        suites,
        suite_filters=set(args.suite),
        include_disabled=args.include_disabled,
    )
    if not selected:
        print("No suites selected. Use --include-disabled or --suite.", file=sys.stderr)
        return 2
    runner_filters = {runner.strip() for runner in args.runner if runner.strip()}
    if any(not runner.strip() for runner in args.runner):
        print("--runner must not be empty", file=sys.stderr)
        return 2
    try:
        selected = [_apply_runner_filter(suite, runner_filters) for suite in selected]
    except ValueError as exc:
        print(str(exc), file=sys.stderr)
        return 2

    if args.repeat is not None and args.repeat <= 0:
        print("--repeat must be positive", file=sys.stderr)
        return 2
    if args.timeout_sec is not None and args.timeout_sec <= 0:
        print("--timeout-sec must be positive", file=sys.stderr)
        return 2

    run_started = dt.datetime.now(dt.timezone.utc)
    output_root = (args.output_root or _default_output_root()).resolve()
    output_root.mkdir(parents=True, exist_ok=True)
    repos_root = args.repos_root.resolve()

    run_env = _base_run_env()
    run_env.setdefault(
        "MOLT_GUARD_PROFILE_LOG",
        str(output_root / "memory_guard" / "commands.jsonl"),
    )
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH", run_env)

    suite_results: list[SuiteResult] = []
    backend_daemon_cleanup: list[dict[str, Any]] = []
    memory_guard_incidents: list[dict[str, Any]] = []
    output_lock = threading.RLock()
    interrupted: BenchInterrupted | None = None
    overall_rc = 0

    def write_outputs_locked() -> tuple[Path, Path, str]:
        with output_lock:
            return _write_run_outputs(
                output_root=output_root,
                args=args,
                metadata=metadata,
                manifest_path=manifest_path,
                run_started=run_started,
                runner_filters=runner_filters,
                suite_root_overrides=suite_root_overrides,
                repo_ref_overrides=repo_ref_overrides,
                suite_results=list(suite_results),
                limits=limits,
                interrupted=interrupted,
                backend_daemon_cleanup=list(backend_daemon_cleanup),
                memory_guard_incidents=list(memory_guard_incidents),
                run_env=run_env,
            )

    def record_sentinel_violation(
        _violation: Any,
        _limits: Any,
        payload: dict[str, Any],
    ) -> None:
        incident = {
            "event": payload.get("event", "repo_process_guard_tripped"),
            "recorded_at": dt.datetime.now(dt.timezone.utc).isoformat(),
            "guard_started_at": payload.get("guard_started_at"),
            "observed_at": payload.get("observed_at"),
            "elapsed_s": payload.get("elapsed_s"),
            "violation": payload.get("violation"),
            "limits": payload.get("limits"),
            "active_pgids": payload.get("active_pgids"),
            "kill_scope": payload.get("kill_scope"),
            "victim_pgid": payload.get("victim_pgid"),
            "victim_command": payload.get("victim_command"),
            "action": payload.get("action"),
        }
        with output_lock:
            memory_guard_incidents.append(incident)
            _write_run_outputs(
                output_root=output_root,
                args=args,
                metadata=metadata,
                manifest_path=manifest_path,
                run_started=run_started,
                runner_filters=runner_filters,
                suite_root_overrides=suite_root_overrides,
                repo_ref_overrides=repo_ref_overrides,
                suite_results=list(suite_results),
                limits=limits,
                interrupted=interrupted,
                backend_daemon_cleanup=list(backend_daemon_cleanup),
                memory_guard_incidents=list(memory_guard_incidents),
                run_env=run_env,
            )

    try:
        with BenchSignalScope():
            with harness_memory_guard.repo_process_sentinel(
                repo_root=REPO_ROOT,
                artifact_root=output_root,
                label="bench_friends",
                limits=limits,
                on_violation=record_sentinel_violation,
            ):
                try:
                    for suite in selected:
                        suite_timeout = args.timeout_sec or suite.timeout_sec
                        suite_repeat = args.repeat or suite.repeat
                        suite = SuiteSpec(
                            **{
                                **suite.__dict__,
                                "timeout_sec": suite_timeout,
                                "repeat": suite_repeat,
                            }
                        )
                        if suite.id in repo_ref_overrides:
                            suite = replace(
                                suite, repo_ref=repo_ref_overrides[suite.id]
                            )
                        try:
                            acquisition = _acquire_suite(
                                suite,
                                repos_root=repos_root,
                                suite_root_override=suite_root_overrides.get(suite.id),
                                checkout=args.checkout,
                                fetch=args.fetch,
                                timeout_sec=suite.timeout_sec,
                                dry_run=args.dry_run,
                                limits=limits,
                            )
                            suite_root = acquisition.suite_root
                            suite_workdir = acquisition.suite_workdir
                            source_custody = acquisition.custody
                            resolved_ref = source_custody.head_ref
                            suite_logs = output_root / "logs" / suite.id
                            tokens = {
                                "repo_root": str(Path.cwd().resolve()),
                                "suite_root": str(suite_root.resolve()),
                                "suite_workdir": str(suite_workdir.resolve()),
                                "output_root": str(output_root),
                                "pathsep": os.pathsep,
                                "python": sys.executable,
                                "project_python": _project_python(),
                            }
                            suite_env = run_env.copy()
                            suite_env.update(_resolve_env(suite.env, tokens))
                            if not args.dry_run:
                                _materialize_output_env_paths(
                                    suite_env,
                                    output_root=output_root,
                                )
                            prep_ok, prep_reason = _run_prepare_steps(
                                suite,
                                suite_workdir=suite_workdir,
                                suite_env=suite_env,
                                tokens=tokens,
                                timeout_sec=suite.timeout_sec,
                                logs_dir=suite_logs,
                                dry_run=args.dry_run,
                                limits=limits,
                            )
                            runners: dict[str, RunnerResult] = {}
                            if prep_ok:
                                for runner_name, runner_spec in suite.runners.items():
                                    runners[runner_name] = _run_runner(
                                        runner_spec,
                                        suite=suite,
                                        suite_workdir=suite_workdir,
                                        suite_env=suite_env,
                                        tokens=tokens,
                                        logs_dir=suite_logs,
                                        dry_run=args.dry_run,
                                        limits=limits,
                                    )
                            else:
                                for runner_name in suite.runners:
                                    runners[runner_name] = RunnerResult(
                                        name=runner_name,
                                        role=suite.runners[runner_name].role,
                                        status="failed",
                                        reason=prep_reason,
                                    )
                            post_run_source_custody = source_custody
                            post_run_custody_reason = None
                            if suite.source == "git" and not args.dry_run:
                                try:
                                    post_run_source_custody = _verify_git_source_custody(
                                        suite,
                                        repo_dir=suite_root,
                                        requested_ref=suite.repo_ref
                                        or source_custody.requested_ref
                                        or "",
                                        timeout_sec=suite.timeout_sec,
                                        dry_run=args.dry_run,
                                        limits=limits,
                                        suite_root_overridden=source_custody.suite_root_overridden,
                                        verification="post_run_git_ref_and_clean_tree",
                                        raise_on_dirty=False,
                                    )
                                    post_run_custody_reason = (
                                        _post_run_source_custody_failure_reason(
                                            suite,
                                            post_run_source_custody,
                                        )
                                    )
                                except Exception as exc:  # noqa: BLE001
                                    post_run_source_custody = replace(
                                        source_custody,
                                        git_clean=False,
                                        verification="post_run_git_ref_and_clean_tree_failed",
                                    )
                                    post_run_custody_reason = (
                                        f"suite {suite.id}: post-run source "
                                        f"custody check failed: {exc}"
                                    )
                            status, reason = _suite_status(runners)
                            if prep_reason and not reason:
                                reason = prep_reason
                            if post_run_custody_reason:
                                status = "failed"
                                reason = _combine_suite_reasons(
                                    reason,
                                    post_run_custody_reason,
                                )
                            metrics = _suite_metrics(runners)
                            suite_result = SuiteResult(
                                id=suite.id,
                                friend=suite.friend,
                                display_name=suite.display_name,
                                semantic_mode=suite.semantic_mode,
                                source=suite.source,
                                suite_root=str(suite_root),
                                suite_workdir=str(suite_workdir),
                                resolved_ref=resolved_ref,
                                requested_ref=post_run_source_custody.requested_ref,
                                source_custody=post_run_source_custody,
                                status=status,
                                reason=reason,
                                adapter_notes=suite.adapter_notes,
                                tags=suite.tags,
                                runners=runners,
                                metrics=metrics,
                            )
                            suite_results.append(suite_result)
                            if status == "failed":
                                overall_rc = 1
                                if args.fail_fast:
                                    break
                        except Exception as exc:  # noqa: BLE001
                            suite_result = SuiteResult(
                                id=suite.id,
                                friend=suite.friend,
                                display_name=suite.display_name,
                                semantic_mode=suite.semantic_mode,
                                source=suite.source,
                                suite_root="",
                                suite_workdir="",
                                resolved_ref=None,
                                requested_ref=suite.repo_ref,
                                source_custody=SourceCustody(
                                    source=suite.source,
                                    requested_ref=suite.repo_ref,
                                    expected_ref=None,
                                    head_ref=None,
                                    ref_verified=False
                                    if suite.source == "git"
                                    else None,
                                    git_clean=False if suite.source == "git" else None,
                                    git_status_porcelain=None,
                                    git_ignored_artifacts=None,
                                    suite_root_overridden=suite.id
                                    in suite_root_overrides,
                                    verification="not_acquired",
                                ),
                                status="failed",
                                reason=str(exc),
                                adapter_notes=suite.adapter_notes,
                                tags=suite.tags,
                                runners={},
                                metrics=_suite_metrics({}),
                            )
                            suite_results.append(suite_result)
                            overall_rc = 1
                            if args.fail_fast:
                                break
                except BenchInterrupted as exc:
                    interrupted = exc
                    raise
                finally:
                    cleanup_reason = (
                        "interrupted" if interrupted is not None else "harness_exit"
                    )
                    backend_daemon_cleanup.append(
                        _cleanup_backend_daemons(
                            run_env=run_env,
                            output_root=output_root,
                            reason=cleanup_reason,
                        )
                    )
    except BenchInterrupted as exc:
        interrupted = exc
        overall_rc = 128 + exc.signum
        print(f"bench_friends: interrupted by {exc.signame}", file=sys.stderr)

    if any(event.get("status") == "failed" for event in backend_daemon_cleanup):
        overall_rc = overall_rc or 1

    json_out, summary_out, summary_text = write_outputs_locked()

    if args.update_doc:
        doc_out = Path("docs/benchmarks/friend_summary.md").resolve()
        doc_out.parent.mkdir(parents=True, exist_ok=True)
        doc_out.write_text(summary_text, encoding="utf-8")

    print(f"Wrote JSON: {json_out}")
    print(f"Wrote summary: {summary_out}")
    if args.update_doc:
        print("Updated docs/benchmarks/friend_summary.md")
    return overall_rc


if __name__ == "__main__":
    raise SystemExit(main())
