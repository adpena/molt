#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import importlib.util
import json
import os
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DIFF_SCRIPT = ROOT / "tests" / "molt_diff.py"
IR_GATE_SCRIPT = ROOT / "tools" / "check_molt_ir_ops.py"


@dataclass(frozen=True)
class AttemptConfig:
    attempt: int
    jobs: int
    timeout_sec: int


def _now_utc() -> str:
    return dt.datetime.now(dt.UTC).isoformat(timespec="seconds")


def _default_run_root_base() -> Path:
    preferred = Path("/Volumes/APDataStore/Molt")
    if preferred.exists():
        return preferred / "ir_probe_supervisor"
    return ROOT / "target" / "ir_probe_supervisor"


def _load_required_probes() -> tuple[str, ...]:
    module_name = "check_molt_ir_ops_for_ir_probe_supervisor"
    spec = importlib.util.spec_from_file_location(
        module_name, ROOT / "tools" / "check_molt_ir_ops.py"
    )
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load tools/check_molt_ir_ops.py")
    module = importlib.util.module_from_spec(spec)
    # dataclasses resolves type metadata via sys.modules[cls.__module__]
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    probes = getattr(module, "REQUIRED_DIFF_PROBES", None)
    if not isinstance(probes, tuple):
        raise RuntimeError("REQUIRED_DIFF_PROBES missing from check_molt_ir_ops.py")
    return probes


def _active_diff_runs() -> list[tuple[int, str]]:
    proc = subprocess.run(
        ["ps", "-axo", "pid=,command="],
        check=True,
        capture_output=True,
        text=True,
    )
    active: list[tuple[int, str]] = []
    for raw in proc.stdout.splitlines():
        line = raw.strip()
        if not line:
            continue
        parts = line.split(maxsplit=1)
        if len(parts) != 2:
            continue
        pid_raw, cmd = parts
        if "tests/molt_diff.py" not in cmd:
            continue
        try:
            pid = int(pid_raw)
        except ValueError:
            continue
        if pid == os.getpid():
            continue
        active.append((pid, cmd))
    return active


def _wait_for_idle(timeout_sec: int, poll_sec: float) -> list[tuple[int, str]]:
    deadline = time.time() + max(0, timeout_sec)
    while True:
        active = _active_diff_runs()
        if not active:
            return []
        if time.time() >= deadline:
            return active
        time.sleep(max(0.25, poll_sec))


def _load_metrics(path: Path) -> list[dict]:
    if not path.exists():
        return []
    entries: list[dict] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        text = line.strip()
        if not text:
            continue
        try:
            payload = json.loads(text)
        except json.JSONDecodeError:
            continue
        if isinstance(payload, dict):
            entries.append(payload)
    return entries


def _latest_run_id(entries: list[dict]) -> str | None:
    best_ts = float("-inf")
    best_run_id: str | None = None
    for payload in entries:
        run_id = payload.get("run_id")
        if not isinstance(run_id, str) or not run_id:
            continue
        ts_raw = payload.get("timestamp")
        ts = float(ts_raw) if isinstance(ts_raw, (int, float)) else float("-inf")
        if ts >= best_ts:
            best_ts = ts
            best_run_id = run_id
    return best_run_id


def _status_by_probe(entries: list[dict], run_id: str) -> dict[str, str]:
    latest: dict[str, tuple[float, str]] = {}
    for payload in entries:
        if payload.get("run_id") != run_id:
            continue
        file_path = payload.get("file")
        status = payload.get("status")
        if not isinstance(file_path, str) or not isinstance(status, str):
            continue
        ts_raw = payload.get("timestamp")
        ts = float(ts_raw) if isinstance(ts_raw, (int, float)) else float("-inf")
        existing = latest.get(file_path)
        if existing is None or ts >= existing[0]:
            latest[file_path] = (ts, status)
    return {path: status for path, (_, status) in latest.items()}


def _should_retry(statuses: dict[str, str]) -> bool:
    retry_states = {"build_timeout", "run_timeout", "build_failed"}
    return any(status in retry_states for status in statuses.values())


def _run_command(
    cmd: list[str], *, env: dict[str, str], cwd: Path, timeout_sec: int
) -> tuple[int, str, str, bool]:
    try:
        proc = subprocess.run(
            cmd,
            cwd=cwd,
            env=env,
            capture_output=True,
            text=True,
            timeout=max(1, timeout_sec),
            check=False,
        )
        return proc.returncode, proc.stdout, proc.stderr, False
    except subprocess.TimeoutExpired as exc:
        return 124, exc.stdout or "", exc.stderr or "", True


def _render_markdown(
    *,
    started_at: str,
    finished_at: str,
    required_probes: tuple[str, ...],
    attempts: list[dict],
    final_ok: bool,
    final_message: str,
) -> str:
    lines = [
        "# IR Probe Supervisor Checkpoint",
        "",
        f"- Started: `{started_at}`",
        f"- Finished: `{finished_at}`",
        f"- Required probes: `{len(required_probes)}`",
        f"- Final status: `{'ok' if final_ok else 'failed'}`",
        f"- Note: {final_message}",
        "",
        "## Attempts",
        "",
        "| Attempt | Jobs | Timeout | Diff RC | Gate RC | Timed Out | Run ID |",
        "| --- | --- | --- | --- | --- | --- | --- |",
    ]
    for item in attempts:
        lines.append(
            "| {attempt} | {jobs} | {timeout}s | {diff_rc} | {gate_rc} | {timed_out} | {run_id} |".format(
                attempt=item.get("attempt"),
                jobs=item.get("jobs"),
                timeout=item.get("timeout_sec"),
                diff_rc=item.get("diff_rc"),
                gate_rc=item.get("gate_rc"),
                timed_out="yes" if item.get("timed_out") else "no",
                run_id=item.get("run_id") or "",
            )
        )
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Supervise required IR differential probes with contention controls, "
            "timeout-aware retry, and strict execution gate validation."
        )
    )
    parser.add_argument(
        "--python", default="3.12", help="uv python version (default: 3.12)"
    )
    parser.add_argument("--jobs", type=int, default=2, help="initial jobs for diff run")
    parser.add_argument(
        "--retry-jobs",
        type=int,
        default=1,
        help="jobs for timeout/build-failed retry attempt",
    )
    parser.add_argument(
        "--diff-timeout",
        type=int,
        default=180,
        help="MOLT_DIFF_TIMEOUT value for each test",
    )
    parser.add_argument(
        "--run-timeout",
        type=int,
        default=5400,
        help="overall subprocess timeout per attempt",
    )
    parser.add_argument(
        "--diff-build-timeout",
        type=int,
        default=0,
        help=(
            "optional MOLT_DIFF_BUILD_TIMEOUT override in seconds "
            "(0 keeps molt_diff.py default)"
        ),
    )
    parser.add_argument(
        "--idle-wait-sec",
        type=int,
        default=900,
        help="max seconds to wait for existing diff runs to clear",
    )
    parser.add_argument(
        "--idle-poll-sec",
        type=float,
        default=5.0,
        help="poll interval for idle wait",
    )
    parser.add_argument(
        "--skip-idle-check",
        action="store_true",
        help="skip waiting for existing active tests/molt_diff.py processes",
    )
    parser.add_argument(
        "--run-root-base",
        type=Path,
        default=_default_run_root_base(),
        help="base directory for supervisor run artifacts",
    )
    parser.add_argument(
        "--cache-root",
        type=Path,
        default=Path("/Volumes/APDataStore/Molt/molt_cache"),
        help="MOLT_CACHE path",
    )
    parser.add_argument(
        "--rlimit-gb",
        type=int,
        default=10,
        help="MOLT_DIFF_RLIMIT_GB value",
    )
    parser.add_argument(
        "--report-json",
        type=Path,
        help="optional explicit JSON report path (default under run root)",
    )
    parser.add_argument(
        "--report-md",
        type=Path,
        help="optional explicit markdown report path (default under run root)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print planned commands/env and exit without running",
    )
    args = parser.parse_args()

    started_at = _now_utc()
    required_probes = _load_required_probes()
    timestamp = dt.datetime.now().strftime("%Y%m%d_%H%M%S")
    run_root = args.run_root_base / timestamp
    run_root.mkdir(parents=True, exist_ok=True)
    report_json = args.report_json or (run_root / "checkpoint.json")
    report_md = args.report_md or (run_root / "checkpoint.md")

    attempts_cfg = [
        AttemptConfig(
            attempt=1, jobs=max(1, args.jobs), timeout_sec=max(1, args.run_timeout)
        ),
        AttemptConfig(
            attempt=2,
            jobs=max(1, args.retry_jobs),
            timeout_sec=max(1, args.run_timeout),
        ),
    ]

    if args.dry_run:
        for attempt_cfg in attempts_cfg:
            diff_cmd = [
                "uv",
                "run",
                "--python",
                args.python,
                "python3",
                "-u",
                str(DIFF_SCRIPT),
                "--jobs",
                str(attempt_cfg.jobs),
                *required_probes,
            ]
            print(f"attempt {attempt_cfg.attempt}: {' '.join(diff_cmd)}")
        if args.diff_build_timeout > 0:
            print(f"dry-run env: MOLT_DIFF_BUILD_TIMEOUT={args.diff_build_timeout}")
        print("ir-probe-supervisor: dry-run complete")
        return 0

    if not args.skip_idle_check:
        active = _wait_for_idle(args.idle_wait_sec, args.idle_poll_sec)
        if active:
            payload = {
                "started_at": started_at,
                "finished_at": _now_utc(),
                "required_probe_count": len(required_probes),
                "required_probes": list(required_probes),
                "final_ok": False,
                "final_message": "timed out waiting for active diff runs",
                "active_diff_runs": [
                    {"pid": pid, "command": cmd} for pid, cmd in active
                ],
                "attempts": [],
            }
            report_json.parent.mkdir(parents=True, exist_ok=True)
            report_json.write_text(
                json.dumps(payload, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            report_md.parent.mkdir(parents=True, exist_ok=True)
            report_md.write_text(
                _render_markdown(
                    started_at=started_at,
                    finished_at=payload["finished_at"],
                    required_probes=required_probes,
                    attempts=[],
                    final_ok=False,
                    final_message="timed out waiting for active diff runs",
                )
                + "\n",
                encoding="utf-8",
            )
            print(
                f"ir-probe-supervisor: FAIL: active diff runs did not clear ({len(active)})"
            )
            return 1

    attempt_results: list[dict] = []
    final_ok = False
    final_message = "unattempted"

    for attempt_cfg in attempts_cfg:
        if attempt_cfg.attempt > 1:
            previous = attempt_results[-1]
            if previous.get("diff_rc") == 0 and previous.get("gate_rc") == 0:
                break
            if not _should_retry(previous.get("statuses", {})):
                break

        attempt_root = run_root / f"attempt_{attempt_cfg.attempt}"
        diff_root = attempt_root / "diff_root"
        target_root = attempt_root / "target"
        tmp_root = diff_root / "tmp"
        failures_path = diff_root / "ir_probe_failures.txt"
        diff_root.mkdir(parents=True, exist_ok=True)
        target_root.mkdir(parents=True, exist_ok=True)
        tmp_root.mkdir(parents=True, exist_ok=True)

        env = os.environ.copy()
        env["MOLT_DIFF_ROOT"] = str(diff_root)
        env["MOLT_DIFF_TMPDIR"] = str(tmp_root)
        env["MOLT_DIFF_MEASURE_RSS"] = "1"
        env["MOLT_DIFF_RLIMIT_GB"] = str(args.rlimit_gb)
        env["MOLT_DIFF_TIMEOUT"] = str(args.diff_timeout)
        env["MOLT_DIFF_FAILURES"] = str(failures_path)
        env["MOLT_CACHE"] = str(args.cache_root)
        env["CARGO_TARGET_DIR"] = str(target_root)
        env["MOLT_DIFF_CARGO_TARGET_DIR"] = str(target_root)
        if args.diff_build_timeout > 0:
            env["MOLT_DIFF_BUILD_TIMEOUT"] = str(args.diff_build_timeout)

        diff_cmd = [
            "uv",
            "run",
            "--python",
            args.python,
            "python3",
            "-u",
            str(DIFF_SCRIPT),
            "--jobs",
            str(attempt_cfg.jobs),
            *required_probes,
        ]

        diff_rc, diff_out, diff_err, diff_timed_out = _run_command(
            diff_cmd, env=env, cwd=ROOT, timeout_sec=attempt_cfg.timeout_sec
        )
        (attempt_root / "diff.stdout.log").write_text(diff_out, encoding="utf-8")
        (attempt_root / "diff.stderr.log").write_text(diff_err, encoding="utf-8")

        metrics_path = diff_root / "rss_metrics.jsonl"
        entries = _load_metrics(metrics_path)
        run_id = _latest_run_id(entries)
        statuses = _status_by_probe(entries, run_id) if run_id else {}

        gate_rc = 1
        gate_out = ""
        gate_err = ""
        if run_id:
            gate_cmd = [
                sys.executable,
                str(IR_GATE_SCRIPT),
                "--require-probe-execution",
                "--probe-rss-metrics",
                str(metrics_path),
                "--failure-queue",
                str(failures_path),
                "--probe-run-id",
                run_id,
            ]
            gate_rc, gate_out, gate_err, _ = _run_command(
                gate_cmd,
                env=env,
                cwd=ROOT,
                timeout_sec=max(60, min(600, attempt_cfg.timeout_sec)),
            )
        (attempt_root / "gate.stdout.log").write_text(gate_out, encoding="utf-8")
        (attempt_root / "gate.stderr.log").write_text(gate_err, encoding="utf-8")

        result = {
            "attempt": attempt_cfg.attempt,
            "jobs": attempt_cfg.jobs,
            "timeout_sec": attempt_cfg.timeout_sec,
            "diff_rc": diff_rc,
            "gate_rc": gate_rc,
            "timed_out": diff_timed_out,
            "run_id": run_id,
            "statuses": statuses,
            "artifact_root": str(attempt_root),
        }
        attempt_results.append(result)

        if diff_rc == 0 and gate_rc == 0:
            final_ok = True
            final_message = f"attempt {attempt_cfg.attempt} passed"
            break
        final_ok = False
        final_message = f"attempt {attempt_cfg.attempt} failed (diff_rc={diff_rc}, gate_rc={gate_rc})"

    payload = {
        "started_at": started_at,
        "finished_at": _now_utc(),
        "required_probe_count": len(required_probes),
        "required_probes": list(required_probes),
        "run_root": str(run_root),
        "final_ok": final_ok,
        "final_message": final_message,
        "attempts": attempt_results,
    }
    report_json.parent.mkdir(parents=True, exist_ok=True)
    report_json.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    report_md.parent.mkdir(parents=True, exist_ok=True)
    report_md.write_text(
        _render_markdown(
            started_at=payload["started_at"],
            finished_at=payload["finished_at"],
            required_probes=required_probes,
            attempts=attempt_results,
            final_ok=final_ok,
            final_message=final_message,
        )
        + "\n",
        encoding="utf-8",
    )

    status = "OK" if final_ok else "FAIL"
    print(f"ir-probe-supervisor: {status}: {final_message}")
    print(f"  report_json={report_json}")
    print(f"  report_md={report_md}")
    return 0 if final_ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
