from __future__ import annotations

import argparse
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
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any


DEFAULT_EXT_ROOT = Path("/Volumes/APDataStore/Molt")
DEFAULT_ENV_FILE = Path("ops/linear/runtime/symphony.env")


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
    base_url: str, *, samples: int, interval_ms: int, timeout_seconds: int
) -> list[DashboardStateSample]:
    target = urllib.parse.urljoin(base_url.rstrip("/") + "/", "api/v1/state")
    etag = ""
    rows: list[DashboardStateSample] = []
    total = max(int(samples), 1)
    pause_seconds = max(int(interval_ms), 0) / 1000.0
    for idx in range(total):
        headers = {"Cache-Control": "no-cache"}
        if etag:
            headers["If-None-Match"] = etag
        req = urllib.request.Request(target, headers=headers, method="GET")
        started = time.perf_counter()
        status = 0
        had_etag = False
        try:
            with urllib.request.urlopen(
                req, timeout=max(int(timeout_seconds), 1)
            ) as resp:
                status = int(resp.status)
                response_etag = (resp.headers.get("ETag") or "").strip()
                if response_etag:
                    etag = response_etag
                    had_etag = True
                _ = resp.read()
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
            )
        )
        if pause_seconds > 0 and idx + 1 < total:
            time.sleep(pause_seconds)
    return rows


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


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    modes = _parse_modes(args.modes)
    ext_root = Path(os.environ.get("MOLT_EXT_ROOT", str(DEFAULT_EXT_ROOT))).expanduser()
    if not ext_root.exists():
        raise RuntimeError(f"External root not mounted: {ext_root}")

    env = os.environ.copy()
    _ext_env_defaults(env, ext_root)
    if not shutil.which("uv"):
        raise RuntimeError("uv is required for symphony_perf")

    samples: list[Sample] = []
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

    output_path: Path | None = None
    if args.output_json:
        output_path = Path(args.output_json).expanduser()
    else:
        stamp = datetime.now(UTC).strftime("%Y%m%d_%H%M%S")
        output_path = ext_root / "logs" / "symphony" / f"symphony_perf_{stamp}.json"
    if output_path is not None:
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text(
            json.dumps(
                {
                    "generated_at": datetime.now(UTC)
                    .isoformat()
                    .replace("+00:00", "Z"),
                    "workflow_path": args.workflow_path,
                    "modes": modes,
                    "iterations": int(args.iterations),
                    "summary": summary,
                    "dashboard_state_api": dashboard_report,
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
                },
                indent=2,
            ),
            encoding="utf-8",
        )
        print(f"wrote {output_path}")

    any_failure = any(sample.returncode != 0 for sample in samples)
    return 2 if any_failure else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
