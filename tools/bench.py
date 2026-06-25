import argparse
import datetime as dt
import hashlib
import importlib.util
import json
import os
import platform
import shlex
import shutil
import statistics
import subprocess
import sys
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
TOOLS_ROOT = Path(__file__).resolve().parent
SRC_ROOT = REPO_ROOT / "src"
BENCH_RESULTS_DIR = REPO_ROOT / "bench" / "results"
BENCH_TMP_ROOT = REPO_ROOT / "tmp" / "bench"
DEFAULT_BASELINE_PATH = BENCH_RESULTS_DIR / "baseline.json"
DEFAULT_BATCH_BUILD_TIMEOUT_S = 600.0
MAX_FAILURE_DETAIL_RECORDS = 32
MAX_FAILURE_MESSAGE_CHARS = 4000


def _chmod_tree_readable(root: Path) -> None:
    try:
        root.chmod(0o755)
    except OSError:
        pass
    for current, dirs, files in os.walk(root):
        current_path = Path(current)
        try:
            current_path.chmod(0o755)
        except OSError:
            pass
        for name in (*dirs, *files):
            try:
                (current_path / name).chmod(0o755)
            except OSError:
                pass


def _rmtree_bench_temp(path: Path) -> None:
    try:
        shutil.rmtree(path)
        return
    except OSError:
        _chmod_tree_readable(path)
    try:
        shutil.rmtree(path)
    except OSError as exc:
        print(
            f"warning: failed to remove benchmark temp dir {path}: {exc}",
            file=sys.stderr,
        )


def _append_event_jsonl(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(payload, sort_keys=True) + "\n")


if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from batch_compile_client import BatchCompileServerClient  # noqa: E402
from bench_evidence import comparable_run_metadata_errors  # noqa: E402
from bench_metadata import benchmark_reference_contract  # noqa: E402
import harness_memory_guard  # noqa: E402
import memory_guard  # noqa: E402
import bench_suites  # noqa: E402
import perf_authority  # noqa: E402
from molt import backend_daemon_custody as daemon_custody  # noqa: E402
from molt.dx import (  # noqa: E402
    CANONICAL_RUN_ENV_KEYS,
    RunContext,
    select_external_artifact_root,
)

from molt.harness_conformance import (  # noqa: E402
    build_molt_conformance_env,
    ensure_molt_conformance_dirs,
)

SUPER_SAMPLES = 10

BENCHMARKS = bench_suites.BENCHMARKS
SMOKE_BENCHMARKS = bench_suites.SMOKE_BENCHMARKS
WS_BENCHMARKS = bench_suites.WS_BENCHMARKS
DYNAMIC_BUILTIN_SLICES = bench_suites.DYNAMIC_BUILTIN_SLICES
MOLT_ARGS_BY_BENCH = bench_suites.MOLT_ARGS_BY_BENCH
molt_args_for_benchmark = bench_suites.molt_args_for_benchmark

CODON_BENCH_RUNTIME_ARGS_BY_NAME = {
    "binary_trees.py": ["20"],
    "chaos.py": ["{DEVNULL}"],
    "fannkuch.py": ["11"],
    "nbody.py": ["10000000"],
    "set_partition.py": ["15"],
    "primes.py": ["100000"],
    "taq.py": ["{TAQ_FILE}"],
    "word_count.py": ["{WORD_FILE}"],
}


@dataclass(frozen=True)
class BenchRunner:
    cmd: list[str]
    script: str | None
    env: dict[str, str]
    build_s: float = 0.0
    size_kb: float | None = None


class _BenchTempDir:
    def __init__(self, path: Path) -> None:
        self.path = path

    @property
    def name(self) -> str:
        return str(self.path)

    @classmethod
    def create(cls, root: Path) -> "_BenchTempDir":
        root.mkdir(
            mode=0o755 if os.name == "nt" else 0o777,
            parents=True,
            exist_ok=True,
        )
        for _attempt in range(100):
            path = root / f"molt-bench-{uuid.uuid4().hex[:8]}"
            try:
                path.mkdir(mode=0o755 if os.name == "nt" else 0o700)
            except FileExistsError:
                continue
            return cls(path)
        raise FileExistsError(f"could not create unique benchmark temp dir in {root}")

    def cleanup(self) -> None:
        if not self.path.exists():
            return
        _rmtree_bench_temp(self.path)


@dataclass(frozen=True)
class MoltBinary:
    path: Path
    temp_dir: object
    build_s: float
    size_kb: float


@dataclass(frozen=True)
class _RunResult:
    returncode: int
    stdout: str = ""
    stderr: str = ""


@dataclass(frozen=True)
class RunSample:
    elapsed_s: float
    stdout: str
    stderr: str


@dataclass(frozen=True)
class MoltFailure:
    phase: str
    status: str
    returncode: int | None
    timed_out: bool
    elapsed_s: float | None
    detail: str | None = None
    message: str | None = None
    stdout: str = ""
    stderr: str = ""
    signal: dict[str, object] | None = None
    guard_violation: dict[str, object] | None = None
    orphaned_process_groups: tuple[int, ...] = ()


@dataclass(frozen=True)
class SampleBatch:
    samples: list[RunSample]
    ok: bool
    warmup_samples: list[RunSample] = field(default_factory=list)
    failure: MoltFailure | None = None

    @property
    def times_s(self) -> list[float]:
        return [sample.elapsed_s for sample in self.samples] if self.ok else []

    @property
    def warmup_times_s(self) -> list[float]:
        return [sample.elapsed_s for sample in self.warmup_samples]


def _enable_line_buffering() -> None:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(line_buffering=True)
        except AttributeError:
            continue


def _run_cmd(
    cmd: list[str],
    env: dict[str, str],
    *,
    capture: bool,
    tty: bool,
    limits: harness_memory_guard.HarnessMemoryLimits | None = None,
) -> _RunResult:
    resolved_limits = limits or harness_memory_guard.limits_from_env("MOLT_BENCH", env)
    if tty and not capture:
        print(
            "TTY mode requested; using guarded subprocess mode.",
            file=sys.stderr,
        )
    res = harness_memory_guard.guarded_completed_process(
        cmd,
        prefix="MOLT_BENCH",
        env=env,
        capture_output=capture,
        limits=resolved_limits,
    )
    return _RunResult(res.returncode, res.stdout or "", res.stderr or "")


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


def _prepend_pythonpath(env: dict[str, str], path: str) -> dict[str, str]:
    current = env.get("PYTHONPATH", "")
    env["PYTHONPATH"] = f"{path}{os.pathsep}{current}" if current else path
    return env


def _base_python_env() -> dict[str, str]:
    env = os.environ.copy()
    env.setdefault("PYTHONHASHSEED", "0")
    env.setdefault("PYTHONUNBUFFERED", "1")
    return _prepend_pythonpath(env, "src")


def _canonical_interpreter(executable: str) -> str:
    """Resolve the CPython baseline interpreter to an absolute, existing path.

    Delegates to the single source of truth in `harness_memory_guard` so the
    bench and the startup/size audit canonicalize the baseline identically and
    a relative `.venv/bin/python3` form can never reach the guard's spawn
    boundary (where it would be mis-resolved against the child cwd).
    """
    return harness_memory_guard.canonical_interpreter(executable)


def _backend_daemon_record_payload(
    record: daemon_custody.BackendDaemonIdentityRecord,
) -> dict[str, object]:
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


def _backend_daemon_cleanup_log_path(env: dict[str, str]) -> Path | None:
    raw_path = env.get("MOLT_BENCH_DAEMON_CLEANUP_LOG", "").strip()
    if not raw_path:
        return None
    path = Path(raw_path).expanduser()
    return path if path.is_absolute() else REPO_ROOT / path


def _record_backend_daemon_cleanup_event(
    env: dict[str, str],
    event: dict[str, object],
) -> None:
    log_path = _backend_daemon_cleanup_log_path(env)
    if log_path is None:
        return
    _append_event_jsonl(log_path, event)


def _prune_backend_daemons(env: dict[str, str] | None = None) -> int:
    prune_env = env if env is not None else _canonical_bench_env()
    event: dict[str, object] = {
        "schema_version": 1,
        "event": "bench_backend_daemon_cleanup",
        "recorded_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "reason": prune_env.get("MOLT_BENCH_DAEMON_CLEANUP_REASON", "bench_prune"),
        "session_id": prune_env.get("MOLT_SESSION_ID", ""),
        "project_root": str(REPO_ROOT),
        "status": "ok",
        "terminated": [],
        "terminated_count": 0,
    }
    if os.name != "posix":
        event["status"] = "unsupported"
        event["detail"] = "backend_daemon_posix_signal_custody_unavailable"
        _record_backend_daemon_cleanup_event(prune_env, event)
        return 0
    try:
        terminated = daemon_custody.terminate_backend_daemons_for_session(
            prune_env,
            project_root=REPO_ROOT,
            grace=0.75,
        )
    except Exception as exc:  # noqa: BLE001
        event["status"] = "failed"
        event["error"] = str(exc)
        _record_backend_daemon_cleanup_event(prune_env, event)
        raise
    event["terminated"] = [
        _backend_daemon_record_payload(record) for record in terminated
    ]
    event["terminated_count"] = len(terminated)
    _record_backend_daemon_cleanup_event(prune_env, event)
    return len(terminated)


def _is_codon_bench_script(script: str) -> bool:
    normalized = Path(script).as_posix()
    return "codon_benchmarks/bench/codon/" in normalized


def _default_codon_taq_file() -> Path:
    explicit = os.environ.get("MOLT_BENCH_CODON_TAQ_FILE")
    if explicit:
        return Path(explicit).expanduser().resolve()
    repo_sample = (
        REPO_ROOT
        / "bench"
        / "friends"
        / "repos"
        / "codon_benchmarks"
        / "bench"
        / "data"
        / "taq.txt"
    )
    if repo_sample.exists():
        return repo_sample.resolve()
    bench_tmp_root = _bench_tmp_root(_canonical_bench_env())
    bench_tmp_root.mkdir(parents=True, exist_ok=True)
    generated = bench_tmp_root / "molt_codon_taq_sample.txt"
    if generated.exists():
        return generated.resolve()
    lines = ["timestamp|source|symbol|price|volume\n"]
    symbols = ("AAPL", "MSFT", "GOOG")
    for i in range(6000):
        timestamp = 1_700_000_000_000 + (i * 1_000_000)
        symbol = symbols[i % len(symbols)]
        volume = 100 + (i % 97)
        lines.append(f"{timestamp}|Q|{symbol}|0|{volume}\n")
    generated.write_text("".join(lines), encoding="utf-8")
    return generated.resolve()


def _default_codon_word_file() -> Path:
    explicit = os.environ.get("MOLT_BENCH_CODON_WORD_FILE")
    if explicit:
        return Path(explicit).expanduser().resolve()
    return _default_codon_taq_file()


def resolve_benchmark_run_args(script: str) -> list[str]:
    if not _is_codon_bench_script(script):
        return []
    args = CODON_BENCH_RUNTIME_ARGS_BY_NAME.get(Path(script).name, [])
    resolved: list[str] = []
    for arg in args:
        if arg == "{DEVNULL}":
            resolved.append(os.devnull)
        elif arg == "{TAQ_FILE}":
            resolved.append(str(_default_codon_taq_file()))
        elif arg == "{WORD_FILE}":
            resolved.append(str(_default_codon_word_file()))
        else:
            resolved.append(arg)
    return resolved


def measure_runtime(
    cmd_args,
    script=None,
    env=None,
    run_args=None,
    timeout_s: float | None = None,
    label: str | None = None,
    limits: harness_memory_guard.HarnessMemoryLimits | None = None,
) -> RunSample | None:
    full_cmd = cmd_args + ([script] if script else [])
    if run_args:
        full_cmd.extend(run_args)
    start = time.perf_counter()
    try:
        res = harness_memory_guard.guarded_completed_process(
            full_cmd,
            prefix="MOLT_BENCH",
            capture_output=True,
            text=True,
            env=env,
            timeout=timeout_s,
            limits=limits,
        )
    except subprocess.TimeoutExpired:
        msg = f" timed out after {timeout_s:.1f}s" if timeout_s is not None else ""
        bench_label = f" for {label}" if label else ""
        print(f"Benchmark run{bench_label}{msg}.", file=sys.stderr)
        return None
    elapsed_s = getattr(res, "elapsed_s", None)
    if elapsed_s is None:
        elapsed_s = time.perf_counter() - start
    if res.returncode != 0:
        return None
    return RunSample(elapsed_s, res.stdout, res.stderr)


def _resolve_molt_output(payload: dict) -> Path | None:
    output_str = payload.get("data", {}).get("output") or payload.get("output")
    if not output_str:
        return None
    output_path = Path(output_str)
    if output_path.exists():
        return output_path
    fallback = output_path.with_suffix(".exe")
    if fallback.exists():
        return fallback
    return None


def _bench_session_id(env: dict[str, str] | None = None) -> str:
    source = env if env is not None else os.environ
    explicit = source.get("MOLT_SESSION_ID", "").strip()
    return explicit or f"bench-{os.getpid()}"


def _selected_bench_artifact_root(env: dict[str, str]) -> Path:
    explicit = env.get("MOLT_EXT_ROOT", "").strip()
    if explicit:
        return Path(explicit).expanduser().resolve()
    return (
        select_external_artifact_root(
            REPO_ROOT,
            env,
            create_dirs=True,
            prefer_external=True,
        )
        or REPO_ROOT
    )


def _bench_tmp_root(env: dict[str, str]) -> Path:
    explicit = env.get("MOLT_BENCH_TMP_ROOT", "").strip()
    if explicit:
        return Path(explicit).expanduser().resolve()
    tmp_root = env.get("MOLT_DIFF_TMPDIR") or env.get("TMPDIR")
    if tmp_root:
        return Path(tmp_root).expanduser().resolve() / "bench"
    return _selected_bench_artifact_root(env) / "tmp" / "bench"


def _canonical_bench_env(base_env: dict[str, str] | None = None) -> dict[str, str]:
    env = (os.environ.copy() if base_env is None else base_env).copy()
    explicit_canonical_keys = {key for key in CANONICAL_RUN_ENV_KEYS if env.get(key)}
    for key, value in build_molt_conformance_env(
        REPO_ROOT,
        _bench_session_id(env),
    ).items():
        if key not in CANONICAL_RUN_ENV_KEYS:
            env[key] = value
    force_default_keys = tuple(
        key
        for key in CANONICAL_RUN_ENV_KEYS
        if key not in explicit_canonical_keys
        and key not in {"MOLT_EXT_ROOT", "MOLT_SESSION_ID"}
    )
    env = RunContext(
        REPO_ROOT,
        session_prefix="bench",
        prefer_external_artifacts=True,
    ).canonical_env(
        env,
        create_dirs=True,
        force_default_keys=force_default_keys,
    )
    env.setdefault("MOLT_BENCH_TMP_ROOT", str(_bench_tmp_root(env)))
    ensure_molt_conformance_dirs(env)
    BENCH_RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    _bench_tmp_root(env).mkdir(parents=True, exist_ok=True)
    return env


def _molt_build_cmd(build_profile: str) -> list[str]:
    """Return the command prefix for invoking the Molt compiler.

    Benchmarks inherit the exact interpreter running the harness.  That keeps
    Windows custody single-owner: no nested ``uv run`` launcher can create a
    second Python process tree with a different hash-seed/bootstrap contract.
    """
    return [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--build-profile",
        build_profile,
    ]


class _BenchBatchBuildServer:
    def __init__(self, env: dict[str, str]) -> None:
        self._guard_context = harness_memory_guard.HarnessExecutionContext.from_env(
            "MOLT_BENCH",
            env,
            repo_root=REPO_ROOT,
        )
        self._limits = self._guard_context.limits
        self._client = BatchCompileServerClient(
            [
                sys.executable,
                "-m",
                "molt.cli",
                "internal-batch-build-server",
            ],
            cwd=REPO_ROOT,
            env=env,
            guard_context=self._guard_context,
            reader_name="molt-bench-batch-server-reader",
        )

    def request_build(
        self, params: dict[str, object], *, timeout_s: float
    ) -> dict[str, object]:
        return self._client.request("build", params=params, timeout=timeout_s)

    def close(self) -> None:
        self._client.close(timeout=5.0)


def _molt_build_params(
    *,
    script: str,
    out_dir: Path,
    build_profile: str,
    extra_args: list[str] | None,
    env: dict[str, str],
    use_molt_build_cache: bool = True,
) -> dict[str, object]:
    params: dict[str, object] = {
        "file_path": script,
        "profile": build_profile,
        "target": "native",
        "trusted": True,
        "json_output": True,
        "out_dir": str(out_dir),
        "cache": use_molt_build_cache,
        "env_overrides": env,
        "codec": env.get("MOLT_CODEC", "msgpack"),
    }
    remaining = list(extra_args or [])
    while remaining:
        arg = remaining.pop(0)
        if arg == "--type-hints":
            if not remaining:
                raise ValueError("--type-hints requires a value")
            params["type_hints"] = remaining.pop(0)
            continue
        if arg.startswith("--type-hints="):
            params["type_hints"] = arg.split("=", maxsplit=1)[1]
            continue
        if arg == "--stdlib-profile":
            if not remaining:
                raise ValueError("--stdlib-profile requires a value")
            params["stdlib_profile"] = remaining.pop(0)
            continue
        if arg.startswith("--stdlib-profile="):
            params["stdlib_profile"] = arg.split("=", maxsplit=1)[1]
            continue
        raise ValueError(f"unsupported benchmark Molt build arg: {arg}")
    return params


def _batch_response_completed_process(
    args: list[str], response: dict[str, object]
) -> subprocess.CompletedProcess[str]:
    returncode = response.get("returncode")
    if not isinstance(returncode, int):
        returncode = 0 if response.get("ok") is True else 1
    stdout = response.get("stdout")
    stderr = response.get("stderr")
    error = response.get("error")
    stderr_text = stderr if isinstance(stderr, str) else ""
    if isinstance(error, str) and error:
        stderr_text = f"{stderr_text}\n{error}" if stderr_text else error
    return subprocess.CompletedProcess(
        args=args,
        returncode=returncode,
        stdout=stdout if isinstance(stdout, str) else "",
        stderr=stderr_text,
    )


_BUILD_FAILURE_SIGNATURES: tuple[tuple[str, str], ...] = (
    ("backend daemon returned empty response", "backend_daemon_empty_response"),
    (
        "backend daemon died while request was in flight",
        "backend_daemon_died_in_flight",
    ),
    ("backend daemon process is not running", "backend_daemon_process_not_running"),
    ("backend daemon closed response pipe", "backend_daemon_closed_response_pipe"),
    ("batch compile server response timed out", "batch_server_response_timeout"),
    ("batch compile server closed response pipe", "batch_server_closed_response_pipe"),
    ("batch compile server process is not running", "batch_server_process_not_running"),
    ("invalid batch compile response json", "batch_server_invalid_json"),
)

_RUNTIME_FAILURE_SIGNATURES: tuple[tuple[str, str], ...] = (
    (
        "molt fatal: invalid object header before dec_ref",
        "molt_runtime_invalid_object_header_before_dec_ref",
    ),
    ("molt fatal:", "molt_runtime_fatal"),
)


def _rss_record_payload(
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


def _failure_output(stdout: str | None, stderr: str | None) -> str:
    parts = [part.strip() for part in (stderr or "", stdout or "") if part.strip()]
    return "\n".join(parts)


def _bounded_failure_text(
    value: object,
    *,
    limit: int = MAX_FAILURE_MESSAGE_CHARS,
) -> str | None:
    if value is None:
        return None
    text = str(value)
    if not text:
        return None
    if len(text) <= limit:
        return text
    return f"... <truncated to last {limit} chars>\n{text[-limit:]}"


def _failure_message(stdout: str | None, stderr: str | None) -> str | None:
    output = _failure_output(stdout, stderr)
    if not output:
        return None
    return _bounded_failure_text(output)


def _failure_detail(phase: str, stdout: str | None, stderr: str | None) -> str | None:
    haystack = _failure_output(stdout, stderr).casefold()
    signatures = (
        _BUILD_FAILURE_SIGNATURES if phase == "build" else _RUNTIME_FAILURE_SIGNATURES
    )
    for needle, detail in signatures:
        if needle in haystack:
            return detail
    return None


def _resolved_molt_failure_phase(
    phase: str, stdout: str | None, stderr: str | None
) -> str:
    if phase == "run" and _failure_detail("build", stdout, stderr) is not None:
        return "build"
    return phase


def _classified_molt_failure(
    *,
    phase: str,
    returncode: int | None,
    stdout: str | None = None,
    stderr: str | None = None,
    elapsed_s: float | None,
    timed_out: bool = False,
    violation: memory_guard.RssViolation | None = None,
    orphaned_process_groups: tuple[int, ...] = (),
    default_status: str,
    detail: str | None = None,
) -> MoltFailure:
    phase = _resolved_molt_failure_phase(phase, stdout, stderr)
    signal_payload = (
        None if returncode is None else memory_guard.exit_signal_payload(returncode)
    )
    detail = detail or _failure_detail(phase, stdout, stderr)
    if violation is not None:
        status = "rss_limit_exceeded"
    elif timed_out:
        status = "timeout"
    elif signal_payload is not None:
        status = "signal_exit"
    elif (
        phase == "build"
        and detail is not None
        and detail.startswith(("backend_daemon_", "batch_server_"))
    ):
        status = "daemon_crash"
    elif phase == "run" and detail is not None and detail.startswith("molt_runtime_"):
        status = "runtime_crash"
    elif orphaned_process_groups:
        status = "orphaned_processes_cleaned"
    else:
        status = default_status
    return MoltFailure(
        phase=phase,
        status=status,
        returncode=returncode,
        timed_out=timed_out,
        elapsed_s=elapsed_s,
        detail=detail,
        message=_failure_message(stdout, stderr),
        stdout=stdout or "",
        stderr=stderr or "",
        signal=signal_payload,
        guard_violation=_rss_record_payload(violation),
        orphaned_process_groups=orphaned_process_groups,
    )


def classify_molt_process_failure(
    *,
    phase: str,
    returncode: int | None,
    stdout: str | None = None,
    stderr: str | None = None,
    elapsed_s: float | None,
    timed_out: bool = False,
    violation: memory_guard.RssViolation | None = None,
    orphaned_process_groups: tuple[int, ...] = (),
    default_status: str | None = None,
) -> MoltFailure:
    resolved_phase = _resolved_molt_failure_phase(phase, stdout, stderr)
    return _classified_molt_failure(
        phase=resolved_phase,
        returncode=returncode,
        stdout=stdout,
        stderr=stderr,
        elapsed_s=elapsed_s,
        timed_out=timed_out,
        violation=violation,
        orphaned_process_groups=orphaned_process_groups,
        default_status=default_status
        or ("build_failed" if resolved_phase == "build" else "runtime_failed"),
    )


def _classified_molt_exception(
    *,
    phase: str,
    exc: BaseException,
    elapsed_s: float | None,
    default_status: str,
) -> MoltFailure:
    timed_out = isinstance(exc, (TimeoutError, subprocess.TimeoutExpired))
    return _classified_molt_failure(
        phase=phase,
        returncode=memory_guard.TIMEOUT_RETURN_CODE if timed_out else None,
        stderr=str(exc),
        elapsed_s=elapsed_s,
        timed_out=timed_out,
        default_status="timeout" if timed_out else default_status,
    )


def molt_failure_payload(failure: MoltFailure) -> dict[str, object]:
    return {
        "phase": failure.phase,
        "status": failure.status,
        "detail": failure.detail,
        "message": failure.message,
        "stdout_tail": _bounded_failure_text(failure.stdout),
        "stderr_tail": _bounded_failure_text(failure.stderr),
        "returncode": failure.returncode,
        "timed_out": failure.timed_out,
        "elapsed_s": failure.elapsed_s,
        "signal": failure.signal,
        "guard_violation": failure.guard_violation,
        "orphaned_process_groups": list(failure.orphaned_process_groups),
    }


def _molt_failure_json_fields(failure: MoltFailure | None) -> dict[str, object]:
    status = "pass" if failure is None else failure.status
    payload = None if failure is None else molt_failure_payload(failure)
    return {
        "molt_status": status,
        "molt_run_status": status,
        "molt_run_returncode": 0 if failure is None else failure.returncode,
        "molt_run_timed_out": False if failure is None else failure.timed_out,
        "molt_failure": payload,
        "molt_failure_phase": None if payload is None else payload["phase"],
        "molt_failure_status": None if payload is None else payload["status"],
        "molt_failure_detail": None if payload is None else payload["detail"],
        "molt_failure_message": None if payload is None else payload["message"],
        "molt_failure_returncode": None if payload is None else payload["returncode"],
        "molt_failure_timed_out": (False if payload is None else payload["timed_out"]),
        "molt_failure_elapsed_s": None if payload is None else payload["elapsed_s"],
        "molt_failure_signal": None if payload is None else payload["signal"],
        "molt_failure_guard_violation": (
            None if payload is None else payload["guard_violation"]
        ),
        "molt_failure_orphaned_process_groups": (
            [] if payload is None else payload["orphaned_process_groups"]
        ),
    }


def _emit_molt_build_failure(
    script: str, res: subprocess.CompletedProcess[str]
) -> None:
    print(
        f"Molt build failed for {Path(script).name} with exit {res.returncode}.",
        file=sys.stderr,
    )
    for label, text in (("stdout", res.stdout), ("stderr", res.stderr)):
        body = (text or "").strip()
        if not body:
            continue
        if len(body) > 4000:
            body = body[-4000:]
            body = f"... <truncated to last 4000 chars>\n{body}"
        print(f"--- Molt build {label} ---\n{body}", file=sys.stderr)


def prepare_molt_binary(
    script: str,
    extra_args: list[str] | None = None,
    env: dict[str, str] | None = None,
    *,
    build_profile: str = "release",
    batch_server: _BenchBatchBuildServer | None = None,
    build_timeout_s: float = DEFAULT_BATCH_BUILD_TIMEOUT_S,
    limits: harness_memory_guard.HarnessMemoryLimits | None = None,
    use_molt_build_cache: bool = True,
) -> MoltBinary | MoltFailure:
    env = _canonical_bench_env(env)
    _prune_backend_daemons(env)
    resolved_limits = limits or harness_memory_guard.limits_from_env("MOLT_BENCH", env)
    bench_tmp_root = _bench_tmp_root(env)

    def _attempt_build() -> MoltBinary | MoltFailure:
        temp_dir = _BenchTempDir.create(bench_tmp_root)
        out_dir = Path(temp_dir.name)
        args = [
            *_molt_build_cmd(build_profile),
            "--trusted",
            "--json",
            "--cache" if use_molt_build_cache else "--rebuild",
            "--out-dir",
            str(out_dir),
        ]
        args.extend(extra_args or [])
        args.append(script)
        start = time.perf_counter()
        try:
            if batch_server is None:
                res = harness_memory_guard.guarded_completed_process(
                    args,
                    prefix="MOLT_BENCH",
                    env=env,
                    capture_output=True,
                    text=True,
                    timeout=build_timeout_s,
                    limits=resolved_limits,
                )
            else:
                params = _molt_build_params(
                    script=script,
                    out_dir=out_dir,
                    build_profile=build_profile,
                    extra_args=extra_args,
                    env=env,
                    use_molt_build_cache=use_molt_build_cache,
                )
                response = batch_server.request_build(params, timeout_s=build_timeout_s)
                res = _batch_response_completed_process(args, response)
        except (
            RuntimeError,
            TimeoutError,
            ValueError,
            subprocess.TimeoutExpired,
        ) as exc:
            failure = _classified_molt_exception(
                phase="build",
                exc=exc,
                elapsed_s=time.perf_counter() - start,
                default_status="build_failed",
            )
            temp_dir.cleanup()
            return failure
        build_s = time.perf_counter() - start

        if res.returncode != 0:
            _emit_molt_build_failure(script, res)
            failure = _classified_molt_failure(
                phase="build",
                returncode=res.returncode,
                stdout=res.stdout,
                stderr=res.stderr,
                elapsed_s=build_s,
                timed_out=bool(getattr(res, "timed_out", False)),
                violation=getattr(res, "violation", None),
                orphaned_process_groups=tuple(
                    int(pgid)
                    for pgid in getattr(res, "orphaned_process_groups", ()) or ()
                ),
                default_status="build_failed",
            )
            temp_dir.cleanup()
            return failure

        try:
            payload = json.loads(res.stdout.strip() or "{}")
        except json.JSONDecodeError:
            _emit_molt_build_failure(script, res)
            failure = _classified_molt_failure(
                phase="build",
                returncode=res.returncode,
                stdout=res.stdout,
                stderr=res.stderr,
                elapsed_s=build_s,
                default_status="build_output_invalid",
                detail="build_json_invalid",
            )
            temp_dir.cleanup()
            return failure

        output_path = _resolve_molt_output(payload)
        if output_path is None:
            failure = _classified_molt_failure(
                phase="build",
                returncode=res.returncode,
                stdout=res.stdout,
                stderr=res.stderr,
                elapsed_s=build_s,
                default_status="build_artifact_missing",
                detail="build_output_missing",
            )
            temp_dir.cleanup()
            return failure

        binary_size = output_path.stat().st_size / 1024
        return MoltBinary(output_path, temp_dir, build_s, binary_size)

    result = _attempt_build()
    if isinstance(result, MoltBinary):
        return result

    print(
        "Backend build failed; pruning stale daemons and retrying...", file=sys.stderr
    )
    _prune_backend_daemons(env)
    time.sleep(1)
    return _attempt_build()


def measure_molt_run(
    binary: Path,
    env: dict[str, str] | None = None,
    label: str | None = None,
    run_args: list[str] | None = None,
    timeout_s: float | None = None,
    limits: harness_memory_guard.HarnessMemoryLimits | None = None,
) -> RunSample | MoltFailure | None:
    cmd = [str(binary)]
    if run_args:
        cmd.extend(run_args)
    start = time.perf_counter()
    try:
        res = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix="MOLT_BENCH",
            capture_output=True,
            text=True,
            env=env,
            timeout=timeout_s,
            limits=limits,
        )
    except subprocess.TimeoutExpired:
        msg = f" timed out after {timeout_s:.1f}s" if timeout_s is not None else ""
        if label:
            print(f"Molt run timed out for {label}{msg}.", file=sys.stderr)
        else:
            print(f"Molt run timed out{msg}.", file=sys.stderr)
        return MoltFailure(
            phase="run",
            status="timeout",
            returncode=memory_guard.TIMEOUT_RETURN_CODE,
            timed_out=True,
            elapsed_s=timeout_s,
            detail=None,
            message=None,
        )
    elapsed_s = getattr(res, "elapsed_s", None)
    if elapsed_s is None:
        elapsed_s = time.perf_counter() - start
    orphaned_process_groups = tuple(
        int(pgid) for pgid in getattr(res, "orphaned_process_groups", ()) or ()
    )
    if res.returncode != 0 or orphaned_process_groups:
        err = (res.stderr or res.stdout).strip()
        if err:
            prefix = f"Molt run failed for {label}: " if label else "Molt run failed: "
            print(f"{prefix}{err}", file=sys.stderr)
        return _classified_molt_failure(
            phase="run",
            returncode=res.returncode,
            stdout=res.stdout,
            stderr=res.stderr,
            timed_out=bool(getattr(res, "timed_out", False)),
            elapsed_s=elapsed_s,
            violation=getattr(res, "violation", None),
            orphaned_process_groups=orphaned_process_groups,
            default_status="runtime_failed",
        )
    return RunSample(elapsed_s, res.stdout, res.stderr)


def collect_samples(measure_fn, samples, warmup=0) -> SampleBatch:
    # Warmup runs are discarded. They prime (1) the page cache for the binary
    # and its shared libs, and (2) on macOS the amfid/provenance cache for the
    # binary's cdhash — both keyed to the EXACT artifact path the measured runs
    # reuse, so the recorded samples reflect warm steady-state. The same warmup
    # count is applied identically to every runtime (cpython, pypy, molt) by the
    # shared call site in `_bench_one`, keeping the comparison fair. Cold-start
    # and binary-size live in tools/output_startup_size_audit.py, not here.
    warmup_samples: list[RunSample] = []
    for _ in range(warmup):
        sample = measure_fn()
        if sample is None:
            return SampleBatch(
                [],
                False,
                warmup_samples,
                MoltFailure("run", "runtime_failed", None, False, None),
            )
        if isinstance(sample, MoltFailure):
            return SampleBatch([], False, warmup_samples, sample)
        warmup_samples.append(sample)
    measured: list[RunSample] = []
    for _ in range(samples):
        sample = measure_fn()
        if sample is None:
            return SampleBatch(
                measured,
                False,
                warmup_samples,
                MoltFailure("run", "runtime_failed", None, False, None),
            )
        if isinstance(sample, MoltFailure):
            return SampleBatch(measured, False, warmup_samples, sample)
        measured.append(sample)
    return SampleBatch(measured, bool(measured), warmup_samples)


def summarize_samples(samples: list[float]) -> dict[str, float | list[float]]:
    mean = statistics.mean(samples)
    median = statistics.median(samples)
    variance = statistics.pvariance(samples) if len(samples) > 1 else 0.0
    min_s = min(samples)
    max_s = max(samples)
    return {
        "mean_s": mean,
        "median_s": median,
        "variance_s": variance,
        "range_s": max_s - min_s,
        "min_s": min_s,
        "max_s": max_s,
        "samples_s": list(samples),
    }


def _sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8", "surrogatepass")).hexdigest()


def _stable_output(batch: SampleBatch) -> tuple[str, str] | None:
    if not batch.ok or not batch.samples:
        return None
    first = batch.samples[0]
    expected = (first.stdout, first.stderr)
    if all((sample.stdout, sample.stderr) == expected for sample in batch.samples):
        return expected
    return None


def _output_parity_evidence(
    reference_batch: SampleBatch | None,
    molt_batch: SampleBatch,
    *,
    reference_runtime: str,
    reference_required: bool,
    reference_reason: str,
) -> dict[str, object]:
    empty_hashes = {
        "reference_stdout_sha256": None,
        "molt_stdout_sha256": None,
        "reference_stderr_sha256": None,
        "molt_stderr_sha256": None,
    }
    if not reference_required:
        return {
            "checked": False,
            "ok": None,
            "reference_runtime": reference_runtime,
            "reason": reference_reason,
            "stdout_match": None,
            "stderr_match": None,
            **empty_hashes,
        }

    if reference_batch is None or not reference_batch.ok:
        return {
            "checked": False,
            "ok": None,
            "reference_runtime": reference_runtime,
            "reason": "reference_unavailable",
            "stdout_match": None,
            "stderr_match": None,
            **empty_hashes,
        }

    reference_output = _stable_output(reference_batch)
    if reference_output is None:
        return {
            "checked": True,
            "ok": False,
            "reference_runtime": reference_runtime,
            "reason": "reference_unstable",
            "stdout_match": None,
            "stderr_match": None,
            **empty_hashes,
        }

    if not molt_batch.ok:
        reference_stdout, reference_stderr = reference_output
        return {
            "checked": True,
            "ok": False,
            "reference_runtime": reference_runtime,
            "reason": "molt_unavailable",
            "stdout_match": None,
            "stderr_match": None,
            "reference_stdout_sha256": _sha256_text(reference_stdout),
            "molt_stdout_sha256": None,
            "reference_stderr_sha256": _sha256_text(reference_stderr),
            "molt_stderr_sha256": None,
        }

    molt_output = _stable_output(molt_batch)
    if molt_output is None:
        reference_stdout, reference_stderr = reference_output
        return {
            "checked": True,
            "ok": False,
            "reference_runtime": reference_runtime,
            "reason": "molt_unstable",
            "stdout_match": None,
            "stderr_match": None,
            "reference_stdout_sha256": _sha256_text(reference_stdout),
            "molt_stdout_sha256": None,
            "reference_stderr_sha256": _sha256_text(reference_stderr),
            "molt_stderr_sha256": None,
        }

    reference_stdout, reference_stderr = reference_output
    molt_stdout, molt_stderr = molt_output
    stdout_match = reference_stdout == molt_stdout
    stderr_match = reference_stderr == molt_stderr
    ok = stdout_match and stderr_match
    if ok:
        reason = "match"
    elif not stdout_match:
        reason = "stdout_mismatch"
    else:
        reason = "stderr_mismatch"
    return {
        "checked": True,
        "ok": ok,
        "reference_runtime": reference_runtime,
        "reason": reason,
        "stdout_match": stdout_match,
        "stderr_match": stderr_match,
        "reference_stdout_sha256": _sha256_text(reference_stdout),
        "molt_stdout_sha256": _sha256_text(molt_stdout),
        "reference_stderr_sha256": _sha256_text(reference_stderr),
        "molt_stderr_sha256": _sha256_text(molt_stderr),
    }


def _has_native_output_parity_failures(payload: dict) -> bool:
    for stats in payload.get("benchmarks", {}).values():
        parity = stats.get("molt_output_parity")
        if (
            isinstance(parity, dict)
            and parity.get("checked")
            and parity.get("ok") is False
        ):
            return True
    return False


def _module_available(name: str) -> bool:
    return importlib.util.find_spec(name) is not None


def _find_compiled_binary(output_dir: Path, stem: str) -> Path | None:
    candidates = [
        output_dir / stem,
        output_dir / f"{stem}.bin",
        output_dir / f"{stem}.exe",
    ]
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    for candidate in sorted(output_dir.glob(f"{stem}*")):
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return candidate
    return None


def _nuitka_command(explicit_cmd: str | None) -> list[str] | None:
    if explicit_cmd:
        parts = shlex.split(explicit_cmd)
        return parts if parts else None
    nuitka = shutil.which("nuitka")
    if nuitka:
        return [nuitka]
    if _module_available("nuitka"):
        return [sys.executable, "-m", "nuitka"]
    return None


def _prepare_nuitka_runner(
    script_path: Path,
    build_root: Path,
    base_env: dict[str, str],
    *,
    tty: bool,
    nuitka_cmd: list[str] | None,
    limits: harness_memory_guard.HarnessMemoryLimits,
) -> BenchRunner | None:
    if nuitka_cmd is None:
        return None
    module_name = f"bench_nuitka_{script_path.stem}"
    module_dir = build_root / module_name
    module_dir.mkdir(parents=True, exist_ok=True)
    build_start = time.perf_counter()
    build = _run_cmd(
        [
            *nuitka_cmd,
            "--onefile",
            "--output-dir",
            str(module_dir),
            "--remove-output",
            str(script_path),
        ],
        env=base_env,
        capture=not tty,
        tty=tty,
        limits=limits,
    )
    build_s = time.perf_counter() - build_start
    if build.returncode != 0:
        return None
    binary_path = _find_compiled_binary(module_dir, script_path.stem)
    if binary_path is None:
        return None
    size_kb = binary_path.stat().st_size / 1024
    return BenchRunner(
        [str(binary_path)], None, base_env, build_s=build_s, size_kb=size_kb
    )


def _pyodide_command(explicit_cmd: str | None) -> list[str] | None:
    if explicit_cmd:
        parts = shlex.split(explicit_cmd)
        return parts if parts else None
    env_cmd = os.environ.get("MOLT_BENCH_PYODIDE_CMD", "").strip()
    if env_cmd:
        parts = shlex.split(env_cmd)
        return parts if parts else None
    return None


def _prepare_pyodide_runner(
    script_path: Path,
    base_env: dict[str, str],
    *,
    pyodide_cmd: list[str] | None,
) -> BenchRunner | None:
    if pyodide_cmd is None:
        return None
    return BenchRunner(pyodide_cmd, str(script_path), base_env)


def _prepare_codon_runner(
    script_path: Path,
    build_root: Path,
    base_env: dict[str, str],
    *,
    tty: bool,
    limits: harness_memory_guard.HarnessMemoryLimits,
) -> BenchRunner | None:
    codon = shutil.which("codon")
    if not codon:
        return None
    arch_prefix: list[str] = []
    if platform.system() == "Darwin" and platform.machine() == "x86_64":
        arch_prefix = ["/usr/bin/arch", "-arm64"]
    module_name = f"bench_codon_{script_path.stem}"
    module_dir = build_root / module_name
    module_dir.mkdir(parents=True, exist_ok=True)
    binary_path = module_dir / module_name
    env = base_env.copy()
    codon_home: str | None = None
    if "CODON_HOME" not in env:
        codon_path = Path(codon).resolve()
        candidate = codon_path.parent.parent
        if (candidate / "lib" / "codon").exists():
            codon_home = str(candidate)
            env["CODON_HOME"] = codon_home
    else:
        codon_home = env.get("CODON_HOME")
    build_start = time.perf_counter()
    build = _run_cmd(
        arch_prefix
        + [codon, "build", "-release", str(script_path), "-o", str(binary_path)],
        env=env,
        capture=not tty,
        tty=tty,
        limits=limits,
    )
    build_s = time.perf_counter() - build_start
    if build.returncode != 0:
        return None
    if codon_home:
        libomp = Path(codon_home) / "lib" / "codon" / "libomp.dylib"
        target = module_dir / "libomp.dylib"
        if libomp.exists() and not target.exists():
            shutil.copy2(libomp, target)
    size_kb = binary_path.stat().st_size / 1024 if binary_path.exists() else None
    return BenchRunner(
        arch_prefix + [str(binary_path)], None, env, build_s=build_s, size_kb=size_kb
    )


def _pypy_command(
    env: dict[str, str], limits: harness_memory_guard.HarnessMemoryLimits
) -> list[str] | None:
    if not shutil.which("uv"):
        print("Skipping PyPy: uv not found.", file=sys.stderr)
        return None
    probe = harness_memory_guard.guarded_completed_process(
        [
            "uv",
            "run",
            "--no-project",
            "--python",
            "pypy@3.11",
            "python",
            "-c",
            "print('ok')",
        ],
        prefix="MOLT_BENCH",
        env=env,
        capture_output=True,
        text=True,
        limits=limits,
    )
    if probe.returncode != 0:
        msg = (probe.stderr or probe.stdout).strip().splitlines()
        hint = msg[-1] if msg else "PyPy unavailable for this Python requirement"
        print(f"Skipping PyPy: {hint}", file=sys.stderr)
        return None
    return ["uv", "run", "--no-project", "--python", "pypy@3.11", "python"]


def bench_results(
    benchmarks,
    samples,
    warmup,
    use_cpython,
    use_pypy,
    use_codon,
    use_nuitka,
    use_pyodide,
    super_run,
    runtime_timeout_s,
    molt_build_profile,
    *,
    tty: bool,
    nuitka_cmd: str | None,
    pyodide_cmd: str | None,
    use_molt_build_cache: bool = True,
    artifact_root: Path | None = None,
):
    base_env = _canonical_bench_env(_base_python_env())
    sentinel_artifact_root = artifact_root.resolve() if artifact_root else None
    if sentinel_artifact_root is not None:
        (sentinel_artifact_root / "memory_guard").mkdir(parents=True, exist_ok=True)
        base_env.setdefault(
            "MOLT_GUARD_PROFILE_LOG",
            str(sentinel_artifact_root / "memory_guard" / "commands.jsonl"),
        )
        base_env["MOLT_BENCH_DAEMON_CLEANUP_LOG"] = str(
            sentinel_artifact_root / "memory_guard" / "backend_daemon_cleanup.jsonl"
        )
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH", base_env)
    runtimes = {}
    if use_cpython:
        runtimes["cpython"] = [_canonical_interpreter(sys.executable)]
    if use_pypy:
        pypy_cmd = _pypy_command(base_env, limits)
        if pypy_cmd:
            runtimes["pypy"] = pypy_cmd

    if use_codon and not shutil.which("codon"):
        print("Skipping Codon: codon not found.", file=sys.stderr)
        use_codon = False
    resolved_nuitka_cmd = _nuitka_command(nuitka_cmd) if use_nuitka else None
    if use_nuitka and resolved_nuitka_cmd is None:
        print(
            "Skipping Nuitka: nuitka not found (or pass --nuitka-cmd).",
            file=sys.stderr,
        )
        use_nuitka = False
    resolved_pyodide_cmd = _pyodide_command(pyodide_cmd) if use_pyodide else None
    if use_pyodide and resolved_pyodide_cmd is None:
        print(
            "Skipping Pyodide: set --pyodide-cmd or MOLT_BENCH_PYODIDE_CMD.",
            file=sys.stderr,
        )
        use_pyodide = False
    bench_tmp_root = _bench_tmp_root(base_env)
    sentinel_artifact_root = sentinel_artifact_root or bench_tmp_root

    header = (
        f"{'Benchmark':<30} | {'CPython(s)':<10} | {'PyPy(s)':<10} | "
        f"{'CodonBuild':<10} | {'CodonRun':<10} | {'NuitkaBld':<10} | "
        f"{'NuitkaRun':<10} | {'PyodideRun':<10} | {'MoltBuild':<10} | "
        f"{'MoltRun':<10} | {'MoltKB':<10} | {'Speedup':<10} | {'M/PyPy':<10} | "
        f"{'M/Codon':<10} | {'M/Nuitka':<10} | {'M/Pyodide':<10}"
    )
    print(header)
    print("-" * len(header))

    codon_root = REPO_ROOT / "bench" / "codon"
    nuitka_root = REPO_ROOT / "bench" / "nuitka"

    data = {}
    with harness_memory_guard.repo_process_sentinel(
        repo_root=REPO_ROOT,
        artifact_root=sentinel_artifact_root,
        label="bench",
        limits=limits,
    ):
        batch_server = _BenchBatchBuildServer(base_env)
        try:
            for script in benchmarks:
                data.update(
                    _bench_one(
                        script,
                        samples,
                        warmup,
                        runtimes,
                        use_codon,
                        use_nuitka,
                        use_pyodide,
                        super_run,
                        runtime_timeout_s,
                        molt_build_profile,
                        tty=tty,
                        base_env=base_env,
                        codon_root=codon_root,
                        nuitka_root=nuitka_root,
                        resolved_nuitka_cmd=resolved_nuitka_cmd,
                        resolved_pyodide_cmd=resolved_pyodide_cmd,
                        batch_server=batch_server,
                        limits=limits,
                        use_molt_build_cache=use_molt_build_cache,
                    )
                )
        finally:
            batch_server.close()

    return data


def _bench_one(
    script,
    samples,
    warmup,
    runtimes,
    use_codon,
    use_nuitka,
    use_pyodide,
    super_run,
    runtime_timeout_s,
    molt_build_profile,
    *,
    tty: bool,
    base_env: dict[str, str],
    codon_root: Path,
    nuitka_root: Path,
    resolved_nuitka_cmd: list[str] | None,
    resolved_pyodide_cmd: list[str] | None,
    batch_server: _BenchBatchBuildServer,
    limits: harness_memory_guard.HarnessMemoryLimits,
    use_molt_build_cache: bool = True,
):
    results = {}
    runtime_ok = {}
    runtime_batches: dict[str, SampleBatch] = {}
    stats = {}
    data = {}
    name = os.path.basename(script)
    reference_contract = benchmark_reference_contract(script)
    run_args = resolve_benchmark_run_args(script)
    for rt_name, cmd in runtimes.items():
        if not reference_contract.external_baselines:
            continue
        batch = collect_samples(
            lambda: measure_runtime(
                cmd,
                script,
                env=base_env,
                run_args=run_args,
                timeout_s=runtime_timeout_s,
                label=f"{name} [{rt_name}]",
                limits=limits,
            ),
            samples,
            warmup=warmup,
        )
        runtime_batches[rt_name] = batch
        sample_times = batch.times_s
        results[rt_name] = statistics.mean(sample_times) if batch.ok else None
        runtime_ok[rt_name] = batch.ok
        if batch.ok:
            # Always record min (best-achievable, hyperfine norm) and σ
            # (via variance_s) alongside the mean headline so best-case
            # regressions are caught without requiring --super-run.
            stats[rt_name] = summarize_samples(sample_times)

    codon_time: float | None = None
    codon_build: float | None = None
    codon_size: float | None = None
    codon_ok = False
    codon_batch: SampleBatch | None = None
    if use_codon and reference_contract.external_baselines:
        runner = _prepare_codon_runner(
            Path(script),
            codon_root,
            base_env,
            tty=tty,
            limits=limits,
        )
        if runner is not None:
            codon_build = runner.build_s
            codon_size = runner.size_kb
            codon_batch = collect_samples(
                lambda: measure_runtime(
                    runner.cmd,
                    runner.script,
                    env=runner.env,
                    run_args=run_args,
                    timeout_s=runtime_timeout_s,
                    label=f"{name} [codon]",
                    limits=limits,
                ),
                samples,
                warmup=warmup,
            )
            codon_ok = codon_batch.ok
            if codon_ok:
                codon_samples = codon_batch.times_s
                codon_time = statistics.mean(codon_samples)
                # min + σ recorded unconditionally (hyperfine norm), matching
                # the primary-runtime lane so best-case regressions are visible
                # without --super-run.
                stats["codon"] = summarize_samples(codon_samples)
        else:
            print(f"Skipping Codon for {name}.", file=sys.stderr)

    nuitka_time: float | None = None
    nuitka_build: float | None = None
    nuitka_size: float | None = None
    nuitka_ok = False
    nuitka_batch: SampleBatch | None = None
    if use_nuitka and reference_contract.external_baselines:
        runner = _prepare_nuitka_runner(
            Path(script),
            nuitka_root,
            base_env,
            tty=tty,
            nuitka_cmd=resolved_nuitka_cmd,
            limits=limits,
        )
        if runner is not None:
            nuitka_build = runner.build_s
            nuitka_size = runner.size_kb
            nuitka_batch = collect_samples(
                lambda: measure_runtime(
                    runner.cmd,
                    runner.script,
                    env=runner.env,
                    run_args=run_args,
                    timeout_s=runtime_timeout_s,
                    label=f"{name} [nuitka]",
                    limits=limits,
                ),
                samples,
                warmup=warmup,
            )
            nuitka_ok = nuitka_batch.ok
            if nuitka_ok:
                nuitka_samples = nuitka_batch.times_s
                nuitka_time = statistics.mean(nuitka_samples)
                stats["nuitka"] = summarize_samples(nuitka_samples)
        else:
            print(f"Skipping Nuitka for {name}.", file=sys.stderr)

    pyodide_time: float | None = None
    pyodide_build: float | None = None
    pyodide_size: float | None = None
    pyodide_ok = False
    pyodide_batch: SampleBatch | None = None
    if use_pyodide and reference_contract.external_baselines:
        runner = _prepare_pyodide_runner(
            Path(script), base_env, pyodide_cmd=resolved_pyodide_cmd
        )
        if runner is not None:
            pyodide_batch = collect_samples(
                lambda: measure_runtime(
                    runner.cmd,
                    runner.script,
                    env=runner.env,
                    run_args=run_args,
                    timeout_s=runtime_timeout_s,
                    label=f"{name} [pyodide]",
                    limits=limits,
                ),
                samples,
                warmup=warmup,
            )
            pyodide_ok = pyodide_batch.ok
            if pyodide_ok:
                pyodide_samples = pyodide_batch.times_s
                pyodide_time = statistics.mean(pyodide_samples)
                stats["pyodide"] = summarize_samples(pyodide_samples)
        else:
            print(f"Skipping Pyodide for {name}.", file=sys.stderr)

    molt_time: float | None = None
    molt_size: float | None = None
    molt_build: float | None = None
    molt_args = molt_args_for_benchmark(script)
    molt_ok = False
    molt_batch = SampleBatch([], False)
    molt_failure: MoltFailure | None = None
    molt_runner_result = prepare_molt_binary(
        script,
        molt_args,
        env=base_env,
        build_profile=molt_build_profile,
        batch_server=batch_server,
        limits=limits,
        use_molt_build_cache=use_molt_build_cache,
    )
    if isinstance(molt_runner_result, MoltBinary):
        molt_runner = molt_runner_result
        try:
            molt_batch = collect_samples(
                lambda: measure_molt_run(
                    molt_runner.path,
                    env=base_env,
                    label=name,
                    run_args=run_args,
                    timeout_s=runtime_timeout_s,
                    limits=limits,
                ),
                samples,
                warmup=warmup,
            )
            molt_ok = molt_batch.ok
            molt_failure = molt_batch.failure
            if molt_ok:
                molt_samples = molt_batch.times_s
                molt_time = statistics.mean(molt_samples)
                stats["molt"] = summarize_samples(molt_samples)
            molt_build = molt_runner.build_s
            molt_size = molt_runner.size_kb
        finally:
            molt_runner.temp_dir.cleanup()
    else:
        molt_failure = molt_runner_result
        print(f"Molt build/run failed for {name}.", file=sys.stderr)

    cpython_time = results.get("cpython") if runtime_ok.get("cpython", False) else None
    output_parity = _output_parity_evidence(
        runtime_batches.get("cpython"),
        molt_batch,
        reference_runtime=reference_contract.reference_runtime,
        reference_required=reference_contract.external_baselines,
        reference_reason=reference_contract.reason,
    )
    if output_parity["checked"] and not output_parity["ok"]:
        parity_elapsed_s = molt_time
        molt_ok = False
        molt_time = None
        if molt_failure is None:
            molt_failure = MoltFailure(
                phase="parity",
                status="output_mismatch",
                returncode=0,
                timed_out=False,
                elapsed_s=parity_elapsed_s,
                detail=str(output_parity.get("reason") or "output_mismatch"),
            )
    molt_failure_fields = _molt_failure_json_fields(molt_failure)
    pypy_time = results.get("pypy") if runtime_ok.get("pypy", False) else None
    # Route through the single authority guard: a missing/None/non-positive
    # molt_time (build failure, daemon crash, runaway) yields None - NEVER a
    # finite ratio. A non-ok molt run must not produce a speedup even if a
    # stale molt_time lingered.
    speedup = perf_authority.safe_speedup(cpython_time, molt_time) if molt_ok else None
    # Every emitted molt/X ratio is produced by the SINGLE guarded authority
    # (perf_authority.signed_ratio), in the MOLT_OVER_BASELINE direction
    # (molt_time / baseline_time; < 1.0 means molt is faster). A missing /
    # None / 0 / NaN baseline time (the external-runtime-absent shape) yields
    # value=None, NEVER a finite slowness ratio - the four sibling external
    # ratios previously bypassed the guard entirely. A non-ok molt run forces
    # the molt operand to None so no ratio is produced.
    _molt_time_if_ok = molt_time if molt_ok else None
    _MOB = perf_authority.RatioDirection.MOLT_OVER_BASELINE
    cpython_ratio_block = perf_authority.signed_ratio(
        _molt_time_if_ok, cpython_time, direction=_MOB
    )
    pypy_ratio_block = perf_authority.signed_ratio(
        _molt_time_if_ok, pypy_time, direction=_MOB
    )
    codon_ratio_block = perf_authority.signed_ratio(
        _molt_time_if_ok, codon_time if codon_ok else None, direction=_MOB
    )
    nuitka_ratio_block = perf_authority.signed_ratio(
        _molt_time_if_ok, nuitka_time if nuitka_ok else None, direction=_MOB
    )
    pyodide_ratio_block = perf_authority.signed_ratio(
        _molt_time_if_ok, pyodide_time if pyodide_ok else None, direction=_MOB
    )
    ratio = cpython_ratio_block["value"]
    pypy_ratio = pypy_ratio_block["value"]
    codon_ratio = codon_ratio_block["value"]
    nuitka_ratio = nuitka_ratio_block["value"]
    pyodide_ratio = pyodide_ratio_block["value"]

    def _cell(value: float | None, width: int = 10) -> str:
        if value is None:
            return f"{'n/a':<{width}}"
        return f"{value:<{width}.4f}"

    def _ratio_cell(value: float | None, width: int = 10) -> str:
        if value is None:
            return f"{'n/a':<{width}}"
        return f"{value:<{width}.2f}x"

    cpython_cell = _cell(cpython_time)
    pypy_cell = _cell(pypy_time)
    codon_build_cell = _cell(codon_build)
    codon_run_cell = _cell(codon_time)
    nuitka_build_cell = _cell(nuitka_build)
    nuitka_run_cell = _cell(nuitka_time)
    pyodide_run_cell = _cell(pyodide_time)
    molt_build_cell = _cell(molt_build)
    molt_run_cell = _cell(molt_time)
    molt_size_cell = _cell(molt_size)
    speedup_cell = _ratio_cell(speedup)
    pypy_ratio_cell = _ratio_cell(pypy_ratio)
    codon_ratio_cell = _ratio_cell(codon_ratio)
    nuitka_ratio_cell = _ratio_cell(nuitka_ratio)
    pyodide_ratio_cell = _ratio_cell(pyodide_ratio)

    print(
        f"{name:<30} | {cpython_cell} | {pypy_cell} | {codon_build_cell} | "
        f"{codon_run_cell} | {nuitka_build_cell} | {nuitka_run_cell} | "
        f"{pyodide_run_cell} | {molt_build_cell} | {molt_run_cell} | "
        f"{molt_size_cell} | {speedup_cell} | {pypy_ratio_cell} | "
        f"{codon_ratio_cell} | {nuitka_ratio_cell} | {pyodide_ratio_cell}"
    )

    def _runtime_samples(rt_name: str) -> list[float] | None:
        batch = runtime_batches.get(rt_name)
        return batch.times_s if batch is not None else None

    def _runtime_warmup_samples(rt_name: str) -> list[float] | None:
        batch = runtime_batches.get(rt_name)
        return batch.warmup_times_s if batch is not None else None

    def _optional_samples(batch: SampleBatch | None) -> list[float] | None:
        return batch.times_s if batch is not None else None

    def _optional_warmup_samples(batch: SampleBatch | None) -> list[float] | None:
        return batch.warmup_times_s if batch is not None else None

    data[name] = {
        "cpython_time_s": cpython_time,
        "cpython_samples_s": _runtime_samples("cpython"),
        "cpython_warmup_samples_s": _runtime_warmup_samples("cpython"),
        "pypy_time_s": pypy_time,
        "pypy_samples_s": _runtime_samples("pypy"),
        "pypy_warmup_samples_s": _runtime_warmup_samples("pypy"),
        "codon_time_s": codon_time,
        "codon_samples_s": _optional_samples(codon_batch),
        "codon_warmup_samples_s": _optional_warmup_samples(codon_batch),
        "codon_build_s": codon_build,
        "codon_size_kb": codon_size,
        "nuitka_time_s": nuitka_time,
        "nuitka_samples_s": _optional_samples(nuitka_batch),
        "nuitka_warmup_samples_s": _optional_warmup_samples(nuitka_batch),
        "nuitka_build_s": nuitka_build,
        "nuitka_size_kb": nuitka_size,
        "pyodide_time_s": pyodide_time,
        "pyodide_samples_s": _optional_samples(pyodide_batch),
        "pyodide_warmup_samples_s": _optional_warmup_samples(pyodide_batch),
        "pyodide_build_s": pyodide_build,
        "pyodide_size_kb": pyodide_size,
        "molt_time_s": molt_time,
        "molt_samples_s": molt_batch.times_s,
        "molt_warmup_samples_s": molt_batch.warmup_times_s,
        "molt_build_s": molt_build,
        "molt_size_kb": molt_size,
        "molt_speedup": speedup,
        "molt_cpython_ratio": ratio,
        "molt_pypy_ratio": pypy_ratio,
        "molt_codon_ratio": codon_ratio,
        "molt_nuitka_ratio": nuitka_ratio,
        "molt_pyodide_ratio": pyodide_ratio,
        # Explicit direction metadata for every ratio field above, so a
        # downstream/ranking consumer can never misread the sign of a ratio
        # (audit meta-bug item 2). molt_speedup is baseline/molt (>1 = faster);
        # every molt_*_ratio is molt/baseline (<1 = faster).
        "ratio_directions": {
            "molt_speedup": perf_authority.RatioDirection.SPEEDUP.value,
            "molt_cpython_ratio": cpython_ratio_block["direction"],
            "molt_pypy_ratio": pypy_ratio_block["direction"],
            "molt_codon_ratio": codon_ratio_block["direction"],
            "molt_nuitka_ratio": nuitka_ratio_block["direction"],
            "molt_pyodide_ratio": pyodide_ratio_block["direction"],
        },
        "molt_ok": molt_ok,
        **molt_failure_fields,
        "molt_output_parity": output_parity,
        "reference_runtime": reference_contract.reference_runtime,
        "reference_reason": reference_contract.reason,
        "pypy_ok": runtime_ok.get("pypy", False),
        "molt_args": molt_args,
        "run_args": run_args,
        "codon_ok": codon_ok,
        "nuitka_ok": nuitka_ok,
        "pyodide_ok": pyodide_ok,
    }
    # Always serialize per-runtime min/mean/σ so downstream consumers
    # (bench_report, regression checks) see best-achievable + variance without
    # requiring --super-run. `super_stats` is retained under --super-run for the
    # legacy verbose-table consumer.
    data[name]["runtime_stats"] = stats
    if super_run:
        data[name]["super_stats"] = stats
    return data


def write_json(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")


def load_json(path: Path) -> dict:
    return json.loads(path.read_text())


def compare_baseline(current: dict, baseline: dict, max_regression: float) -> list[str]:
    metadata_errors = comparable_run_metadata_errors(current, baseline)
    if metadata_errors:
        return [
            "incompatible benchmark baseline: "
            + "; ".join(metadata_errors)
            + "; regenerate the baseline with matching benchmark timing settings"
        ]

    regressions = []
    baseline_bench = baseline.get("benchmarks", {})
    for name, stats in current.get("benchmarks", {}).items():
        current_ratio = stats.get("molt_cpython_ratio")
        base_ratio = baseline_bench.get(name, {}).get("molt_cpython_ratio")
        if current_ratio is None or base_ratio is None:
            continue
        if current_ratio > base_ratio * (1 + max_regression):
            regressions.append(
                f"{name}: ratio {current_ratio:.4f} > {base_ratio:.4f} * {1 + max_regression:.2f}"
            )
    return regressions


def _summary_path_for_json(json_out: Path, explicit: Path | None) -> Path:
    if explicit is not None:
        return explicit
    if json_out.name == "results.json":
        return json_out.with_name("summary.md")
    return json_out.with_name(f"{json_out.stem}_summary.md")


def _failure_details_path_for_json(json_out: Path) -> Path:
    if json_out.name == "results.json":
        return json_out.with_name("molt_failure_details.jsonl")
    return json_out.with_name(f"{json_out.stem}_molt_failure_details.jsonl")


def _bench_custody_artifacts(
    *,
    json_out: Path,
    summary_out: Path,
    artifact_root: Path,
    failure_details_path: Path,
) -> dict[str, str]:
    memory_guard_root = artifact_root / "memory_guard"
    return {
        "results_json": str(json_out),
        "summary_md": str(summary_out),
        "molt_failure_details_jsonl": str(failure_details_path),
        "harness_command_profile_jsonl": str(memory_guard_root / "commands.jsonl"),
        "repo_process_sentinel_jsonl": str(memory_guard_root / "bench_sentinel.jsonl"),
        "backend_daemon_cleanup_jsonl": str(
            memory_guard_root / "backend_daemon_cleanup.jsonl"
        ),
    }


def _molt_failure_detail_records(
    benchmarks: dict[str, object],
) -> dict[str, object]:
    records: list[dict[str, object]] = []
    total = 0
    for benchmark_name, raw_stats in sorted(benchmarks.items()):
        if not isinstance(raw_stats, dict):
            continue
        raw_failure = raw_stats.get("molt_failure")
        if not isinstance(raw_failure, dict):
            continue
        total += 1
        if len(records) >= MAX_FAILURE_DETAIL_RECORDS:
            continue
        records.append(
            {
                "benchmark": benchmark_name,
                "phase": raw_failure.get("phase"),
                "status": raw_failure.get("status"),
                "detail": raw_failure.get("detail"),
                "returncode": raw_failure.get("returncode"),
                "timed_out": raw_failure.get("timed_out"),
                "elapsed_s": raw_failure.get("elapsed_s"),
                "message": _bounded_failure_text(raw_failure.get("message")),
                "guard_violation": raw_failure.get("guard_violation"),
                "signal": raw_failure.get("signal"),
                "orphaned_process_groups": raw_failure.get("orphaned_process_groups"),
                "log_refs": raw_failure.get("log_refs", []),
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
    failure_details: dict[str, object],
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    records = failure_details.get("records", [])
    if not isinstance(records, list):
        records = []
    with path.open("w", encoding="utf-8") as handle:
        for record in records:
            if isinstance(record, dict):
                handle.write(json.dumps(record, sort_keys=True) + "\n")


def _format_summary_seconds(value: object) -> str:
    if not isinstance(value, (int, float)):
        return "-"
    return f"{float(value):.4f}"


def _render_bench_summary_markdown(payload: dict[str, object]) -> str:
    custody_artifacts = payload.get("custody_artifacts")
    if not isinstance(custody_artifacts, dict):
        custody_artifacts = {}
    failure_details = payload.get("molt_failure_details")
    if not isinstance(failure_details, dict):
        failure_details = {"records": [], "total": 0, "truncated": False}
    records = failure_details.get("records", [])
    if not isinstance(records, list):
        records = []

    lines: list[str] = []
    lines.append("# Molt Benchmark Summary")
    lines.append("")
    lines.append(f"Generated: {payload.get('created_at', '')}")
    if custody_artifacts.get("results_json"):
        lines.append(f"JSON: `{custody_artifacts['results_json']}`")
    lines.append("")
    lines.append(
        "| Benchmark | Molt Status | CPython s | Molt s | Molt/CPython | Failure |"
    )
    lines.append("| --- | --- | ---: | ---: | ---: | --- |")
    benchmarks = payload.get("benchmarks")
    if isinstance(benchmarks, dict):
        for name, raw_stats in sorted(benchmarks.items()):
            if not isinstance(raw_stats, dict):
                continue
            failure = raw_stats.get("molt_failure")
            failure_text = "-"
            if isinstance(failure, dict):
                detail = failure.get("detail")
                failure_text = str(failure.get("status", "failed"))
                if detail:
                    failure_text = f"{failure_text} ({detail})"
            lines.append(
                "| "
                f"{name} | {raw_stats.get('molt_status', 'unknown')} | "
                f"{_format_summary_seconds(raw_stats.get('cpython_time_s'))} | "
                f"{_format_summary_seconds(raw_stats.get('molt_time_s'))} | "
                f"{_format_summary_seconds(raw_stats.get('molt_cpython_ratio'))} | "
                f"{failure_text} |"
            )

    lines.append("")
    lines.append("## Custody Artifacts")
    for key in (
        "molt_failure_details_jsonl",
        "harness_command_profile_jsonl",
        "repo_process_sentinel_jsonl",
        "backend_daemon_cleanup_jsonl",
    ):
        value = custody_artifacts.get(key)
        if value:
            lines.append(f"- `{key}`: `{value}`")

    if records:
        lines.append("")
        lines.append("## Molt Failure Details")
        for record in records:
            if not isinstance(record, dict):
                continue
            detail = record.get("detail")
            detail_text = f" detail=`{detail}`" if detail else ""
            lines.append(
                f"- `{record.get('benchmark')}` phase=`{record.get('phase')}` "
                f"status=`{record.get('status')}`{detail_text}"
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

    lines.append("")
    lines.append("Generated by `tools/bench.py`.")
    return "\n".join(lines) + "\n"


def main():
    _enable_line_buffering()
    parser = argparse.ArgumentParser(description="Run Molt benchmark suite.")
    parser.add_argument("--json-out", type=Path, default=None)
    parser.add_argument("--summary-out", type=Path, default=None)
    parser.add_argument("--baseline", type=Path, default=None)
    parser.add_argument("--update-baseline", action="store_true")
    parser.add_argument("--max-regression", type=float, default=0.15)
    parser.add_argument("--samples", type=int, default=None)
    parser.add_argument(
        "--warmup",
        type=int,
        default=None,
        help="Warmup runs per benchmark before sampling (default: 1, or 0 for --smoke).",
    )
    parser.add_argument("--smoke", action="store_true")
    parser.add_argument(
        "--no-cpython",
        action="store_true",
        help="Skip CPython timing lane (useful when focusing on Molt vs external lanes).",
    )
    parser.add_argument("--no-pypy", action="store_true")
    parser.add_argument("--no-codon", action="store_true")
    parser.add_argument("--no-nuitka", action="store_true")
    parser.add_argument("--no-pyodide", action="store_true")
    parser.add_argument(
        "--nuitka-cmd",
        default=None,
        help=(
            "Override Nuitka command prefix, e.g. 'python -m nuitka'. "
            "Default auto-probes `nuitka` then `python -m nuitka`."
        ),
    )
    parser.add_argument(
        "--pyodide-cmd",
        default=None,
        help=(
            "Pyodide run command prefix (also reads MOLT_BENCH_PYODIDE_CMD). "
            "The command must accept `<script> [args...]`."
        ),
    )
    parser.add_argument(
        "--ws",
        action="store_true",
        help="Include websocket wait benchmark (also honors MOLT_BENCH_WS=1).",
    )
    parser.add_argument(
        "--dynamic-builtin-only",
        action="store_true",
        help=(
            "Run only isolated locals/dir/__import__/delattr benchmark slices; "
            "kept out of core throughput KPI lanes."
        ),
    )
    parser.add_argument(
        "--script",
        action="append",
        help="Benchmark a custom script path (repeatable).",
    )
    parser.add_argument(
        "--super",
        action="store_true",
        help="Run all benchmarks 10x and emit mean/median/variance/range stats.",
    )
    parser.add_argument(
        "--tty",
        action="store_true",
        help="Attach subprocesses to a pseudo-TTY for immediate output.",
    )
    parser.add_argument(
        "--runtime-timeout-sec",
        type=float,
        default=None,
        help="Optional per-run timeout in seconds for each benchmark process.",
    )
    parser.add_argument(
        "--molt-profile",
        choices=["dev", "release"],
        default="release",
        help="Build profile used for Molt benchmark binaries (default: release).",
    )
    parser.add_argument(
        "--no-molt-build-cache",
        action="store_true",
        help=(
            "Disable Molt build-cache reads for a deliberate cold rebuild "
            "investigation. Benchmark runs reuse cache by default."
        ),
    )
    args = parser.parse_args()

    if args.super and args.smoke:
        parser.error("--super cannot be combined with --smoke")
    if args.super and args.samples is not None:
        parser.error("--super cannot be combined with --samples")
    if args.dynamic_builtin_only and args.smoke:
        parser.error("--dynamic-builtin-only cannot be combined with --smoke")

    if args.script:
        if args.smoke:
            parser.error("--script cannot be combined with --smoke")
        if args.dynamic_builtin_only:
            parser.error("--script cannot be combined with --dynamic-builtin-only")
        benchmarks = [str(Path(path)) for path in args.script]
        missing = [path for path in benchmarks if not Path(path).exists()]
        if missing:
            parser.error(f"Script(s) not found: {', '.join(missing)}")
    else:
        if args.dynamic_builtin_only:
            benchmarks = list(DYNAMIC_BUILTIN_SLICES)
        else:
            benchmarks = list(SMOKE_BENCHMARKS) if args.smoke else list(BENCHMARKS)
    include_ws = not args.dynamic_builtin_only and (
        args.ws or os.environ.get("MOLT_BENCH_WS") == "1"
    )
    if include_ws:
        for bench in WS_BENCHMARKS:
            if bench not in benchmarks:
                benchmarks.append(bench)
    samples = (
        SUPER_SAMPLES
        if args.super
        else (args.samples if args.samples is not None else (1 if args.smoke else 3))
    )
    use_cpython = not args.no_cpython
    use_pypy = not args.no_pypy
    use_codon = not args.no_codon
    use_nuitka = not args.no_nuitka
    use_pyodide = not args.no_pyodide
    use_tty = args.tty or os.environ.get("MOLT_TTY") == "1"

    json_out = args.json_out
    if json_out is None:
        timestamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%d_%H%M%S")
        json_out = BENCH_RESULTS_DIR / f"bench_{timestamp}.json"
    json_out = json_out.resolve()
    summary_out = _summary_path_for_json(json_out, args.summary_out)
    summary_out = summary_out.resolve()
    artifact_root = json_out.parent
    failure_details_path = _failure_details_path_for_json(json_out).resolve()
    custody_artifacts = _bench_custody_artifacts(
        json_out=json_out,
        summary_out=summary_out,
        artifact_root=artifact_root,
        failure_details_path=failure_details_path,
    )

    initial_cleanup_env = _canonical_bench_env(_base_python_env())
    initial_cleanup_env.setdefault(
        "MOLT_GUARD_PROFILE_LOG",
        custody_artifacts["harness_command_profile_jsonl"],
    )
    initial_cleanup_env["MOLT_BENCH_DAEMON_CLEANUP_LOG"] = custody_artifacts[
        "backend_daemon_cleanup_jsonl"
    ]
    initial_cleanup_env["MOLT_BENCH_DAEMON_CLEANUP_REASON"] = "bench_start"
    _prune_backend_daemons(initial_cleanup_env)

    warmup = args.warmup if args.warmup is not None else (0 if args.smoke else 1)
    results = bench_results(
        benchmarks,
        samples,
        warmup,
        use_cpython,
        use_pypy,
        use_codon,
        use_nuitka,
        use_pyodide,
        args.super,
        args.runtime_timeout_sec,
        args.molt_profile,
        tty=use_tty,
        nuitka_cmd=args.nuitka_cmd,
        pyodide_cmd=args.pyodide_cmd,
        use_molt_build_cache=not args.no_molt_build_cache,
        artifact_root=artifact_root,
    )

    load_avg = None
    try:
        load_avg = os.getloadavg()
    except (AttributeError, OSError):
        load_avg = None

    failure_details = _molt_failure_detail_records(results)
    # bench.py is a NON-CANONICAL lane (daemon batch builder). Stamp every
    # emitted board so its numbers self-identify and are never cited as the
    # contract - the only citable perf source is perf_scoreboard.py
    # --profile release-fast. The actual cargo profile is recorded honestly:
    # the CLI "release" value maps to the release-fast cargo profile.
    _measured_profile = (
        "release-fast" if args.molt_profile == "release" else args.molt_profile
    )
    payload = {
        "schema_version": 1,
        "created_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "git_rev": _git_rev(),
        "provenance": perf_authority.non_canonical_provenance(
            profile=_measured_profile,
            source="tools/bench.py",
            git_rev=_git_rev(),
        ),
        "super_run": args.super,
        "samples": samples,
        "warmup": warmup,
        "timing_mode": "warm_throughput" if warmup > 0 else "cold_first_run",
        "system": {
            "platform": platform.platform(),
            "python": platform.python_version(),
            "machine": platform.machine(),
            "cpu_count": os.cpu_count(),
            "load_avg": load_avg,
        },
        "memory_guard": harness_memory_guard.limits_summary(
            harness_memory_guard.limits_from_env("MOLT_BENCH")
        ),
        "custody_artifacts": custody_artifacts,
        "molt_failure_details": failure_details,
        "benchmarks": results,
    }

    _write_failure_details_jsonl(failure_details_path, failure_details)
    write_json(json_out, payload)
    summary_out.parent.mkdir(parents=True, exist_ok=True)
    summary_out.write_text(_render_bench_summary_markdown(payload), encoding="utf-8")

    if _has_native_output_parity_failures(payload):
        print(
            f"Native Molt output parity failed; evidence written to {json_out}.",
            file=sys.stderr,
        )
        raise SystemExit(1)

    baseline_path = args.baseline
    if args.update_baseline:
        if baseline_path is None:
            baseline_path = DEFAULT_BASELINE_PATH
        write_json(baseline_path, payload)
        print(f"Baseline updated: {baseline_path}")
        return

    if baseline_path is None or not baseline_path.exists():
        return

    baseline = load_json(baseline_path)
    regressions = compare_baseline(payload, baseline, args.max_regression)
    if regressions:
        print("Performance regression detected:")
        for line in regressions:
            print(f"  - {line}")
        raise SystemExit(1)


if __name__ == "__main__":
    main()
