from __future__ import annotations

import json
import math
import os
import shutil
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
BENCH_DIR = ROOT / "bench"
RESULTS_DIR = BENCH_DIR / "results"
RESULTS_DIR.mkdir(parents=True, exist_ok=True)


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


def read_process_table() -> list[tuple[int, float, int, str]]:
    cmd = ["ps", "-A", "-o", "pid=", "-o", "%cpu=", "-o", "rss=", "-o", "command="]
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        return []
    rows: list[tuple[int, float, int, str]] = []
    for line in proc.stdout.splitlines():
        parts = line.strip().split(None, 3)
        if len(parts) < 4:
            continue
        try:
            pid = int(parts[0])
            cpu = float(parts[1])
            rss = int(float(parts[2]))
        except ValueError:
            continue
        rows.append((pid, cpu, rss, parts[3]))
    return rows


def extract_proc_matchers(env: dict[str, str]) -> dict[str, list[str]]:
    server = env.get("MOLT_SERVER", "auto").lower()
    matchers: dict[str, list[str]] = {"worker": ["molt-worker", "molt_worker"]}
    if server == "gunicorn":
        matchers["server"] = ["gunicorn"]
    elif server == "uvicorn":
        matchers["server"] = ["uvicorn"]
    elif server == "django":
        matchers["server"] = ["manage.py runserver", "runserver"]
    else:
        matchers["server"] = ["gunicorn", "uvicorn", "runserver"]
    return matchers


def sample_processes(
    matchers: dict[str, list[str]],
) -> dict[str, tuple[float, int, int]]:
    table = read_process_table()
    samples: dict[str, tuple[float, int, int]] = {}
    for label, patterns in matchers.items():
        cpu_sum = 0.0
        rss_sum = 0
        proc_count = 0
        for _, cpu, rss, cmd in table:
            if any(pattern in cmd for pattern in patterns):
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
    env = dict(env)
    env.setdefault("K6_SUMMARY_TREND_STATS", "med,p(95),p(99),p(99.9)")
    env.setdefault("K6_LOG_LEVEL", "error")
    cmd = ["k6", "run", "--quiet", str(script)]
    stderr_path = RESULTS_DIR / f"k6_{script.stem}_stderr.log"
    matchers = extract_proc_matchers(env)
    samples = {label: [] for label in matchers}
    with stderr_path.open("w", encoding="utf-8") as handle:
        proc = subprocess.Popen(cmd, stdout=handle, stderr=handle, text=True, env=env)
        while proc.poll() is None:
            if matchers:
                snapshot = sample_processes(matchers)
                for label, sample in snapshot.items():
                    samples[label].append(sample)
            time.sleep(1.0)
    if proc.returncode != 0:
        tail = tail_lines(stderr_path)
        detail = tail[-1] if tail else f"exit code {proc.returncode}"
        raise SystemExit(f"k6 failed for {script}: {detail}")
    # k6 summary is printed to stderr in JSON when K6_SUMMARY_EXPORT is set
    summary_path = Path(env["K6_SUMMARY_EXPORT"])
    data = json.loads(summary_path.read_text())
    proc_metrics = summarize_proc_samples(samples)
    return data, proc_metrics


def parse_k6_summary(name: str, summary: dict[str, Any]) -> BenchResult:
    http = summary.get("metrics", {})
    reqs = http.get("http_reqs", {}).get("rate", 0.0)
    durations = http.get("http_req_duration", {})
    percentiles = durations.get("percentiles")
    if isinstance(percentiles, dict):
        p50 = percentiles.get("50", 0.0)
        p95 = percentiles.get("95", 0.0)
        p99 = percentiles.get("99", 0.0)
        p999 = percentiles.get("999", 0.0)
    else:
        p50 = durations.get("p(50)", durations.get("med", 0.0))
        p95 = durations.get("p(95)", 0.0)
        p99 = durations.get("p(99)", 0.0)
        p999 = durations.get("p(99.9)", durations.get("p(99.99)", 0.0))
    error_rate = http.get("http_req_failed", {}).get("rate", 0.0)
    return BenchResult(name, reqs, p50, p95, p99, p999, error_rate, summary)


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
            {"queue_ms": [], "exec_ms": [], "queue_depth": []},
        )
        for key in ("queue_ms", "exec_ms", "queue_depth"):
            value = payload.get(key)
            if isinstance(value, (int, float)):
                bucket[key].append(float(value))

    summary: dict[str, dict[str, float]] = {}
    for entry, metrics in by_entry.items():
        queue_ms = metrics["queue_ms"]
        exec_ms = metrics["exec_ms"]
        queue_depth = metrics["queue_depth"]
        summary[entry] = {
            "count": max(len(queue_ms), len(exec_ms)),
            "queue_ms_p50": percentile(queue_ms, 50),
            "queue_ms_p95": percentile(queue_ms, 95),
            "exec_ms_p50": percentile(exec_ms, 50),
            "exec_ms_p95": percentile(exec_ms, 95),
            "queue_depth_max": float(max(queue_depth) if queue_depth else 0.0),
        }
    return summary


def main() -> None:
    env = os.environ.copy()
    baseline = run_scenario("baseline", "bench/k6/baseline.js", env)
    offload = run_scenario("offload", "bench/k6/offload.js", env)
    offload_table = run_scenario("offload_table", "bench/k6/offload_table.js", env)
    results = [baseline, offload, offload_table]

    timestamp = time.strftime("%Y%m%dT%H%M%S", time.gmtime())
    artifact = {
        "timestamp": timestamp,
        "git": os.popen("git rev-parse HEAD").read().strip(),
        "baseline": baseline.raw,
        "offload": offload.raw,
        "offload_table": offload_table.raw,
    }
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
    md_lines = [
        f"# Demo k6 {timestamp}",
        "",
        "## Summary",
    ]
    for result in results:
        md_lines.append(
            f"- {result.name}: {result.req_per_s:.1f} req/s, "
            f"p50={result.p50:.1f}ms p95={result.p95:.1f}ms, "
            f"errors={result.error_rate * 100:.2f}%"
        )
    if worker_metrics:
        md_lines.append("")
        md_lines.append("## Worker metrics (molt_accel hooks)")
        md_lines.append(
            "| entry | count | queue_ms p50 | queue_ms p95 | exec_ms p50 | exec_ms p95 | queue_depth max |"
        )
        md_lines.append("|---|---:|---:|---:|---:|---:|---:|")
        for entry, metrics in sorted(worker_metrics.items()):
            md_lines.append(
                f"| {entry} | {metrics['count']:.0f} | "
                f"{metrics['queue_ms_p50']:.1f} | {metrics['queue_ms_p95']:.1f} | "
                f"{metrics['exec_ms_p50']:.1f} | {metrics['exec_ms_p95']:.1f} | "
                f"{metrics['queue_depth_max']:.0f} |"
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
            "| scenario | role | cpu_avg | cpu_max | rss_avg_kb | rss_max_kb | proc_count_avg | proc_count_max | samples |"
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
            f"p50={result.p50:.1f}ms p95={result.p95:.1f}ms p99={result.p99:.1f}ms p999={result.p999:.1f}ms, "
            f"errors={result.error_rate * 100:.2f}%"
        )

    print(fmt(baseline))
    print(fmt(offload))
    print(fmt(offload_table))


if __name__ == "__main__":
    if not shutil.which("k6"):
        raise SystemExit(
            "k6 is required for the demo bench; install from https://k6.io/"
        )
    main()
