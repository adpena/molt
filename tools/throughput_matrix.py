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


DEFAULT_EXTERNAL_ROOT = Path("/Volumes/APDataStore/Molt")

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


def _resolved_external_root(*, fallback: Path | None = None) -> Path:
    configured = os.environ.get("MOLT_EXT_ROOT")
    if configured:
        root = Path(configured).expanduser().resolve()
        if root.is_dir():
            return root
        raise SystemExit(
            f"MOLT_EXT_ROOT is not a mounted directory: {root}. "
            "Mount the external volume or pass --output-root explicitly."
        )
    if DEFAULT_EXTERNAL_ROOT.is_dir():
        return DEFAULT_EXTERNAL_ROOT
    if fallback is not None:
        return fallback
    raise SystemExit(
        "External volume is required for throughput matrix defaults: "
        f"{DEFAULT_EXTERNAL_ROOT} is not mounted. "
        "Mount it or pass --output-root explicitly for an approved override."
    )


def _default_output_root() -> Path:
    external_root = _resolved_external_root()
    ts = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    return external_root / f"throughput_matrix_{ts}"


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
    case_cache: Path,
    shared_target: Path,
    *,
    wrapper_enabled: bool,
    external_root: Path,
    diff_root: Path,
    diff_tmp: Path,
) -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = "src"
    env["UV_NO_SYNC"] = "1"
    env["PYTHONHASHSEED"] = "0"
    env["MOLT_EXT_ROOT"] = str(external_root)
    env["MOLT_CACHE"] = str(case_cache)
    env["CARGO_TARGET_DIR"] = str(shared_target)
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = str(shared_target)
    env["MOLT_DIFF_ROOT"] = str(diff_root)
    env["MOLT_DIFF_TMPDIR"] = str(diff_tmp)
    env["TMPDIR"] = str(diff_tmp)
    env.setdefault("UV_CACHE_DIR", str(external_root / "uv-cache"))
    if wrapper_enabled:
        env["MOLT_USE_SCCACHE"] = "1"
        env.pop("SCCACHE_DISABLE", None)
    else:
        env["MOLT_USE_SCCACHE"] = "0"
        env["SCCACHE_DISABLE"] = "1"
        env.pop("RUSTC_WRAPPER", None)
        env.pop("CARGO_BUILD_RUSTC_WRAPPER", None)
    env["MOLT_DIFF_ALLOW_RUSTC_WRAPPER"] = "1" if wrapper_enabled else "0"
    return env


def _run_build_matrix(
    args: argparse.Namespace, repo_root: Path, output_root: Path
) -> list[dict[str, Any]]:
    external_root = _resolved_external_root(
        fallback=output_root if args.output_root else None
    )
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
            diff_root = case_root / "diff_root"
            tmp_root = case_root / "tmp"
            out_root.mkdir(parents=True, exist_ok=True)
            cache_root.mkdir(parents=True, exist_ok=True)
            diff_root.mkdir(parents=True, exist_ok=True)
            tmp_root.mkdir(parents=True, exist_ok=True)
            env = _base_env(
                cache_root,
                shared_target,
                wrapper_enabled=bool(wrapper),
                external_root=external_root,
                diff_root=diff_root,
                diff_tmp=tmp_root,
            )

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
    external_root = _resolved_external_root(
        fallback=output_root if args.output_root else None
    )
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
            env = _base_env(
                cache_root,
                shared_target,
                wrapper_enabled=bool(wrapper),
                external_root=external_root,
                diff_root=diff_root,
                diff_tmp=tmp_root,
            )
            env["MOLT_DIFF_MEASURE_RSS"] = "1"
            env["MOLT_DIFF_RLIMIT_GB"] = "10"
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


def _is_nonzero_returncode(payload: dict[str, Any]) -> bool:
    rc = payload.get("returncode")
    return isinstance(rc, int) and rc != 0


def _is_timed_out(payload: dict[str, Any]) -> bool:
    return bool(payload.get("timed_out", False))


def _evaluate_gate_status(
    args: argparse.Namespace,
    *,
    build_matrix: list[dict[str, Any]],
    diff_matrix: list[dict[str, Any]],
) -> dict[str, Any]:
    build_error_count = 0
    build_timeout_count = 0
    diff_command_error_count = 0
    diff_timeout_count = 0
    diff_failed_tests = 0
    ratio_violations: list[dict[str, Any]] = []
    build_error_cases: list[str] = []
    build_timeout_cases: list[str] = []
    diff_error_cases: list[str] = []
    diff_timeout_cases: list[str] = []
    diff_failed_cases: list[dict[str, Any]] = []

    for case in build_matrix:
        case_name = str(case.get("case", "unknown"))
        single = case.get("single")
        if isinstance(single, dict):
            if _is_nonzero_returncode(single):
                build_error_count += 1
                build_error_cases.append(f"{case_name}:single")
            if _is_timed_out(single):
                build_timeout_count += 1
                build_timeout_cases.append(f"{case_name}:single")
            single_elapsed = single.get("elapsed_sec")
        else:
            single_elapsed = None

        workers = case.get("concurrent_workers", [])
        if isinstance(workers, list):
            for index, worker in enumerate(workers):
                if not isinstance(worker, dict):
                    continue
                if _is_nonzero_returncode(worker):
                    build_error_count += 1
                    build_error_cases.append(f"{case_name}:worker_{index}")
                if _is_timed_out(worker):
                    build_timeout_count += 1
                    build_timeout_cases.append(f"{case_name}:worker_{index}")

        ratio_threshold = args.gate_max_concurrent_vs_single_ratio
        conc_wall = case.get("concurrent_wall_sec")
        if (
            ratio_threshold is not None
            and isinstance(single_elapsed, (int, float))
            and float(single_elapsed) > 0
            and isinstance(conc_wall, (int, float))
        ):
            ratio = float(conc_wall) / float(single_elapsed)
            if ratio > ratio_threshold:
                ratio_violations.append(
                    {
                        "case": case_name,
                        "single_elapsed_sec": round(float(single_elapsed), 6),
                        "concurrent_wall_sec": round(float(conc_wall), 6),
                        "ratio": round(ratio, 6),
                        "max_ratio": ratio_threshold,
                    }
                )

    for case in diff_matrix:
        case_name = str(case.get("case", "unknown"))
        if _is_nonzero_returncode(case):
            diff_command_error_count += 1
            diff_error_cases.append(case_name)
        if _is_timed_out(case):
            diff_timeout_count += 1
            diff_timeout_cases.append(case_name)
        summary = case.get("summary")
        if isinstance(summary, dict):
            failed = summary.get("failed")
            if isinstance(failed, int) and failed > 0:
                diff_failed_tests += failed
                diff_failed_cases.append(
                    {
                        "case": case_name,
                        "failed": failed,
                        "passed": summary.get("passed"),
                        "total": summary.get("total"),
                    }
                )

    thresholds: dict[str, Any] = {
        "max_build_errors": args.gate_max_build_errors,
        "max_build_timeouts": args.gate_max_build_timeouts,
        "max_diff_command_errors": args.gate_max_diff_command_errors,
        "max_diff_timeouts": args.gate_max_diff_timeouts,
        "max_diff_failed_tests": args.gate_max_diff_failed_tests,
        "max_concurrent_vs_single_ratio": args.gate_max_concurrent_vs_single_ratio,
    }
    observed: dict[str, Any] = {
        "build_errors": build_error_count,
        "build_timeouts": build_timeout_count,
        "diff_command_errors": diff_command_error_count,
        "diff_timeouts": diff_timeout_count,
        "diff_failed_tests": diff_failed_tests,
        "ratio_violations": len(ratio_violations),
    }

    failed_reasons: list[str] = []
    if build_error_count > args.gate_max_build_errors:
        failed_reasons.append(
            f"build command errors {build_error_count} > {args.gate_max_build_errors}"
        )
    if build_timeout_count > args.gate_max_build_timeouts:
        failed_reasons.append(
            f"build timeouts {build_timeout_count} > {args.gate_max_build_timeouts}"
        )
    if diff_command_error_count > args.gate_max_diff_command_errors:
        failed_reasons.append(
            "diff command errors "
            f"{diff_command_error_count} > {args.gate_max_diff_command_errors}"
        )
    if diff_timeout_count > args.gate_max_diff_timeouts:
        failed_reasons.append(
            f"diff timeouts {diff_timeout_count} > {args.gate_max_diff_timeouts}"
        )
    if diff_failed_tests > args.gate_max_diff_failed_tests:
        failed_reasons.append(
            f"diff failed tests {diff_failed_tests} > {args.gate_max_diff_failed_tests}"
        )
    if ratio_violations:
        failed_reasons.append(
            "build concurrency ratio violations detected: "
            f"{len(ratio_violations)} over threshold"
        )

    return {
        "passed": not failed_reasons,
        "thresholds": thresholds,
        "observed": observed,
        "failed_reasons": failed_reasons,
        "violations": {
            "build_error_cases": build_error_cases,
            "build_timeout_cases": build_timeout_cases,
            "diff_error_cases": diff_error_cases,
            "diff_timeout_cases": diff_timeout_cases,
            "diff_failed_cases": diff_failed_cases,
            "concurrency_ratio_cases": ratio_violations,
        },
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Run a throughput matrix over Molt build and optional differential runs."
        )
    )
    parser.add_argument(
        "--output-root",
        help=(
            "Output root for artifacts/results. Defaults under mounted "
            "MOLT_EXT_ROOT (or /Volumes/APDataStore/Molt)."
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
    parser.add_argument(
        "--gate-max-build-errors",
        type=int,
        default=0,
        help="Gate threshold for non-zero build command return codes (default: 0).",
    )
    parser.add_argument(
        "--gate-max-build-timeouts",
        type=int,
        default=0,
        help="Gate threshold for build command timeouts (default: 0).",
    )
    parser.add_argument(
        "--gate-max-diff-command-errors",
        type=int,
        default=0,
        help=("Gate threshold for non-zero diff command return codes (default: 0)."),
    )
    parser.add_argument(
        "--gate-max-diff-timeouts",
        type=int,
        default=0,
        help="Gate threshold for diff command timeouts (default: 0).",
    )
    parser.add_argument(
        "--gate-max-diff-failed-tests",
        type=int,
        default=0,
        help=(
            "Gate threshold for summed failed diff tests from per-case summaries "
            "(default: 0)."
        ),
    )
    parser.add_argument(
        "--gate-max-concurrent-vs-single-ratio",
        type=float,
        default=None,
        help=(
            "Optional gate threshold for `concurrent_wall_sec / single_elapsed_sec` "
            "per build case."
        ),
    )
    parser.add_argument(
        "--fail-on-gate",
        action="store_true",
        help="Exit with code 2 when gate status fails.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo_root = Path(__file__).resolve().parent.parent
    output_root = (
        Path(args.output_root).expanduser().resolve()
        if args.output_root
        else _default_output_root()
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
            "gate_max_build_errors": args.gate_max_build_errors,
            "gate_max_build_timeouts": args.gate_max_build_timeouts,
            "gate_max_diff_command_errors": args.gate_max_diff_command_errors,
            "gate_max_diff_timeouts": args.gate_max_diff_timeouts,
            "gate_max_diff_failed_tests": args.gate_max_diff_failed_tests,
            "gate_max_concurrent_vs_single_ratio": (
                args.gate_max_concurrent_vs_single_ratio
            ),
            "fail_on_gate": args.fail_on_gate,
        }
    }
    results["build_matrix"] = _run_build_matrix(args, repo_root, output_root)
    results["diff_matrix"] = (
        _run_diff_matrix(args, repo_root, output_root) if args.run_diff else []
    )
    results["gate_status"] = _evaluate_gate_status(
        args,
        build_matrix=results["build_matrix"],
        diff_matrix=results["diff_matrix"],
    )

    out_path = output_root / "matrix_results.json"
    out_path.write_text(json.dumps(results, indent=2) + "\n")
    print(f"wrote {out_path}", flush=True)
    if bool(results["gate_status"].get("passed", False)):
        print("gate_status=pass", flush=True)
        return 0
    print("gate_status=fail", flush=True)
    reasons = results["gate_status"].get("failed_reasons", [])
    if isinstance(reasons, list):
        for reason in reasons:
            print(f"gate_failure: {reason}", flush=True)
    if args.fail_on_gate:
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
