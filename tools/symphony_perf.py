from __future__ import annotations

import argparse
import asyncio
import base64
import hashlib
import json
import os
import shlex
import shutil
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from molt.symphony.paths import (
    resolve_molt_ext_root,
    resolve_symphony_env_file,
    symphony_perf_reports_dir,
)

DEFAULT_ENV_FILE = resolve_symphony_env_file()


@dataclass(slots=True)
class Sample:
    mode: str
    iteration: int
    returncode: int
    duration_s: float
    stdout_tail: str
    stderr_tail: str


@dataclass(slots=True)
class DashboardStateSample:
    iteration: int
    status: int
    latency_ms: float
    had_etag: bool
    state_payload: dict[str, Any] | None = None


def _load_env_file(path: Path) -> dict[str, str]:
    if not path.exists():
        return {}
    loaded: dict[str, str] = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        loaded[key.strip()] = value.strip().strip('"').strip("'")
    return loaded


def _load_dashboard_api_token(
    env_file_values: dict[str, str], env: dict[str, str]
) -> str | None:
    for key in ("MOLT_SYMPHONY_API_TOKEN", "MOLT_SYMPHONY_DASHBOARD_TOKEN"):
        token = str(env.get(key) or env_file_values.get(key) or "").strip()
        if token:
            return token
    token_file_raw = str(
        env.get("MOLT_SYMPHONY_API_TOKEN_FILE")
        or env_file_values.get("MOLT_SYMPHONY_API_TOKEN_FILE")
        or ""
    ).strip()
    if not token_file_raw:
        return None
    token_file = Path(token_file_raw).expanduser()
    try:
        token = token_file.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    return token or None


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Benchmark Symphony launcher modes (python, molt-run, molt-bin) "
            "to identify the best execution path."
        )
    )
    parser.add_argument("workflow_path", nargs="?", default="WORKFLOW.md")
    parser.add_argument(
        "--env-file",
        default=str(DEFAULT_ENV_FILE),
        help="Runtime env file used by tools/symphony_run.py.",
    )
    parser.add_argument(
        "--modes",
        default="python,molt-run,molt-bin",
        help="Comma-separated execution modes.",
    )
    parser.add_argument("--iterations", type=int, default=3)
    parser.add_argument(
        "--skip-mode-runs",
        action="store_true",
        help="Skip launcher-mode runs and collect only dashboard/hash/report analytics.",
    )
    parser.add_argument("--molt-profile", choices=["dev", "release"], default="dev")
    parser.add_argument(
        "--rebuild-each-run",
        action="store_true",
        help="Force rebuild for molt-bin on each sample.",
    )
    parser.add_argument(
        "--output-json",
        default=None,
        help="Optional JSON output path.",
    )
    parser.add_argument(
        "--timeout-seconds",
        type=int,
        default=600,
        help="Per-sample timeout for tools/symphony_run.py execution.",
    )
    parser.add_argument(
        "--dashboard-url",
        default=None,
        help="Optional Symphony dashboard base URL for API polling benchmark (e.g. http://127.0.0.1:8089).",
    )
    parser.add_argument(
        "--api-samples",
        type=int,
        default=40,
        help="Number of /api/v1/state samples to collect when --dashboard-url is set.",
    )
    parser.add_argument(
        "--api-interval-ms",
        type=int,
        default=250,
        help="Delay between /api/v1/state benchmark samples.",
    )
    parser.add_argument(
        "--hash-bench-iterations",
        type=int,
        default=0,
        help="Optional number of state-hash micro-benchmark iterations (0 disables).",
    )
    parser.add_argument(
        "--hash-bench-bytes",
        type=int,
        default=65536,
        help="Payload size in bytes for state-hash benchmark.",
    )
    parser.add_argument(
        "--hash-helper-cmd",
        default="",
        help=(
            "Optional helper command for hash benchmark (e.g. "
            "'/path/to/symphony_state_hasher_bin')."
        ),
    )
    parser.add_argument(
        "--compare-with",
        default="",
        help=(
            "Optional path to a prior symphony_perf JSON report. "
            "When provided, emits current-vs-baseline deltas."
        ),
    )
    parser.add_argument(
        "--auto-compare-latest",
        action="store_true",
        help="Auto-compare against the newest prior report in --reports-dir.",
    )
    parser.add_argument(
        "--reports-dir",
        default="",
        help=(
            "Directory containing symphony_perf_*.json reports. "
            "Defaults to the Symphony log root under Vertigo."
        ),
    )
    parser.add_argument(
        "--keep-reports",
        type=int,
        default=0,
        help="Prune report files in --reports-dir to the latest N after writing output.",
    )
    parser.add_argument(
        "--verdict-json",
        default="",
        help=(
            "Optional path for concise performance verdict output "
            "(status, breaches, baseline, report path)."
        ),
    )
    parser.add_argument(
        "--fail-on-regression",
        action="store_true",
        help="Return nonzero when configured regression thresholds are breached.",
    )
    parser.add_argument(
        "--max-avg-regression-s",
        type=float,
        default=None,
        help="Maximum allowed avg_s increase per mode vs baseline.",
    )
    parser.add_argument(
        "--max-p95-regression-s",
        type=float,
        default=None,
        help="Maximum allowed p95_s increase per mode vs baseline.",
    )
    parser.add_argument(
        "--max-avg-regression-ratio",
        type=float,
        default=None,
        help="Maximum allowed avg_s regression ratio per mode (e.g. 0.15 for +15%).",
    )
    parser.add_argument(
        "--max-p95-regression-ratio",
        type=float,
        default=None,
        help="Maximum allowed p95_s regression ratio per mode (e.g. 0.20 for +20%).",
    )
    parser.add_argument(
        "--max-dashboard-avg-latency-regression-ms",
        type=float,
        default=None,
        help="Maximum allowed dashboard API avg latency regression in ms.",
    )
    parser.add_argument(
        "--max-dashboard-p95-latency-regression-ms",
        type=float,
        default=None,
        help="Maximum allowed dashboard API p95 latency regression in ms.",
    )
    parser.add_argument(
        "--linear-sync-regressions",
        action="store_true",
        help=(
            "Create/update Linear issues for top regression/optimization candidates "
            "when comparison data is available."
        ),
    )
    parser.add_argument(
        "--linear-team",
        default="",
        help="Linear team reference (name/key/id) for regression issue sync.",
    )
    parser.add_argument(
        "--linear-project",
        default="",
        help="Optional Linear project reference for regression issue sync.",
    )
    parser.add_argument(
        "--linear-max-issues",
        type=int,
        default=8,
        help="Maximum regression issues to sync to Linear per run.",
    )
    parser.add_argument(
        "--linear-dry-run",
        action="store_true",
        help="Prepare Linear regression issue plan without mutating Linear state.",
    )
    return parser


def _parse_modes(raw: str) -> list[str]:
    allowed = {"python", "molt-run", "molt-bin"}
    result: list[str] = []
    for part in raw.split(","):
        mode = part.strip()
        if not mode:
            continue
        if mode not in allowed:
            raise RuntimeError(f"Unsupported mode: {mode}")
        if mode not in result:
            result.append(mode)
    if not result:
        raise RuntimeError("No valid modes provided")
    return result


def _ext_env_defaults(env: dict[str, str], ext_root: Path) -> None:
    env.setdefault("MOLT_EXT_ROOT", str(ext_root))
    env.setdefault("CARGO_TARGET_DIR", str(ext_root / "cargo-target"))
    env.setdefault("MOLT_DIFF_CARGO_TARGET_DIR", env["CARGO_TARGET_DIR"])
    env.setdefault("MOLT_CACHE", str(ext_root / "molt_cache"))
    env.setdefault("MOLT_DIFF_ROOT", str(ext_root / "diff"))
    env.setdefault("MOLT_DIFF_TMPDIR", str(ext_root / "tmp"))
    env.setdefault("UV_CACHE_DIR", str(ext_root / "uv-cache"))
    env.setdefault("MOLT_BACKEND_DAEMON_SOCKET_DIR", "/tmp/molt_backend_sockets")
    env.setdefault("TMPDIR", str(ext_root / "tmp"))
    env.setdefault("PYTHONPATH", "src")


def _run_once(
    *,
    mode: str,
    workflow_path: str,
    env_file: str,
    molt_profile: str,
    rebuild_binary: bool,
    env: dict[str, str],
    timeout_seconds: int,
) -> Sample:
    cmd = [
        "uv",
        "run",
        "--python",
        "3.12",
        "python3",
        "tools/symphony_run.py",
        workflow_path,
        "--env-file",
        env_file,
        "--once",
        "--exec-mode",
        mode,
        "--molt-profile",
        molt_profile,
    ]
    if rebuild_binary and mode == "molt-bin":
        cmd.append("--rebuild-binary")

    started = time.perf_counter()
    try:
        proc = subprocess.run(
            cmd,
            check=False,
            env=env,
            capture_output=True,
            text=True,
            timeout=max(int(timeout_seconds), 1),
        )
        duration_s = max(time.perf_counter() - started, 0.0)
        return Sample(
            mode=mode,
            iteration=0,
            returncode=int(proc.returncode),
            duration_s=duration_s,
            stdout_tail=(proc.stdout or "")[-2000:],
            stderr_tail=(proc.stderr or "")[-2000:],
        )
    except subprocess.TimeoutExpired as exc:
        duration_s = max(time.perf_counter() - started, 0.0)
        return Sample(
            mode=mode,
            iteration=0,
            returncode=-1,
            duration_s=duration_s,
            stdout_tail=(exc.stdout or "")[-2000:],
            stderr_tail=(exc.stderr or "timeout")[-2000:],
        )


def _summary(samples: list[Sample]) -> dict[str, Any]:
    grouped: dict[str, list[Sample]] = {}
    for sample in samples:
        grouped.setdefault(sample.mode, []).append(sample)

    report: dict[str, Any] = {}
    for mode, rows in grouped.items():
        success_rows = [item for item in rows if item.returncode == 0]
        durations = [item.duration_s for item in success_rows]
        failures = sum(1 for item in rows if item.returncode != 0)
        entry: dict[str, Any] = {
            "samples": len(rows),
            "successes": len(success_rows),
            "failures": failures,
        }
        if durations:
            entry.update(
                {
                    "min_s": round(min(durations), 4),
                    "max_s": round(max(durations), 4),
                    "avg_s": round(statistics.fmean(durations), 4),
                    "p95_s": round(_p95(durations), 4),
                }
            )
        else:
            entry.update(
                {
                    "min_s": None,
                    "max_s": None,
                    "avg_s": None,
                    "p95_s": None,
                }
            )
        report[mode] = entry
    return report


def _p95(values: list[float]) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    idx = max(int(round((len(ordered) - 1) * 0.95)), 0)
    return ordered[idx]


def _collect_dashboard_state_samples(
    base_url: str,
    *,
    samples: int,
    interval_ms: int,
    timeout_seconds: int,
    auth_token: str | None = None,
) -> list[DashboardStateSample]:
    target = urllib.parse.urljoin(base_url.rstrip("/") + "/", "api/v1/state")
    etag = ""
    rows: list[DashboardStateSample] = []
    total = max(int(samples), 1)
    pause_seconds = max(int(interval_ms), 0) / 1000.0
    for idx in range(total):
        headers = {"Cache-Control": "no-cache"}
        if auth_token:
            headers["Authorization"] = f"Bearer {auth_token}"
        if etag:
            headers["If-None-Match"] = etag
        req = urllib.request.Request(target, headers=headers, method="GET")
        started = time.perf_counter()
        status = 0
        had_etag = False
        state_payload: dict[str, Any] | None = None
        try:
            with urllib.request.urlopen(
                req, timeout=max(int(timeout_seconds), 1)
            ) as resp:
                status = int(resp.status)
                response_etag = (resp.headers.get("ETag") or "").strip()
                if response_etag:
                    etag = response_etag
                    had_etag = True
                body = resp.read()
                if status == 200:
                    state_payload = _decode_state_payload(body)
        except urllib.error.HTTPError as exc:
            status = int(exc.code)
            response_etag = (
                (exc.headers.get("ETag") or "").strip() if exc.headers else ""
            )
            if response_etag:
                etag = response_etag
                had_etag = True
            _ = exc.read()
        except urllib.error.URLError:
            status = -1
        latency_ms = max((time.perf_counter() - started) * 1000.0, 0.0)
        rows.append(
            DashboardStateSample(
                iteration=idx + 1,
                status=status,
                latency_ms=latency_ms,
                had_etag=had_etag,
                state_payload=state_payload,
            )
        )
        if pause_seconds > 0 and idx + 1 < total:
            time.sleep(pause_seconds)
    return rows


def _decode_state_payload(body: bytes) -> dict[str, Any] | None:
    try:
        parsed = json.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return None
    return parsed if isinstance(parsed, dict) else None


def _summarize_dashboard_state_samples(
    rows: list[DashboardStateSample],
) -> dict[str, Any]:
    if not rows:
        return {
            "samples": 0,
            "status_200": 0,
            "status_304": 0,
            "errors": 0,
            "etag_seen": 0,
            "avg_latency_ms": None,
            "p95_latency_ms": None,
        }
    latencies = [row.latency_ms for row in rows if row.status >= 0]
    ok_200 = sum(1 for row in rows if row.status == 200)
    not_modified = sum(1 for row in rows if row.status == 304)
    errors = sum(1 for row in rows if row.status < 0 or row.status >= 400)
    return {
        "samples": len(rows),
        "status_200": ok_200,
        "status_304": not_modified,
        "errors": errors,
        "etag_seen": sum(1 for row in rows if row.had_etag),
        "avg_latency_ms": round(statistics.fmean(latencies), 3) if latencies else None,
        "p95_latency_ms": round(_p95(latencies), 3) if latencies else None,
    }


def _bench_python_hash(*, payload: bytes, iterations: int) -> dict[str, Any]:
    started = time.perf_counter()
    for _ in range(max(iterations, 1)):
        _ = hashlib.blake2s(payload, digest_size=8).hexdigest()
    elapsed = max(time.perf_counter() - started, 1e-9)
    bytes_total = len(payload) * max(iterations, 1)
    return {
        "mode": "python_blake2s",
        "iterations": max(iterations, 1),
        "payload_bytes": len(payload),
        "elapsed_s": round(elapsed, 6),
        "hashes_per_second": round(max(iterations, 1) / elapsed, 2),
        "throughput_mb_s": round((bytes_total / elapsed) / (1024 * 1024), 2),
    }


def _bench_helper_hash(
    *, payload: bytes, iterations: int, helper_cmd: str
) -> dict[str, Any]:
    command = shlex.split(helper_cmd.strip())
    if not command:
        return {"mode": "helper", "error": "empty_helper_command"}
    has_stdio_text = "--stdio" in command
    has_stdio_frame = "--stdio-frame" in command
    if has_stdio_text and has_stdio_frame:
        return {
            "mode": "helper",
            "error": "conflicting_helper_modes",
            "command": command,
        }
    if not has_stdio_text and not has_stdio_frame:
        command.append("--stdio")
        has_stdio_text = True
    payload_b64 = base64.b64encode(payload).decode("ascii")
    try:
        proc = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=has_stdio_text,
            encoding="utf-8" if has_stdio_text else None,
            bufsize=1 if has_stdio_text else 0,
        )
    except OSError as exc:
        return {"mode": "helper", "error": "spawn_failed", "message": str(exc)}

    started = time.perf_counter()
    error: str | None = None
    requested_iterations = max(iterations, 1)
    completed_iterations = 0
    try:
        if proc.stdin is None or proc.stdout is None:
            error = "pipe_unavailable"
        else:
            for _ in range(requested_iterations):
                if has_stdio_frame:
                    proc.stdin.write(len(payload).to_bytes(4, "big", signed=False))
                    proc.stdin.write(payload)
                    proc.stdin.flush()
                    digest = proc.stdout.read(8)
                    if not isinstance(digest, (bytes, bytearray)) or len(digest) != 8:
                        error = "invalid_helper_output"
                        break
                    completed_iterations += 1
                else:
                    proc.stdin.write(payload_b64 + "\n")
                    proc.stdin.flush()
                    line = proc.stdout.readline()
                    etag = line.strip()
                    if not (etag.startswith('W/"') and etag.endswith('"')):
                        error = "invalid_helper_output"
                        break
                    completed_iterations += 1
    except OSError as exc:
        error = f"io_error:{exc}"
    elapsed = max(time.perf_counter() - started, 1e-9)
    try:
        if proc.stdin is not None:
            proc.stdin.close()
    except OSError:
        pass
    try:
        proc.terminate()
    except OSError:
        pass
    try:
        proc.wait(timeout=0.5)
    except (subprocess.TimeoutExpired, OSError):
        try:
            proc.kill()
        except OSError:
            pass

    bytes_total = len(payload) * completed_iterations
    report: dict[str, Any] = {
        "mode": "helper_framed" if has_stdio_frame else "helper_stdio",
        "iterations": requested_iterations,
        "iterations_completed": completed_iterations,
        "payload_bytes": len(payload),
        "elapsed_s": round(elapsed, 6),
        "hashes_per_second": round(completed_iterations / elapsed, 2),
        "throughput_mb_s": round((bytes_total / elapsed) / (1024 * 1024), 2),
        "command": command,
    }
    if error is not None:
        report["error"] = error
    return report


def _compare_reports(
    current: dict[str, Any], baseline: dict[str, Any]
) -> dict[str, Any]:
    current_summary = current.get("summary")
    baseline_summary = baseline.get("summary")
    current_modes = current_summary if isinstance(current_summary, dict) else {}
    baseline_modes = baseline_summary if isinstance(baseline_summary, dict) else {}
    mode_comparison: dict[str, Any] = {}
    for mode in sorted(set(current_modes) | set(baseline_modes)):
        current_row = current_modes.get(mode)
        baseline_row = baseline_modes.get(mode)
        if not isinstance(current_row, dict) or not isinstance(baseline_row, dict):
            continue
        current_avg = _num_or_none(current_row.get("avg_s"))
        baseline_avg = _num_or_none(baseline_row.get("avg_s"))
        current_p95 = _num_or_none(current_row.get("p95_s"))
        baseline_p95 = _num_or_none(baseline_row.get("p95_s"))
        mode_comparison[mode] = {
            "current_avg_s": current_avg,
            "baseline_avg_s": baseline_avg,
            "avg_delta_s": _delta(current_avg, baseline_avg),
            "avg_delta_ratio": _delta_ratio(current_avg, baseline_avg),
            "current_p95_s": current_p95,
            "baseline_p95_s": baseline_p95,
            "p95_delta_s": _delta(current_p95, baseline_p95),
            "p95_delta_ratio": _delta_ratio(current_p95, baseline_p95),
        }
    current_dashboard = current.get("dashboard_state_api")
    baseline_dashboard = baseline.get("dashboard_state_api")
    dashboard_comparison: dict[str, Any] | None = None
    if isinstance(current_dashboard, dict) and isinstance(baseline_dashboard, dict):
        dashboard_comparison = {
            "current_avg_latency_ms": _num_or_none(
                current_dashboard.get("avg_latency_ms")
            ),
            "baseline_avg_latency_ms": _num_or_none(
                baseline_dashboard.get("avg_latency_ms")
            ),
            "avg_latency_delta_ms": _delta(
                _num_or_none(current_dashboard.get("avg_latency_ms")),
                _num_or_none(baseline_dashboard.get("avg_latency_ms")),
            ),
            "current_p95_latency_ms": _num_or_none(
                current_dashboard.get("p95_latency_ms")
            ),
            "baseline_p95_latency_ms": _num_or_none(
                baseline_dashboard.get("p95_latency_ms")
            ),
            "p95_latency_delta_ms": _delta(
                _num_or_none(current_dashboard.get("p95_latency_ms")),
                _num_or_none(baseline_dashboard.get("p95_latency_ms")),
            ),
        }
    return {
        "baseline_report": baseline.get("generated_at"),
        "mode_comparison": mode_comparison,
        "dashboard_state_api_comparison": dashboard_comparison,
    }


def _num_or_none(value: Any) -> float | None:
    try:
        parsed = float(value)
    except (TypeError, ValueError):
        return None
    if parsed != parsed or parsed in {float("inf"), float("-inf")}:
        return None
    return parsed


def _delta(current: float | None, baseline: float | None) -> float | None:
    if current is None or baseline is None:
        return None
    return round(current - baseline, 4)


def _delta_ratio(current: float | None, baseline: float | None) -> float | None:
    if current is None or baseline is None or baseline == 0:
        return None
    return round((current - baseline) / baseline, 4)


def _collect_regression_breaches(
    comparison: dict[str, Any],
    *,
    max_avg_regression_s: float | None,
    max_p95_regression_s: float | None,
    max_avg_regression_ratio: float | None,
    max_p95_regression_ratio: float | None,
    max_dashboard_avg_latency_regression_ms: float | None,
    max_dashboard_p95_latency_regression_ms: float | None,
) -> list[dict[str, Any]]:
    breaches: list[dict[str, Any]] = []
    mode_rows = comparison.get("mode_comparison")
    if isinstance(mode_rows, dict):
        for mode, raw_row in mode_rows.items():
            row = raw_row if isinstance(raw_row, dict) else {}
            _maybe_add_breach(
                breaches,
                scope=f"mode:{mode}",
                metric="avg_delta_s",
                observed=_num_or_none(row.get("avg_delta_s")),
                threshold=max_avg_regression_s,
            )
            _maybe_add_breach(
                breaches,
                scope=f"mode:{mode}",
                metric="p95_delta_s",
                observed=_num_or_none(row.get("p95_delta_s")),
                threshold=max_p95_regression_s,
            )
            _maybe_add_breach(
                breaches,
                scope=f"mode:{mode}",
                metric="avg_delta_ratio",
                observed=_num_or_none(row.get("avg_delta_ratio")),
                threshold=max_avg_regression_ratio,
            )
            _maybe_add_breach(
                breaches,
                scope=f"mode:{mode}",
                metric="p95_delta_ratio",
                observed=_num_or_none(row.get("p95_delta_ratio")),
                threshold=max_p95_regression_ratio,
            )
    dashboard_row = comparison.get("dashboard_state_api_comparison")
    if isinstance(dashboard_row, dict):
        _maybe_add_breach(
            breaches,
            scope="dashboard_state_api",
            metric="avg_latency_delta_ms",
            observed=_num_or_none(dashboard_row.get("avg_latency_delta_ms")),
            threshold=max_dashboard_avg_latency_regression_ms,
        )
        _maybe_add_breach(
            breaches,
            scope="dashboard_state_api",
            metric="p95_latency_delta_ms",
            observed=_num_or_none(dashboard_row.get("p95_latency_delta_ms")),
            threshold=max_dashboard_p95_latency_regression_ms,
        )
    return breaches


def _maybe_add_breach(
    breaches: list[dict[str, Any]],
    *,
    scope: str,
    metric: str,
    observed: float | None,
    threshold: float | None,
) -> None:
    if threshold is None or observed is None:
        return
    if observed <= threshold:
        return
    breaches.append(
        {
            "scope": scope,
            "metric": metric,
            "observed": round(observed, 4),
            "threshold": round(float(threshold), 4),
            "delta_over_budget": round(observed - float(threshold), 4),
        }
    )


def _default_reports_dir(ext_root: Path) -> Path:
    del ext_root
    return symphony_perf_reports_dir()


def _resolve_reports_dir(raw: str, *, ext_root: Path) -> Path:
    text = str(raw or "").strip()
    if text:
        return Path(text).expanduser()
    return _default_reports_dir(ext_root)


def _list_reports(reports_dir: Path) -> list[Path]:
    if not reports_dir.exists():
        return []
    return sorted(
        [path for path in reports_dir.glob("symphony_perf_*.json") if path.is_file()],
        key=lambda path: path.stat().st_mtime_ns,
    )


def _discover_latest_baseline_report(
    reports_dir: Path, *, exclude: Sequence[Path] | None = None
) -> Path | None:
    excluded = {path.resolve() for path in (exclude or [])}
    for candidate in reversed(_list_reports(reports_dir)):
        resolved = candidate.resolve()
        if resolved in excluded:
            continue
        return candidate
    return None


def _prune_reports(reports_dir: Path, *, keep_reports: int) -> list[str]:
    keep = max(int(keep_reports), 0)
    if keep <= 0:
        return []
    reports = _list_reports(reports_dir)
    if len(reports) <= keep:
        return []
    removed: list[str] = []
    for candidate in reports[: len(reports) - keep]:
        try:
            candidate.unlink()
            removed.append(str(candidate))
        except OSError:
            continue
    return removed


def _latest_state_payload(
    rows: list[DashboardStateSample],
) -> dict[str, Any] | None:
    for row in reversed(rows):
        if row.status == 200 and isinstance(row.state_payload, dict):
            return row.state_payload
    return None


def _candidate_component(label: str) -> tuple[str, str]:
    text = label.lower()
    if any(
        token in text
        for token in ("dispatch", "retry", "worker", "orchestrator", "event_handler")
    ):
        return ("symphony-orchestrator", "executor")
    if any(
        token in text for token in ("dashboard", "stream", "http", "state_hash", "sse")
    ):
        return ("symphony-dashboard", "executor")
    if any(
        token in text for token in ("molt", "runtime", "backend", "intrinsic", "build")
    ):
        return ("molt-runtime", "executor")
    return ("symphony-core", "executor")


def _build_regression_candidates(
    comparison: dict[str, Any] | None,
    dashboard_rows: list[DashboardStateSample],
    *,
    limit: int,
) -> list[dict[str, Any]]:
    candidates: list[dict[str, Any]] = []
    seen_titles: set[str] = set()

    if isinstance(comparison, dict):
        mode_rows = comparison.get("mode_comparison")
        if isinstance(mode_rows, dict):
            for mode, raw_row in mode_rows.items():
                row = raw_row if isinstance(raw_row, dict) else {}
                avg_delta = _num_or_none(row.get("avg_delta_s")) or 0.0
                p95_delta = _num_or_none(row.get("p95_delta_s")) or 0.0
                if avg_delta <= 0 and p95_delta <= 0:
                    continue
                title = f"Perf Regression: mode {mode}"
                if title in seen_titles:
                    continue
                seen_titles.add(title)
                component, owner = _candidate_component(str(mode))
                candidates.append(
                    {
                        "title": title,
                        "priority": 1,
                        "component": component,
                        "owner_hint": owner,
                        "score": max(avg_delta, p95_delta),
                        "description": (
                            "Symphony perf comparison detected launcher-mode regression.\n\n"
                            f"- Mode: `{mode}`\n"
                            f"- avg delta (s): `{avg_delta}`\n"
                            f"- p95 delta (s): `{p95_delta}`\n"
                            f"- avg delta ratio: `{row.get('avg_delta_ratio')}`\n"
                            f"- p95 delta ratio: `{row.get('p95_delta_ratio')}`\n"
                            "Please investigate hotspot attribution and optimize."
                        ),
                    }
                )
    latest_state = _latest_state_payload(dashboard_rows)
    profiling_compare = (
        latest_state.get("profiling_compare")
        if isinstance(latest_state, dict)
        else None
    )
    regressions = (
        profiling_compare.get("regressions")
        if isinstance(profiling_compare, dict)
        else None
    )
    optimizations = (
        profiling_compare.get("optimizations")
        if isinstance(profiling_compare, dict)
        else None
    )
    for row_value in list(regressions or []) + list(optimizations or []):
        row = row_value if isinstance(row_value, dict) else {}
        label = str(row.get("label") or "").strip()
        if not label:
            continue
        title = f"Perf Hotspot: {label}"
        if title in seen_titles:
            continue
        seen_titles.add(title)
        component, owner = _candidate_component(label)
        score = max(
            _num_or_none(row.get("impact_ms")) or 0.0,
            _num_or_none(row.get("priority_score")) or 0.0,
            _num_or_none(row.get("avg_delta_ms")) or 0.0,
            _num_or_none(row.get("p95_delta_ms")) or 0.0,
        )
        candidates.append(
            {
                "title": title,
                "priority": 1,
                "component": component,
                "owner_hint": owner,
                "score": score,
                "description": (
                    "Symphony profiler identified hotspot regression candidate.\n\n"
                    f"- Label: `{label}`\n"
                    f"- Reason: `{row.get('reason')}`\n"
                    f"- avg delta (ms): `{row.get('avg_delta_ms')}`\n"
                    f"- p95 delta (ms): `{row.get('p95_delta_ms')}`\n"
                    f"- Samples: `{row.get('samples')}`\n"
                    f"- Recent samples: `{row.get('recent_samples')}`\n"
                    f"- Priority score: `{row.get('priority_score')}`\n"
                    "Attach profiler data and lower validated hotspots."
                ),
            }
        )

    candidates.sort(key=lambda row: float(row.get("score") or 0.0), reverse=True)
    return candidates[: max(int(limit), 1)]


def _sync_linear_regression_issues(
    *,
    team: str,
    project: str | None,
    candidates: list[dict[str, Any]],
    dry_run: bool,
) -> dict[str, Any]:
    import tools.linear_workspace as linear_workspace

    if not candidates:
        return {
            "ok": True,
            "planned": 0,
            "result": {"created_count": 0, "updated_count": 0},
        }
    team_id = linear_workspace._resolve_team_id(team)
    project_id = linear_workspace._resolve_project_id(team_id, project)
    existing = linear_workspace._fetch_issues(team_id, project_id)
    desired = [
        linear_workspace.DesiredIssue(
            title=str(candidate["title"]),
            description=str(candidate["description"]),
            priority=int(candidate.get("priority") or 1),
        )
        for candidate in candidates
    ]
    plan = linear_workspace._build_sync_plan(
        desired=desired,
        existing=existing,
        update_existing=True,
        close_duplicates=False,
        duplicate_state_id=None,
    )
    payload: dict[str, Any] = {
        "ok": True,
        "team": team,
        "project": project,
        "planned": {
            "create": len(plan.creates),
            "update": len(plan.updates),
            "existing_skipped": plan.existing_skipped,
        },
        "dry_run": bool(dry_run),
    }
    if dry_run:
        payload["sample_titles"] = [item.title for item in plan.creates[:10]]
        return payload
    outcome = asyncio.run(
        linear_workspace._execute_sync_plan(
            plan=plan,
            team_id=team_id,
            project_id=project_id,
            duplicate_state_id=None,
            concurrency=4,
        )
    )
    created = [item for item in outcome.get("created", []) if isinstance(item, dict)]
    updated = [item for item in outcome.get("updated", []) if isinstance(item, dict)]
    payload["result"] = {
        "created_count": len(created),
        "updated_count": len(updated),
        "error_count": len(outcome.get("errors", [])),
        "created": created,
        "updated": updated,
        "errors": list(outcome.get("errors", [])),
    }
    return payload


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    env_file = Path(args.env_file).expanduser()
    if not env_file.is_absolute():
        env_file = (Path.cwd() / env_file).resolve()
    env = os.environ.copy()
    env_file_values = _load_env_file(env_file)
    for key, value in env_file_values.items():
        env.setdefault(key, value)
    ext_root = resolve_molt_ext_root(env)
    if not ext_root.exists():
        raise RuntimeError(f"External root not mounted: {ext_root}")
    modes = _parse_modes(args.modes)
    reports_dir = _resolve_reports_dir(args.reports_dir, ext_root=ext_root)
    reports_dir.mkdir(parents=True, exist_ok=True)

    _ext_env_defaults(env, ext_root)
    dashboard_api_token = _load_dashboard_api_token(env_file_values, env)
    if not shutil.which("uv"):
        raise RuntimeError("uv is required for symphony_perf")

    output_path: Path
    if args.output_json:
        output_path = Path(args.output_json).expanduser()
    else:
        stamp = datetime.now(UTC).strftime("%Y%m%d_%H%M%S")
        output_path = reports_dir / f"symphony_perf_{stamp}.json"
    output_path.parent.mkdir(parents=True, exist_ok=True)

    samples: list[Sample] = []
    if not args.skip_mode_runs:
        for mode in modes:
            for idx in range(max(args.iterations, 1)):
                sample = _run_once(
                    mode=mode,
                    workflow_path=args.workflow_path,
                    env_file=str(args.env_file),
                    molt_profile=args.molt_profile,
                    rebuild_binary=bool(args.rebuild_each_run),
                    env=env,
                    timeout_seconds=args.timeout_seconds,
                )
                sample.iteration = idx + 1
                samples.append(sample)
                print(
                    f"{mode} iter={sample.iteration} rc={sample.returncode} "
                    f"duration_s={sample.duration_s:.3f}"
                )

    summary = _summary(samples)
    print(json.dumps(summary, indent=2))

    dashboard_report: dict[str, Any] | None = None
    dashboard_samples: list[DashboardStateSample] = []
    if args.dashboard_url:
        dashboard_samples = _collect_dashboard_state_samples(
            args.dashboard_url,
            samples=max(args.api_samples, 1),
            interval_ms=max(args.api_interval_ms, 0),
            timeout_seconds=max(args.timeout_seconds, 1),
            auth_token=dashboard_api_token,
        )
        dashboard_report = _summarize_dashboard_state_samples(dashboard_samples)
        print(
            "dashboard_state_api "
            f"samples={dashboard_report['samples']} "
            f"200={dashboard_report['status_200']} "
            f"304={dashboard_report['status_304']} "
            f"errors={dashboard_report['errors']} "
            f"avg_latency_ms={dashboard_report['avg_latency_ms']}"
        )

    hash_bench: dict[str, Any] | None = None
    if int(args.hash_bench_iterations) > 0:
        payload_size = max(int(args.hash_bench_bytes), 256)
        payload = bytes((idx * 17) % 251 for idx in range(payload_size))
        python_hash = _bench_python_hash(
            payload=payload, iterations=max(int(args.hash_bench_iterations), 1)
        )
        helper_hash: dict[str, Any] | None = None
        if str(args.hash_helper_cmd or "").strip():
            helper_hash = _bench_helper_hash(
                payload=payload,
                iterations=max(int(args.hash_bench_iterations), 1),
                helper_cmd=str(args.hash_helper_cmd),
            )
        hash_bench = {"python": python_hash, "helper": helper_hash}
        print(
            "hash_bench "
            f"python_hps={python_hash['hashes_per_second']} "
            f"helper_hps={(helper_hash or {}).get('hashes_per_second')}"
        )

    comparison: dict[str, Any] | None = None
    regression_breaches: list[dict[str, Any]] = []
    linear_sync: dict[str, Any] | None = None
    compare_with = str(args.compare_with or "").strip()
    if not compare_with and bool(args.auto_compare_latest):
        baseline_path_auto = _discover_latest_baseline_report(
            reports_dir,
            exclude=(output_path,),
        )
        if baseline_path_auto is not None:
            compare_with = str(baseline_path_auto)
    if args.fail_on_regression and not compare_with:
        raise RuntimeError(
            "--fail-on-regression requires --compare-with or --auto-compare-latest."
        )
    if compare_with:
        baseline_path = Path(compare_with).expanduser()
        baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
        if not isinstance(baseline, dict):
            raise RuntimeError(f"Invalid baseline report: {baseline_path}")
        current_payload = {
            "summary": summary,
            "dashboard_state_api": dashboard_report,
            "generated_at": datetime.now(UTC).isoformat().replace("+00:00", "Z"),
        }
        comparison = _compare_reports(current_payload, baseline)
        print(json.dumps({"comparison": comparison}, indent=2))
        regression_breaches = _collect_regression_breaches(
            comparison,
            max_avg_regression_s=args.max_avg_regression_s,
            max_p95_regression_s=args.max_p95_regression_s,
            max_avg_regression_ratio=args.max_avg_regression_ratio,
            max_p95_regression_ratio=args.max_p95_regression_ratio,
            max_dashboard_avg_latency_regression_ms=args.max_dashboard_avg_latency_regression_ms,
            max_dashboard_p95_latency_regression_ms=args.max_dashboard_p95_latency_regression_ms,
        )
        if regression_breaches:
            print(json.dumps({"regression_breaches": regression_breaches}, indent=2))
    if args.linear_sync_regressions:
        team = str(args.linear_team or "").strip()
        if not team:
            raise RuntimeError("--linear-sync-regressions requires --linear-team.")
        candidates = _build_regression_candidates(
            comparison,
            dashboard_samples,
            limit=max(int(args.linear_max_issues), 1),
        )
        linear_sync = _sync_linear_regression_issues(
            team=team,
            project=(str(args.linear_project).strip() or None),
            candidates=candidates,
            dry_run=bool(args.linear_dry_run),
        )
        print(json.dumps({"linear_sync": linear_sync}, indent=2))

    report_payload = {
        "generated_at": datetime.now(UTC).isoformat().replace("+00:00", "Z"),
        "workflow_path": args.workflow_path,
        "modes": modes,
        "iterations": int(args.iterations),
        "skip_mode_runs": bool(args.skip_mode_runs),
        "reports_dir": str(reports_dir),
        "summary": summary,
        "dashboard_state_api": dashboard_report,
        "comparison": comparison,
        "compare_with": compare_with or None,
        "regression_breaches": regression_breaches,
        "linear_sync": linear_sync,
        "hash_bench": hash_bench,
        "samples": [
            {
                "mode": s.mode,
                "iteration": s.iteration,
                "returncode": s.returncode,
                "duration_s": round(s.duration_s, 6),
                "stdout_tail": s.stdout_tail,
                "stderr_tail": s.stderr_tail,
            }
            for s in samples
        ],
        "dashboard_samples": [
            {
                "iteration": row.iteration,
                "status": row.status,
                "latency_ms": round(row.latency_ms, 3),
                "had_etag": row.had_etag,
            }
            for row in dashboard_samples
        ],
    }
    output_path.write_text(
        json.dumps(report_payload, indent=2),
        encoding="utf-8",
    )
    print(f"wrote {output_path}")
    pruned_reports = _prune_reports(
        reports_dir,
        keep_reports=max(int(args.keep_reports), 0),
    )
    if pruned_reports:
        print(json.dumps({"pruned_reports": pruned_reports}, indent=2))

    any_failure = any(sample.returncode != 0 for sample in samples)
    exit_code = 0
    if any_failure:
        exit_code = 2
    elif args.fail_on_regression and regression_breaches:
        exit_code = 3

    verdict_path_raw = str(args.verdict_json or "").strip()
    verdict_path = (
        Path(verdict_path_raw).expanduser()
        if verdict_path_raw
        else reports_dir / "perf" / "verdict.json"
    )
    verdict_path.parent.mkdir(parents=True, exist_ok=True)
    verdict = {
        "generated_at": datetime.now(UTC).isoformat().replace("+00:00", "Z"),
        "status": (
            "run_failed" if exit_code == 2 else ("breach" if exit_code == 3 else "pass")
        ),
        "exit_code": exit_code,
        "report_path": str(output_path),
        "compare_with": compare_with or None,
        "breaches_count": len(regression_breaches),
        "regression_breaches": regression_breaches,
        "dashboard_state_api": dashboard_report,
        "linear_sync": linear_sync,
    }
    verdict_path.write_text(json.dumps(verdict, indent=2) + "\n", encoding="utf-8")
    print(f"verdict {verdict_path}")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
