from __future__ import annotations

import hashlib
import json
import math
import os
import platform
import re
import shutil
import sys
import threading
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, TextIO


ROOT = Path(__file__).resolve().parents[2]
BENCH_DIR = ROOT / "bench"
RESULTS_DIR = BENCH_DIR / "results"
RESULTS_DIR.mkdir(parents=True, exist_ok=True)
TOOLS_ROOT = ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import harness_memory_guard  # noqa: E402
from git_identity import (  # noqa: E402
    clean_checkout_status_arguments,
    is_git_object_id,
    require_git_object_id,
)

BENCH_MEMORY_PREFIX = "MOLT_BENCH"
RUNS_DIRNAME = "demo-runs"
RUN_MANIFEST = "run.json"
RUN_SCHEMA = "molt.demo-run.v1"
SOURCE_SCHEMA = "molt.demo-source.v1"
EVIDENCE_SCOPE = "demo-development-signal"
SCOPE_NOTE = (
    "Scope: development signal against absolute demo budgets; "
    "not CPython-relative or release acceptance evidence."
)
_RUN_ID = re.compile(r"[a-z0-9][a-z0-9-]{7,127}")
_SHA256 = re.compile(r"[0-9a-f]{64}")


def base_env(env: dict[str, str] | None = None) -> dict[str, str]:
    run_env = os.environ.copy()
    overrides = env or {}
    if env:
        run_env.update(env)
    force_default_keys = tuple(
        key
        for key in harness_memory_guard.CANONICAL_ROOT_ENV_KEYS
        if key not in overrides
    )
    return harness_memory_guard.canonical_harness_env(
        run_env,
        repo_root=ROOT,
        force_default_keys=force_default_keys,
    )


def bench_memory_limits(
    env: dict[str, str] | None = None,
) -> harness_memory_guard.HarnessMemoryLimits:
    return harness_memory_guard.limits_from_env(BENCH_MEMORY_PREFIX, base_env(env))


@dataclass
class BenchResult:
    name: str
    req_per_s: float
    p50: float
    p95: float
    p99: float
    p999: float
    error_rate: float
    raw: dict[str, Any]


def read_process_table() -> list[tuple[int, int, float, int, str]]:
    cmd = [
        "ps",
        "-A",
        "-ww",
        "-o",
        "pid=",
        "-o",
        "ppid=",
        "-o",
        "%cpu=",
        "-o",
        "rss=",
        "-o",
        "command=",
    ]
    env = base_env()
    proc = harness_memory_guard.guarded_completed_process(
        cmd,
        prefix=BENCH_MEMORY_PREFIX,
        capture_output=True,
        text=True,
        env=env,
        cwd=ROOT,
        limits=bench_memory_limits(env),
        timeout=5.0,
    )
    if proc.returncode != 0:
        return []
    rows: list[tuple[int, int, float, int, str]] = []
    for line in proc.stdout.splitlines():
        parts = line.strip().split(None, 4)
        if len(parts) < 5:
            continue
        try:
            pid = int(parts[0])
            ppid = int(parts[1])
            cpu = float(parts[2])
            rss = int(float(parts[3]))
        except ValueError:
            continue
        rows.append((pid, ppid, cpu, rss, parts[4]))
    return rows


def run_cmd(cmd: list[str]) -> str | None:
    env = base_env()
    limits = bench_memory_limits(env)
    try:
        proc = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix=BENCH_MEMORY_PREFIX,
            capture_output=True,
            text=True,
            env=env,
            cwd=ROOT,
            limits=limits,
        )
    except OSError:
        return None
    if proc.returncode != 0:
        return None
    output = proc.stdout.strip()
    if not output:
        output = proc.stderr.strip()
    return output or None


def collect_machine_info() -> dict[str, Any]:
    return {
        "platform": platform.platform(),
        "system": platform.system(),
        "release": platform.release(),
        "version": platform.version(),
        "machine": platform.machine(),
        "processor": platform.processor(),
        "cpu_count": os.cpu_count(),
    }


def collect_tool_versions() -> dict[str, str]:
    versions: dict[str, str] = {"python": platform.python_version()}
    k6_version = run_cmd(["k6", "version"])
    if k6_version:
        versions["k6"] = k6_version
    return versions


def extract_proc_matchers(env: dict[str, str]) -> dict[str, dict[str, object]]:
    def parse_env_pid(value: str | None) -> int | None:
        if not value:
            return None
        try:
            return int(value.strip())
        except ValueError:
            return None

    def find_listen_pids(port: int) -> list[int]:
        if not shutil.which("lsof"):
            return []
        cmd = ["lsof", "-n", "-i", f"tcp:{port}", "-sTCP:LISTEN", "-t"]
        limits = bench_memory_limits(env)
        proc = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix=BENCH_MEMORY_PREFIX,
            capture_output=True,
            text=True,
            env=base_env(env),
            cwd=ROOT,
            limits=limits,
        )
        if proc.returncode != 0:
            return []
        pids: list[int] = []
        for line in proc.stdout.splitlines():
            try:
                pids.append(int(line.strip()))
            except ValueError:
                continue
        return pids

    server = env.get("MOLT_SERVER", "auto").lower()
    matchers: dict[str, dict[str, object]] = {
        "worker": {"patterns": ["molt-worker", "molt_worker"]}
    }
    worker_pid = parse_env_pid(env.get("MOLT_DEMO_WORKER_PID"))
    if worker_pid is not None:
        matchers["worker"]["root_pid"] = worker_pid
    if server == "gunicorn":
        matchers["server"] = {"patterns": ["gunicorn"]}
    elif server == "uvicorn":
        matchers["server"] = {"patterns": ["uvicorn"]}
    elif server == "django":
        matchers["server"] = {"patterns": ["manage.py runserver", "runserver"]}
    else:
        matchers["server"] = {"patterns": ["gunicorn", "uvicorn", "runserver"]}
    server_pid = parse_env_pid(env.get("MOLT_DEMO_SERVER_PID"))
    server_root_pids: list[int] = []
    if server_pid is not None:
        server_root_pids.append(server_pid)
    port = parse_env_pid(env.get("MOLT_SERVER_PORT") or "8000")
    if port is not None:
        for pid in find_listen_pids(port):
            if pid not in server_root_pids:
                server_root_pids.append(pid)
    if server_root_pids:
        matchers["server"]["root_pids"] = server_root_pids
    return matchers


def sample_processes(
    matchers: dict[str, dict[str, object]],
) -> dict[str, tuple[float, int, int]]:
    table = read_process_table()
    by_ppid: dict[int, list[int]] = {}
    for pid, ppid, _, _, _ in table:
        by_ppid.setdefault(ppid, []).append(pid)
    samples: dict[str, tuple[float, int, int]] = {}
    for label, selector in matchers.items():
        patterns = selector.get("patterns")
        root_pid = selector.get("root_pid")
        root_pids = selector.get("root_pids")
        target_pids: set[int] = set()
        if isinstance(root_pids, list):
            for candidate in root_pids:
                if not isinstance(candidate, int):
                    continue
                stack = [candidate]
                while stack:
                    current = stack.pop()
                    if current in target_pids:
                        continue
                    target_pids.add(current)
                    stack.extend(by_ppid.get(current, []))
        if isinstance(root_pid, int):
            stack = [root_pid]
            while stack:
                current = stack.pop()
                if current in target_pids:
                    continue
                target_pids.add(current)
                stack.extend(by_ppid.get(current, []))
        pattern_pids: set[int] = set()
        if isinstance(patterns, list):
            for pid, _, _, _, cmd in table:
                if any(pattern in cmd for pattern in patterns):
                    pattern_pids.add(pid)
        selected_pids = target_pids | pattern_pids
        cpu_sum = 0.0
        rss_sum = 0
        proc_count = 0
        for pid, _, cpu, rss, _ in table:
            if not selected_pids or pid not in selected_pids:
                continue
            cpu_sum += cpu
            rss_sum += rss
            proc_count += 1
        samples[label] = (cpu_sum, rss_sum, proc_count)
    return samples


def summarize_proc_samples(
    samples: dict[str, list[tuple[float, int, int]]],
) -> dict[str, dict[str, float]]:
    summaries: dict[str, dict[str, float]] = {}
    for label, values in samples.items():
        if not values:
            continue
        cpu_values = [sample[0] for sample in values]
        rss_values = [sample[1] for sample in values]
        count_values = [sample[2] for sample in values]
        summaries[label] = {
            "samples": float(len(values)),
            "cpu_avg": sum(cpu_values) / len(cpu_values),
            "cpu_max": max(cpu_values),
            "rss_kb_avg": sum(rss_values) / len(rss_values),
            "rss_kb_max": float(max(rss_values)),
            "proc_count_avg": sum(count_values) / len(count_values),
            "proc_count_max": float(max(count_values)),
        }
    return summaries


def summarize_payload_bytes(summary: dict[str, Any]) -> dict[str, float]:
    metrics = summary.get("metrics", {})
    reqs = metrics.get("http_reqs", {}).get("count")
    if not isinstance(reqs, (int, float)) or reqs <= 0:
        return {}
    sent = metrics.get("data_sent", {}).get("count")
    recv = metrics.get("data_received", {}).get("count")
    payload: dict[str, float] = {}
    if isinstance(sent, (int, float)):
        payload["sent_per_req"] = float(sent) / float(reqs)
    if isinstance(recv, (int, float)):
        payload["recv_per_req"] = float(recv) / float(reqs)
    return payload


def tail_lines(path: Path, limit: int = 20) -> list[str]:
    try:
        with path.open("rb") as handle:
            handle.seek(0, os.SEEK_END)
            size = handle.tell()
            data = b""
            while size > 0 and data.count(b"\n") <= limit:
                read_size = min(4096, size)
                size -= read_size
                handle.seek(size)
                data = handle.read(read_size) + data
        return data.decode("utf-8", "ignore").splitlines()[-limit:]
    except OSError:
        return []


def run_k6(
    script: Path, env: dict[str, str]
) -> tuple[dict[str, Any], dict[str, dict[str, float]]]:
    env = base_env(env)
    env.setdefault("K6_SUMMARY_TREND_STATS", "med,p(95),p(99),p(99.9)")
    env.setdefault("K6_LOG_LEVEL", "error")
    cmd = [
        "k6",
        "run",
        "--quiet",
        "--summary-export",
        env["K6_SUMMARY_EXPORT"],
        str(script),
    ]
    stderr_path = RESULTS_DIR / f"k6_{script.stem}_stderr.log"
    matchers = extract_proc_matchers(env)
    samples = {label: [] for label in matchers}
    limits = bench_memory_limits(env)
    stop_sampling = threading.Event()

    def sample_external_processes() -> None:
        while not stop_sampling.is_set():
            if matchers:
                snapshot = sample_processes(matchers)
                for label, sample in snapshot.items():
                    samples[label].append(sample)
            stop_sampling.wait(1.0)

    sampler = threading.Thread(
        target=sample_external_processes,
        name=f"demo-bench-{script.stem}-sampler",
        daemon=False,
    )
    sampler.start()
    try:
        proc = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix=BENCH_MEMORY_PREFIX,
            capture_output=True,
            text=True,
            env=env,
            cwd=ROOT,
            limits=limits,
        )
    finally:
        stop_sampling.set()
        sampler.join()
    with stderr_path.open("w", encoding="utf-8") as handle:
        if proc.stdout:
            handle.write(proc.stdout)
        if proc.stderr:
            handle.write(proc.stderr)
    if proc.returncode != 0:
        tail = tail_lines(stderr_path)
        detail = tail[-1] if tail else f"exit code {proc.returncode}"
        print("\n".join(tail), file=sys.stderr)
        raise SystemExit(
            f"k6 failed for {script} (exit {proc.returncode}): {detail}; "
            f"full output: {stderr_path}; summary: {env['K6_SUMMARY_EXPORT']}"
        )
    # The explicit flag is supported independently of k6 environment aliases.
    summary_path = Path(env["K6_SUMMARY_EXPORT"])
    data = json.loads(summary_path.read_text())
    proc_metrics = summarize_proc_samples(samples)
    return data, proc_metrics


def k6_metric_values(metric: dict[str, Any]) -> dict[str, Any]:
    """Normalize handleSummary values and legacy --summary-export metrics."""
    values = metric.get("values", metric)
    return values if isinstance(values, dict) else {}


def k6_error_rate(metric: dict[str, Any]) -> Any:
    values = k6_metric_values(metric)
    # k6's legacy exporter renames Rate.rate to value, unlike Counter.rate.
    if "rate" in values and "value" in values and values["rate"] != values["value"]:
        return None
    return values.get("rate", values.get("value"))


def k6_p95(durations: dict[str, Any]) -> Any:
    durations = k6_metric_values(durations)
    percentiles = durations.get("percentiles")
    return (
        percentiles.get("95")
        if isinstance(percentiles, dict)
        else durations.get("p(95)")
    )


def parse_k6_summary(name: str, summary: dict[str, Any]) -> BenchResult:
    http = summary.get("metrics", {})
    reqs = k6_metric_values(http.get("http_reqs", {})).get("rate", 0.0)
    durations = k6_metric_values(http.get("http_req_duration", {}))
    durations = k6_metric_values(durations)
    percentiles = durations.get("percentiles")
    if isinstance(percentiles, dict):
        p50 = percentiles.get("50", 0.0)
        p99 = percentiles.get("99", 0.0)
        p999 = percentiles.get("999", 0.0)
    else:
        p50 = durations.get("p(50)", durations.get("med", 0.0))
        p99 = durations.get("p(99)", 0.0)
        p999 = durations.get("p(99.9)", durations.get("p(99.99)", 0.0))
    p95 = k6_p95(durations)
    error_rate = k6_error_rate(http.get("http_req_failed", {}))
    payload_bytes = summarize_payload_bytes(summary)
    if payload_bytes:
        summary["payload_bytes_per_req"] = payload_bytes
    return BenchResult(name, reqs, p50, p95, p99, p999, error_rate, summary)


def check_regressions(artifact: dict[str, Any]) -> list[str]:
    """Validate the actual k6 summary schema and retain the nightly budgets."""
    failures: list[str] = []
    for name, limit in {
        "baseline": 1000.0,
        "offload": 1000.0,
        "offload_table": 1500.0,
    }.items():
        block = artifact.get(name)
        if not isinstance(block, dict):
            failures.append(f"{name}: missing scenario summary")
            continue
        metrics = block.get("metrics")
        if not isinstance(metrics, dict):
            failures.append(f"{name}: missing metrics")
            continue
        required_metrics = ("http_req_duration", "http_reqs", "http_req_failed")
        malformed = [
            key for key in required_metrics if not isinstance(metrics.get(key), dict)
        ]
        if malformed:
            failures.append(
                f"{name}: missing or invalid metrics {', '.join(malformed)}"
            )
            continue
        p95 = k6_p95(metrics["http_req_duration"])
        reqs = k6_metric_values(metrics["http_reqs"]).get("count")
        error_rate = k6_error_rate(metrics["http_req_failed"])
        values = {"p95": p95, "requests": reqs, "error_rate": error_rate}
        invalid = [
            key
            for key, value in values.items()
            if isinstance(value, bool)
            or not isinstance(value, (int, float))
            or not math.isfinite(value)
            or value < 0
        ]
        if invalid:
            failures.append(f"{name}: missing or invalid {', '.join(invalid)}")
            continue
        if reqs <= 0:
            failures.append(f"{name}: no requests completed")
        if error_rate >= 0.01:
            failures.append(f"{name}: error rate {error_rate} >= 0.01")
        if p95 >= limit:
            failures.append(f"{name}: p95 {p95}ms >= {limit}ms")
    return failures


def _git_bytes(*args: str) -> bytes:
    env = base_env()
    proc = harness_memory_guard.guarded_completed_process(
        ["git", *args],
        prefix=BENCH_MEMORY_PREFIX,
        capture_output=True,
        text=False,
        env=env,
        cwd=ROOT,
        limits=bench_memory_limits(env),
        timeout=120.0,
    )
    if proc.returncode != 0 or getattr(proc, "timed_out", False):
        raise ValueError(f"git {' '.join(args)} failed with exit {proc.returncode}")
    return proc.stdout or b""


def capture_source_identity() -> dict[str, object]:
    """Identify the Git-visible source by digests; never record source text."""
    head = _git_bytes("rev-parse", "HEAD").decode("ascii", "replace").strip()
    status = _git_bytes(*clean_checkout_status_arguments(null_terminated=True))
    tracked_diff = _git_bytes("diff", "--binary", "--no-ext-diff", "HEAD", "--")
    return {
        "schema": SOURCE_SCHEMA,
        "head": require_git_object_id(head, label="Demo source HEAD"),
        "dirty": bool(status),
        "status_sha256": hashlib.sha256(status).hexdigest(),
        "tracked_diff_sha256": hashlib.sha256(tracked_diff).hexdigest(),
    }


def _is_source_identity(value: object) -> bool:
    return (
        isinstance(value, dict)
        and value.get("schema") == SOURCE_SCHEMA
        and is_git_object_id(value.get("head"))
        and isinstance(value.get("dirty"), bool)
        and all(
            isinstance(value.get(key), str) and _SHA256.fullmatch(value[key])
            for key in ("status_sha256", "tracked_diff_sha256")
        )
    )


def worker_binary_identity(path_text: str | None) -> dict[str, object] | None:
    """Identify the worker the stack built, when it names one."""
    if not path_text:
        return None
    digest = hashlib.sha256()
    size = 0
    try:
        with Path(path_text).open("rb") as handle:
            while chunk := handle.read(1024 * 1024):
                size += len(chunk)
                digest.update(chunk)
    except OSError:
        return None
    return {"path": path_text, "size": size, "sha256": digest.hexdigest()}


def prepare_run(runs_root: Path, requested: str | None = None) -> Path:
    """Exclusively create one run directory and record its identity before startup.

    The directory is a fresh UUID, or exactly ``requested`` (CI names one per job
    attempt). An existing directory is never reused and nothing is deleted.
    """
    runs_root = runs_root.resolve()
    if requested:
        directory = Path(requested).absolute()
        contained = directory.parent.resolve() == runs_root
        if not contained or not _RUN_ID.fullmatch(directory.name):
            raise ValueError(
                f"Demo run directory must be {runs_root / '<run-id>'} with a "
                f"lowercase [a-z0-9-] run id of 8-128 characters: {requested}"
            )
        directory = runs_root / directory.name
    else:
        directory = runs_root / uuid.uuid4().hex
    # Capture first: unavailable Git fails before any directory exists.
    manifest = {
        "schema": RUN_SCHEMA,
        "run_id": directory.name,
        "created_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "evidence_scope": EVIDENCE_SCOPE,
        "source": capture_source_identity(),
    }
    runs_root.mkdir(parents=True, exist_ok=True)
    try:
        directory.mkdir()
    except FileExistsError as exc:
        raise ValueError(
            f"Demo run directory already exists; runs are never reused: {directory}"
        ) from exc
    with (directory / RUN_MANIFEST).open("x", encoding="utf-8") as handle:
        handle.write(json.dumps(manifest, indent=2) + "\n")
    return directory


def _read_run_file(directory: Path, name: str) -> Any:
    path = directory / name
    if path.is_symlink() or not path.is_file() or path.resolve().parent != directory:
        raise ValueError(
            f"Demo run evidence is not a regular file in {directory}: {name}"
        )
    return json.loads(path.read_text(encoding="utf-8"))


def load_run_manifest(directory: Path) -> tuple[Path, dict[str, Any]]:
    """Validate one explicit run directory against its immutable identity."""
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError(f"Demo run directory must be a real directory: {directory}")
    directory = directory.resolve(strict=True)
    if not (directory / RUN_MANIFEST).exists():
        raise ValueError(
            f"{directory} is not a demo run directory ({RUN_MANIFEST} is missing); "
            "pass the run directory printed by run_stack.sh"
        )
    manifest = _read_run_file(directory, RUN_MANIFEST)
    if not isinstance(manifest, dict) or manifest.get("schema") != RUN_SCHEMA:
        raise ValueError(f"Invalid demo run marker: {directory / RUN_MANIFEST}")
    run_id = manifest.get("run_id")
    if run_id != directory.name or not _RUN_ID.fullmatch(run_id):
        raise ValueError(
            f"Demo run identity {run_id!r} does not match directory {directory.name!r}"
        )
    if not _is_source_identity(manifest.get("source")):
        raise ValueError(f"Demo run {run_id} has no valid source identity")
    return directory, manifest


def check_run_directory(directory: Path) -> tuple[Path, dict[str, Any], list[str]]:
    """Check exactly one explicit run; nothing searches for a latest result."""
    directory, manifest = load_run_manifest(directory)
    run_id = manifest["run_id"]
    composites = sorted(path.name for path in directory.glob("demo_k6_*.json"))
    if len(composites) > 1:
        raise ValueError(
            f"Demo run {run_id} has {len(composites)} composite artifacts; "
            "expected at most one"
        )
    run_failures: list[str] = []
    if composites:
        artifact = _read_run_file(directory, composites[0])
        run = artifact.get("run") if isinstance(artifact, dict) else None
        if (
            not isinstance(run, dict)
            or run.get("run_id") != run_id
            or run.get("source") != manifest["source"]
        ):
            raise ValueError(f"{composites[0]} is not bound to demo run {run_id}")
        if run.get("source_end") != manifest["source"]:
            run_failures.append(
                f"run {run_id}: source identity changed or was unavailable "
                "when the run finished"
            )
    else:
        # A run that stopped early is judged by the summaries it retained.
        artifact = {}
        for name in ("baseline", "offload", "offload_table"):
            summary = f"k6_{name}_summary.json"
            if (directory / summary).exists():
                artifact[name] = _read_run_file(directory, summary)
        run_failures.append(
            f"run {run_id}: incomplete; no composite artifact was published"
        )
    return directory, manifest, check_regressions(artifact) + run_failures


def describe_run(directory: Path, manifest: dict[str, Any]) -> str:
    source = manifest["source"]
    state = "with local changes" if source["dirty"] else "clean"
    run_id = manifest["run_id"]
    return f"demo run {run_id} at {source['head']} ({state}) in {directory}"


def announce_run(directory: Path, *, file: TextIO | None = None) -> None:
    print(f"Demo run {directory.name}: {directory}", file=file)
    print(
        "Check it with: python bench/scripts/run_demo_bench.py "
        f"--check-regressions {directory}",
        file=file,
    )


def bind_run(requested: str | None) -> tuple[Path, dict[str, Any]]:
    """Use the stack's bound run, or bind a fresh one for a direct invocation."""
    try:
        if requested:
            directory, manifest = load_run_manifest(Path(requested))
        else:
            directory = prepare_run(RESULTS_DIR / RUNS_DIRNAME)
            directory, manifest = load_run_manifest(directory)
            announce_run(directory, file=sys.stderr)
    except (OSError, ValueError) as exc:
        raise SystemExit(f"Cannot bind demo run: {exc}") from exc
    existing = sorted(
        path.name
        for path in directory.iterdir()
        if path.name.startswith(("demo_k6_", "k6_"))
    )
    if existing:
        raise SystemExit(
            f"Demo run {directory.name} already has benchmark output "
            f"({', '.join(existing)}); bind a new run instead of reusing it"
        )
    return directory, manifest


def check_cli(target: Path) -> None:
    """Check one explicit run directory, or one historical artifact file."""
    try:
        if target.is_dir():
            directory, manifest, failures = check_run_directory(target)
        else:
            directory = manifest = None
            failures = check_regressions(json.loads(target.read_text()))
    except (OSError, ValueError, TypeError, AttributeError) as exc:
        raise SystemExit(f"Invalid demo performance artifact: {exc}") from exc
    if directory is None or manifest is None:
        if failures:
            raise SystemExit("\n".join(failures))
        print("Perf check OK")
        print(
            f"note: {target} is a historical artifact, not bound to a demo run",
            file=sys.stderr,
        )
        return
    description = describe_run(directory, manifest)
    if failures:
        raise SystemExit("\n".join([f"Perf check FAILED: {description}", *failures]))
    print(f"Perf check OK: {description}")
    print(SCOPE_NOTE)


def run_scenario(name: str, script: str, env: dict[str, str]) -> BenchResult:
    summary_path = RESULTS_DIR / f"k6_{name}_summary.json"
    env = dict(env)
    env["K6_SUMMARY_EXPORT"] = str(summary_path)
    summary, proc_metrics = run_k6(ROOT / script, env)
    result = parse_k6_summary(name, summary)
    result.raw["summary_path"] = str(summary_path)
    if proc_metrics:
        result.raw["proc_metrics"] = proc_metrics
    return result


def percentile(values: list[float], pct: float) -> float:
    if not values:
        return 0.0
    values = sorted(values)
    if len(values) == 1:
        return float(values[0])
    rank = (len(values) - 1) * pct / 100.0
    lower = math.floor(rank)
    upper = math.ceil(rank)
    if lower == upper:
        return float(values[int(rank)])
    weight = rank - lower
    return float(values[lower] + (values[upper] - values[lower]) * weight)


def summarize_worker_metrics(path: Path) -> dict[str, dict[str, float]]:
    def append_metric(
        bucket: dict[str, list[float]],
        payload: dict[str, Any],
        key_ms: str,
        key_us: str,
    ) -> None:
        value = payload.get(key_us)
        if isinstance(value, (int, float)):
            bucket[key_ms].append(float(value) / 1000.0)
            return
        value = payload.get(key_ms)
        if isinstance(value, (int, float)):
            bucket[key_ms].append(float(value))

    by_entry: dict[str, dict[str, list[float]]] = {}
    for line in path.read_text().splitlines():
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        entry = payload.get("entry")
        if not isinstance(entry, str) or not entry:
            continue
        bucket = by_entry.setdefault(
            entry,
            {
                "queue_ms": [],
                "handler_ms": [],
                "exec_ms": [],
                "decode_ms": [],
                "queue_depth": [],
                "payload_bytes": [],
            },
        )
        append_metric(bucket, payload, "queue_ms", "queue_us")
        append_metric(bucket, payload, "handler_ms", "handler_us")
        append_metric(bucket, payload, "exec_ms", "exec_us")
        append_metric(bucket, payload, "decode_ms", "decode_us")
        for key in ("queue_depth", "payload_bytes"):
            value = payload.get(key)
            if isinstance(value, (int, float)):
                bucket[key].append(float(value))

    summary: dict[str, dict[str, float]] = {}
    for entry, metrics in by_entry.items():
        queue_ms = metrics["queue_ms"]
        handler_ms = metrics["handler_ms"]
        exec_ms = metrics["exec_ms"]
        decode_ms = metrics["decode_ms"]
        queue_depth = metrics["queue_depth"]
        payload_bytes = metrics["payload_bytes"]
        summary[entry] = {
            "count": max(len(queue_ms), len(exec_ms)),
            "queue_ms_p50": percentile(queue_ms, 50),
            "queue_ms_p95": percentile(queue_ms, 95),
            "handler_ms_p50": percentile(handler_ms, 50),
            "handler_ms_p95": percentile(handler_ms, 95),
            "exec_ms_p50": percentile(exec_ms, 50),
            "exec_ms_p95": percentile(exec_ms, 95),
            "decode_ms_p50": percentile(decode_ms, 50),
            "decode_ms_p95": percentile(decode_ms, 95),
            "queue_depth_max": float(max(queue_depth) if queue_depth else 0.0),
            "payload_bytes_avg": float(sum(payload_bytes) / len(payload_bytes))
            if payload_bytes
            else 0.0,
            "payload_bytes_p50": percentile(payload_bytes, 50),
            "payload_bytes_p95": percentile(payload_bytes, 95),
        }
    return summary


def main() -> None:
    global RESULTS_DIR
    directory, manifest = bind_run(os.environ.get("MOLT_DEMO_RUN_DIR"))
    RESULTS_DIR = directory
    env = base_env()
    worker_binary = worker_binary_identity(env.get("MOLT_DEMO_WORKER_BIN"))
    limits = bench_memory_limits(env)
    with harness_memory_guard.repo_process_sentinel(
        repo_root=ROOT,
        artifact_root=ROOT / "tmp" / "bench" / "demo",
        label="demo_bench",
        limits=limits,
    ):
        baseline = run_scenario("baseline", "bench/k6/baseline.js", env)
        offload = run_scenario("offload", "bench/k6/offload.js", env)
        offload_table = run_scenario("offload_table", "bench/k6/offload_table.js", env)
    results = [baseline, offload, offload_table]
    # Evidence is source-bound only if the identity recorded at startup still holds.
    try:
        source_end: dict[str, object] | None = capture_source_identity()
    except (OSError, ValueError) as exc:
        print(f"Demo source identity unavailable at run end: {exc}", file=sys.stderr)
        source_end = None
    source_bound = source_end == manifest["source"]

    timestamp = time.strftime("%Y%m%dT%H%M%S", time.gmtime())
    artifact = {
        "timestamp": timestamp,
        "run": {
            "run_id": manifest["run_id"],
            "evidence_scope": EVIDENCE_SCOPE,
            "source": manifest["source"],
            "source_end": source_end,
            "worker_binary": worker_binary,
        },
        "machine": collect_machine_info(),
        "tool_versions": collect_tool_versions(),
        "baseline": baseline.raw,
        "offload": offload.raw,
        "offload_table": offload_table.raw,
    }
    fake_db = {
        "delay_ms": env.get("MOLT_FAKE_DB_DELAY_MS"),
        "decode_us_per_row": env.get("MOLT_FAKE_DB_DECODE_US_PER_ROW"),
        "cpu_iters": env.get("MOLT_FAKE_DB_CPU_ITERS"),
    }
    if any(value for value in fake_db.values() if value is not None):
        artifact["fake_db"] = fake_db
    db_config = {
        "sqlite_path": env.get("MOLT_DB_SQLITE_PATH"),
        "demo_db_path": env.get("MOLT_DEMO_DB_PATH"),
        "sqlite_readwrite": env.get("MOLT_DB_SQLITE_READWRITE"),
    }
    if any(value for value in db_config.values() if value is not None):
        artifact["molt_db"] = db_config
    accel_config = {
        "client_mode": env.get("MOLT_ACCEL_CLIENT_MODE"),
        "pool_size": env.get("MOLT_ACCEL_POOL_SIZE"),
        "wire": env.get("MOLT_WORKER_WIRE") or env.get("MOLT_WIRE"),
    }
    if any(value for value in accel_config.values() if value is not None):
        artifact["molt_accel"] = accel_config
    worker_tuning = {
        "threads": env.get("MOLT_WORKER_THREADS"),
        "max_queue": env.get("MOLT_WORKER_MAX_QUEUE"),
    }
    if any(value for value in worker_tuning.values() if value is not None):
        artifact["molt_worker"] = worker_tuning
    process_context = {
        "server": env.get("MOLT_SERVER"),
        "server_pid": env.get("MOLT_DEMO_SERVER_PID"),
        "worker_pid": env.get("MOLT_DEMO_WORKER_PID"),
    }
    if any(process_context.values()):
        artifact["process_context"] = process_context
    proc_metrics: dict[str, dict[str, dict[str, float]]] = {}
    for result in results:
        metrics = result.raw.get("proc_metrics")
        if isinstance(metrics, dict):
            proc_metrics[result.name] = metrics
    if proc_metrics:
        artifact["process_metrics"] = proc_metrics
    metrics_path = env.get("MOLT_DEMO_METRICS_PATH")
    worker_metrics = None
    if metrics_path:
        path = Path(metrics_path)
        if path.exists():
            worker_metrics = summarize_worker_metrics(path)
            artifact["worker_metrics_path"] = str(path)
            artifact["worker_metrics"] = worker_metrics
    out_path = RESULTS_DIR / f"demo_k6_{timestamp}.json"
    out_path.write_text(json.dumps(artifact, indent=2))
    md_path = RESULTS_DIR / f"demo_k6_{timestamp}.md"
    source_state = "unchanged" if source_bound else "changed or unavailable"
    md_lines = [
        f"# Demo k6 {timestamp}",
        "",
        f"Run: {describe_run(directory, manifest)}",
        "",
        f"Source identity at end: {source_state}",
        "",
        SCOPE_NOTE,
        "",
        "## Summary",
    ]
    for result in results:
        md_lines.append(
            f"- {result.name}: {result.req_per_s:.1f} req/s, "
            f"p50={result.p50:.1f}ms p95={result.p95:.1f}ms, "
            f"errors={result.error_rate * 100:.2f}%"
        )
    payload_rows = []
    for result in results:
        payload = result.raw.get("payload_bytes_per_req")
        if not isinstance(payload, dict):
            continue
        sent = payload.get("sent_per_req")
        recv = payload.get("recv_per_req")
        if isinstance(sent, (int, float)) or isinstance(recv, (int, float)):
            payload_rows.append((result.name, sent, recv))
    if payload_rows:
        md_lines.append("")
        md_lines.append("## Payload bytes per request (k6)")
        md_lines.append("| entry | sent (B) | received (B) |")
        md_lines.append("|---|---:|---:|")
        for entry, sent, recv in payload_rows:
            sent_cell = f"{sent:.1f}" if isinstance(sent, (int, float)) else "-"
            recv_cell = f"{recv:.1f}" if isinstance(recv, (int, float)) else "-"
            md_lines.append(f"| {entry} | {sent_cell} | {recv_cell} |")
    accel_rows = [(key, value) for key, value in accel_config.items() if value]
    if accel_rows:
        md_lines.append("")
        md_lines.append("## Molt accel config")
        for key, value in accel_rows:
            md_lines.append(f"- {key}: {value}")
    db_rows = [(key, value) for key, value in db_config.items() if value]
    if db_rows:
        md_lines.append("")
        md_lines.append("## Molt DB config")
        for key, value in db_rows:
            md_lines.append(f"- {key}: {value}")
    worker_rows = [(key, value) for key, value in worker_tuning.items() if value]
    if worker_rows:
        md_lines.append("")
        md_lines.append("## Molt worker tuning")
        for key, value in worker_rows:
            md_lines.append(f"- {key}: {value}")
    if worker_metrics:
        md_lines.append("")
        md_lines.append("## Worker metrics (molt_accel hooks)")
        md_lines.append(
            "| entry | count | queue_ms p50 | queue_ms p95 | handler_ms p50 | "
            "handler_ms p95 | exec_ms p50 | exec_ms p95 | decode_ms p50 | "
            "decode_ms p95 | queue_depth max | payload avg |"
        )
        md_lines.append("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
        for entry, metrics in sorted(worker_metrics.items()):
            md_lines.append(
                f"| {entry} | {metrics['count']:.0f} | "
                f"{metrics['queue_ms_p50']:.1f} | {metrics['queue_ms_p95']:.1f} | "
                f"{metrics['handler_ms_p50']:.1f} | {metrics['handler_ms_p95']:.1f} | "
                f"{metrics['exec_ms_p50']:.1f} | {metrics['exec_ms_p95']:.1f} | "
                f"{metrics['decode_ms_p50']:.1f} | {metrics['decode_ms_p95']:.1f} | "
                f"{metrics['queue_depth_max']:.0f} | {metrics['payload_bytes_avg']:.1f} |"
            )
    md_lines.append("")
    md_lines.append("## Per-entry metrics")
    md_lines.append(
        "| entry | req/s | p50 (ms) | p95 (ms) | p99 (ms) | p999 (ms) | errors | summary |"
    )
    md_lines.append("|---|---:|---:|---:|---:|---:|---:|---|")
    for result in results:
        summary_path = Path(result.raw.get("summary_path", ""))
        summary_cell = summary_path.name if summary_path else ""
        md_lines.append(
            f"| {result.name} | {result.req_per_s:.1f} | {result.p50:.1f} | "
            f"{result.p95:.1f} | {result.p99:.1f} | {result.p999:.1f} | "
            f"{result.error_rate * 100:.2f}% | {summary_cell} |"
        )
    if proc_metrics:
        md_lines.append("")
        md_lines.append(
            "## Process metrics (CPU avg/max, RSS avg/max KB, process count avg/max)"
        )
        md_lines.append(
            "| scenario | role | cpu_avg | cpu_max | rss_avg_kb | rss_max_kb | "
            "proc_count_avg | proc_count_max | samples |"
        )
        md_lines.append("|---|---|---:|---:|---:|---:|---:|---:|---:|")
        for scenario, metrics in proc_metrics.items():
            for role, stats in metrics.items():
                md_lines.append(
                    f"| {scenario} | {role} | {stats['cpu_avg']:.2f} | "
                    f"{stats['cpu_max']:.2f} | {stats['rss_kb_avg']:.0f} | "
                    f"{stats['rss_kb_max']:.0f} | {stats['proc_count_avg']:.2f} | "
                    f"{stats['proc_count_max']:.0f} | {stats['samples']:.0f} |"
                )
    md_path.write_text("\n".join(md_lines))

    def fmt(result: BenchResult) -> str:
        return (
            f"{result.name}: {result.req_per_s:.1f} req/s, "
            f"p50={result.p50:.1f}ms p95={result.p95:.1f}ms "
            f"p99={result.p99:.1f}ms p999={result.p999:.1f}ms, "
            f"errors={result.error_rate * 100:.2f}%"
        )

    print(fmt(baseline))
    print(fmt(offload))
    print(fmt(offload_table))
    announce_run(directory)
    if not source_bound:
        raise SystemExit(
            f"Demo run {manifest['run_id']} is not source-bound: the Git source "
            "identity changed or was unavailable when the run finished"
        )


if __name__ == "__main__":
    if sys.argv[1:] == ["--prepare-run"]:
        try:
            run_directory = prepare_run(
                RESULTS_DIR / RUNS_DIRNAME, os.environ.get("MOLT_DEMO_RUN_DIR")
            )
        except (OSError, ValueError) as exc:
            raise SystemExit(f"Cannot bind demo run: {exc}") from exc
        announce_run(run_directory, file=sys.stderr)
        print(run_directory)
        raise SystemExit(0)
    if len(sys.argv) == 3 and sys.argv[1] == "--check-regressions":
        check_cli(Path(sys.argv[2]))
        raise SystemExit(0)
    if not shutil.which("k6"):
        raise SystemExit(
            "k6 is required for the demo bench; install from https://k6.io/"
        )
    main()
