#!/usr/bin/env python3
from __future__ import annotations

import argparse
import concurrent.futures
import datetime as dt
import json
import os
import signal
import subprocess
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any


DEFAULT_BUILD_SCRIPTS = [
    "examples/hello.py",
    "tests/differential/basic/ellipsis_basic.py",
]
DEFAULT_DIFF_SCRIPTS = [
    "tests/differential/basic/augassign_inplace.py",
    "tests/differential/basic/container_mutation.py",
    "tests/differential/basic/ellipsis_basic.py",
]


@dataclass
class CommandResult:
    command: list[str]
    returncode: int
    elapsed_sec: float
    timed_out: bool
    stdout_tail: str
    stderr_tail: str


def _default_output_root(repo_root: Path) -> Path:
    external_root = Path("/Volumes/APDataStore/Molt")
    ts = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    if external_root.is_dir():
        return external_root / f"throughput_matrix_{ts}"
    return repo_root / "logs" / f"throughput_matrix_{ts}"


def _tail(text: str, lines: int = 12) -> str:
    if not text:
        return ""
    return "\n".join(text.splitlines()[-lines:])


def _run_command(
    command: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout_sec: float,
) -> CommandResult:
    start = time.perf_counter()
    proc = subprocess.Popen(
        command,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    timed_out = False
    try:
        stdout, stderr = proc.communicate(timeout=timeout_sec)
    except subprocess.TimeoutExpired:
        timed_out = True
        stdout = ""
        stderr = ""
        try:
            os.killpg(proc.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                # Best-effort termination; do not block the harness indefinitely.
                pass
    elapsed = time.perf_counter() - start
    return CommandResult(
        command=command,
        returncode=124 if timed_out else proc.returncode,
        elapsed_sec=round(elapsed, 3),
        timed_out=timed_out,
        stdout_tail=_tail(stdout),
        stderr_tail=_tail(stderr),
    )


def _base_env(
    case_cache: Path, shared_target: Path, *, wrapper_enabled: bool
) -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = "src"
    env["UV_NO_SYNC"] = "1"
    env["PYTHONHASHSEED"] = "0"
    env["MOLT_CACHE"] = str(case_cache)
    env["CARGO_TARGET_DIR"] = str(shared_target)
    if wrapper_enabled:
        env["MOLT_USE_SCCACHE"] = "1"
        env.pop("SCCACHE_DISABLE", None)
    else:
        env["MOLT_USE_SCCACHE"] = "0"
        env["SCCACHE_DISABLE"] = "1"
        env.pop("RUSTC_WRAPPER", None)
        env.pop("CARGO_BUILD_RUSTC_WRAPPER", None)
    return env


def _run_build_matrix(
    args: argparse.Namespace, repo_root: Path, output_root: Path
) -> list[dict[str, Any]]:
    shared_target = (
        Path(args.shared_target_dir).expanduser().resolve()
        if args.shared_target_dir
        else output_root / "shared_target"
    )
    shared_target.mkdir(parents=True, exist_ok=True)

    cases: list[dict[str, Any]] = []
    for profile in args.profiles:
        for wrapper in args.wrappers:
            case_name = f"build_profile_{profile}__wrapper_{wrapper}"
            case_root = output_root / case_name
            out_root = case_root / "out"
            cache_root = case_root / "cache"
            out_root.mkdir(parents=True, exist_ok=True)
            cache_root.mkdir(parents=True, exist_ok=True)
            env = _base_env(cache_root, shared_target, wrapper_enabled=bool(wrapper))

            print(f"[build] {case_name}: single phase", flush=True)
            single_cmd = [
                "uv",
                "run",
                "--python",
                args.python_version,
                "python3",
                "-m",
                "molt.cli",
                "build",
                args.build_scripts[0],
                "--profile",
                profile,
                "--cache-report",
                "--out-dir",
                str(out_root / "single"),
            ]
            single_result = _run_command(
                single_cmd,
                cwd=repo_root,
                env=env,
                timeout_sec=args.timeout_sec,
            )

            print(f"[build] {case_name}: concurrent phase", flush=True)

            def _worker(index: int, script: str) -> CommandResult:
                cmd = [
                    "uv",
                    "run",
                    "--python",
                    args.python_version,
                    "python3",
                    "-m",
                    "molt.cli",
                    "build",
                    script,
                    "--profile",
                    profile,
                    "--cache-report",
                    "--out-dir",
                    str(out_root / f"concurrent_{index}"),
                ]
                return _run_command(
                    cmd,
                    cwd=repo_root,
                    env=env,
                    timeout_sec=args.timeout_sec,
                )

            conc_start = time.perf_counter()
            worker_results: list[CommandResult] = []
            with concurrent.futures.ThreadPoolExecutor(
                max_workers=min(args.concurrency, len(args.build_scripts))
            ) as executor:
                futures = [
                    executor.submit(_worker, idx, script)
                    for idx, script in enumerate(args.build_scripts)
                ]
                for future in concurrent.futures.as_completed(futures):
                    worker_results.append(future.result())
            conc_elapsed = round(time.perf_counter() - conc_start, 3)

            case_payload = {
                "case": case_name,
                "profile": profile,
                "wrapper": bool(wrapper),
                "single": asdict(single_result),
                "concurrent_wall_sec": conc_elapsed,
                "concurrent_workers": [asdict(item) for item in worker_results],
            }
            print(
                (
                    f"[build] {case_name}: single={single_result.elapsed_sec}s "
                    f"(rc={single_result.returncode}) "
                    f"concurrent_wall={conc_elapsed}s"
                ),
                flush=True,
            )
            cases.append(case_payload)
    return cases


def _run_diff_matrix(
    args: argparse.Namespace, repo_root: Path, output_root: Path
) -> list[dict[str, Any]]:
    shared_target = (
        Path(args.shared_target_dir).expanduser().resolve()
        if args.shared_target_dir
        else output_root / "shared_target"
    )
    shared_target.mkdir(parents=True, exist_ok=True)
    cases: list[dict[str, Any]] = []
    for profile in args.profiles:
        for wrapper in args.wrappers:
            case_name = f"diff_profile_{profile}__wrapper_{wrapper}"
            case_root = output_root / case_name
            cache_root = case_root / "cache"
            diff_root = case_root / "diff_root"
            tmp_root = case_root / "tmp"
            log_dir = case_root / "log_dir"
            summary_path = case_root / "summary.json"
            for path in [cache_root, diff_root, tmp_root, log_dir]:
                path.mkdir(parents=True, exist_ok=True)
            env = _base_env(cache_root, shared_target, wrapper_enabled=bool(wrapper))
            env["MOLT_DIFF_MEASURE_RSS"] = "1"
            env["MOLT_DIFF_RLIMIT_GB"] = "10"
            env["MOLT_DIFF_ROOT"] = str(diff_root)
            env["MOLT_DIFF_TMPDIR"] = str(tmp_root)
            env["MOLT_DIFF_ALLOW_RUSTC_WRAPPER"] = "1" if wrapper else "0"
            cmd = [
                "uv",
                "run",
                "--python",
                args.python_version,
                "python3",
                "-u",
                "tests/molt_diff.py",
                "--build-profile",
                profile,
                "--jobs",
                str(args.diff_jobs),
                "--json-output",
                str(summary_path),
                "--log-dir",
                str(log_dir),
                "--no-retry-oom",
                *args.diff_scripts,
            ]
            print(f"[diff] {case_name}", flush=True)
            result = _run_command(
                cmd,
                cwd=repo_root,
                env=env,
                timeout_sec=args.diff_timeout_sec,
            )
            payload: dict[str, Any] = {
                "case": case_name,
                "profile": profile,
                "wrapper": bool(wrapper),
                **asdict(result),
            }
            if summary_path.exists():
                try:
                    payload["summary"] = json.loads(summary_path.read_text())
                except json.JSONDecodeError as exc:
                    payload["summary_read_error"] = str(exc)
            print(
                (
                    f"[diff] {case_name}: elapsed={result.elapsed_sec}s "
                    f"rc={result.returncode}"
                ),
                flush=True,
            )
            cases.append(payload)
    return cases


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Run a throughput matrix over Molt build and optional differential runs."
        )
    )
    parser.add_argument(
        "--output-root",
        help=(
            "Output root for artifacts/results. Defaults to "
            "/Volumes/APDataStore/Molt/... when available, else logs/..."
        ),
    )
    parser.add_argument(
        "--shared-target-dir",
        help=(
            "Optional shared CARGO_TARGET_DIR for all cases. Use a filesystem "
            "with hard-link support (APFS/ext4) for best incremental behavior."
        ),
    )
    parser.add_argument(
        "--python-version",
        default="3.12",
        help="uv python version for commands (default: 3.12).",
    )
    parser.add_argument(
        "--profiles",
        nargs="+",
        default=["dev", "release"],
        choices=["dev", "release"],
        help="Build profiles to test (default: dev release).",
    )
    parser.add_argument(
        "--wrappers",
        nargs="+",
        type=int,
        default=[0, 1],
        choices=[0, 1],
        help="Wrapper modes: 0=off, 1=on (default: 0 1).",
    )
    parser.add_argument(
        "--build-scripts",
        nargs="+",
        default=DEFAULT_BUILD_SCRIPTS,
        help="Scripts used for build single/concurrent phases.",
    )
    parser.add_argument(
        "--concurrency",
        type=int,
        default=2,
        help="Concurrent build workers (default: 2).",
    )
    parser.add_argument(
        "--timeout-sec",
        type=float,
        default=75.0,
        help="Per-build command timeout in seconds (default: 75).",
    )
    parser.add_argument(
        "--run-diff",
        action="store_true",
        help="Also run a differential mini-matrix.",
    )
    parser.add_argument(
        "--diff-scripts",
        nargs="+",
        default=DEFAULT_DIFF_SCRIPTS,
        help="Differential scripts when --run-diff is enabled.",
    )
    parser.add_argument(
        "--diff-jobs",
        type=int,
        default=2,
        help="Worker count for differential runs (default: 2).",
    )
    parser.add_argument(
        "--diff-timeout-sec",
        type=float,
        default=180.0,
        help="Per-diff command timeout in seconds (default: 180).",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo_root = Path(__file__).resolve().parent.parent
    output_root = (
        Path(args.output_root).expanduser().resolve()
        if args.output_root
        else _default_output_root(repo_root)
    )
    output_root.mkdir(parents=True, exist_ok=True)
    print(f"output_root={output_root}", flush=True)

    results: dict[str, Any] = {
        "meta": {
            "timestamp_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
            "repo_root": str(repo_root),
            "output_root": str(output_root),
            "shared_target_dir": args.shared_target_dir or "",
            "python_version": args.python_version,
            "profiles": args.profiles,
            "wrappers": args.wrappers,
            "build_scripts": args.build_scripts,
            "concurrency": args.concurrency,
            "timeout_sec": args.timeout_sec,
            "run_diff": args.run_diff,
            "diff_scripts": args.diff_scripts if args.run_diff else [],
            "diff_jobs": args.diff_jobs if args.run_diff else 0,
            "diff_timeout_sec": args.diff_timeout_sec if args.run_diff else 0,
        }
    }
    results["build_matrix"] = _run_build_matrix(args, repo_root, output_root)
    results["diff_matrix"] = (
        _run_diff_matrix(args, repo_root, output_root) if args.run_diff else []
    )

    out_path = output_root / "matrix_results.json"
    out_path.write_text(json.dumps(results, indent=2) + "\n")
    print(f"wrote {out_path}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
