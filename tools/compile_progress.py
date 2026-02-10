#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import shlex
import signal
import shutil
import subprocess
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class CaseSpec:
    name: str
    profile: str
    cache_mode: str
    daemon: bool
    release_cargo_profile: str | None = None
    warmup_runs: int = 0


@dataclass
class CaseResult:
    name: str
    profile: str
    cache_mode: str
    daemon: bool
    release_cargo_profile: str | None
    command: list[str]
    returncode: int
    elapsed_sec: float
    timed_out: bool
    cache_state: str
    stdout_tail: str
    stderr_tail: str
    diagnostics_path: str | None
    diagnostics_total_sec: float | None
    diagnostics_phase_sec: dict[str, float] | None
    attempts: int
    warmup_runs: int
    retry_reason: str | None


CASE_SPECS: tuple[CaseSpec, ...] = (
    CaseSpec(
        name="dev_cold",
        profile="dev",
        cache_mode="cache-report",
        daemon=True,
    ),
    CaseSpec(
        name="dev_warm",
        profile="dev",
        cache_mode="cache-report",
        daemon=True,
    ),
    CaseSpec(
        name="dev_nocache_daemon_on",
        profile="dev",
        cache_mode="no-cache",
        daemon=True,
    ),
    CaseSpec(
        name="dev_nocache_daemon_off",
        profile="dev",
        cache_mode="no-cache",
        daemon=False,
    ),
    CaseSpec(
        name="release_cold",
        profile="release",
        cache_mode="cache-report",
        daemon=True,
    ),
    CaseSpec(
        name="release_warm",
        profile="release",
        cache_mode="cache-report",
        daemon=True,
    ),
    CaseSpec(
        name="release_nocache_warm",
        profile="release",
        cache_mode="no-cache",
        daemon=True,
    ),
    CaseSpec(
        name="release_fast_cold",
        profile="release",
        cache_mode="cache-report",
        daemon=True,
        release_cargo_profile="release-fast",
    ),
    CaseSpec(
        name="release_fast_warm",
        profile="release",
        cache_mode="cache-report",
        daemon=True,
        release_cargo_profile="release-fast",
    ),
    CaseSpec(
        name="release_fast_nocache_warm",
        profile="release",
        cache_mode="no-cache",
        daemon=True,
        release_cargo_profile="release-fast",
    ),
    CaseSpec(
        name="dev_queue_daemon_on",
        profile="dev",
        cache_mode="no-cache",
        daemon=True,
        warmup_runs=2,
    ),
    CaseSpec(
        name="dev_queue_daemon_off",
        profile="dev",
        cache_mode="no-cache",
        daemon=False,
        warmup_runs=2,
    ),
)

CASE_BY_NAME = {case.name: case for case in CASE_SPECS}
DEFAULT_CASES = tuple(
    case.name
    for case in CASE_SPECS
    if not case.name.startswith("dev_queue_")
    and not case.name.startswith("release_fast_")
)


def _default_output_root(repo_root: Path) -> Path:
    external_root = Path("/Volumes/APDataStore/Molt")
    stamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    if external_root.is_dir():
        return external_root / f"compile_progress_{stamp}"
    return repo_root / "bench" / "results" / "compile_progress" / stamp


def _tail(text: str, lines: int = 20) -> str:
    if not text:
        return ""
    return "\n".join(text.splitlines()[-lines:])


def _extract_cache_state(stdout: str) -> str:
    for line in stdout.splitlines():
        if not line.startswith("Cache: "):
            continue
        if "hit" in line:
            return "hit"
        if "miss" in line:
            return "miss"
        return line.removeprefix("Cache: ").strip() or "unknown"
    return "n/a"


def _is_retryable_failure(returncode: int, timed_out: bool, stderr: str) -> str | None:
    if timed_out:
        return "timeout"
    if returncode == 124:
        return "timeout_exit_124"
    if returncode in (143, -15):
        return "sigterm"
    if returncode != 0 and "Timed out waiting for build lock" in stderr:
        return "build_lock_timeout"
    return None


def _base_env(
    *,
    cache_root: Path,
    target_root: Path,
    sccache_mode: str,
    cargo_incremental: str,
) -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = "src"
    env["UV_NO_SYNC"] = "1"
    env["PYTHONHASHSEED"] = "0"
    env["MOLT_HASH_SEED"] = "0"
    env["MOLT_CACHE"] = str(cache_root)
    env["CARGO_TARGET_DIR"] = str(target_root)
    env["MOLT_USE_SCCACHE"] = sccache_mode
    env["CARGO_INCREMENTAL"] = cargo_incremental
    return env


def _terminate_process_group(
    proc: subprocess.Popen[str], *, grace_sec: float = 5.0
) -> None:
    if proc.poll() is not None:
        return
    if os.name != "nt":
        try:
            os.killpg(proc.pid, signal.SIGTERM)
        except ProcessLookupError:
            return
        try:
            proc.wait(timeout=grace_sec)
            return
        except subprocess.TimeoutExpired:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                return
    else:
        proc.terminate()
    try:
        proc.wait(timeout=2.0)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()


def _kill_run_scoped_processes(marker: str, *, include_daemon: bool) -> list[int]:
    if not marker or os.name == "nt":
        return []
    current_pid = os.getpid()
    match_tokens = ["molt.cli build", "cargo build", "rustc", "/sccache"]
    if include_daemon:
        match_tokens.append("molt-backend --daemon")
    killed: list[int] = []
    deadline = time.monotonic() + 3.0
    while True:
        ps = subprocess.run(
            ["ps", "-ax", "-o", "pid=,command="],
            capture_output=True,
            text=True,
            check=False,
        )
        if ps.returncode != 0:
            return killed
        matching: list[int] = []
        for line in ps.stdout.splitlines():
            stripped = line.strip()
            if not stripped:
                continue
            parts = stripped.split(maxsplit=1)
            if not parts:
                continue
            try:
                pid = int(parts[0])
            except ValueError:
                continue
            if pid == current_pid:
                continue
            cmd = parts[1] if len(parts) > 1 else ""
            if marker not in cmd:
                continue
            if not any(token in cmd for token in match_tokens):
                continue
            matching.append(pid)
        if not matching:
            return killed
        for pid in matching:
            try:
                os.kill(pid, signal.SIGKILL)
                killed.append(pid)
            except ProcessLookupError:
                continue
            except OSError:
                continue
        if time.monotonic() >= deadline:
            return killed
        time.sleep(0.2)
    return killed


def _run_case(
    *,
    case: CaseSpec,
    python_version: str,
    script_path: str,
    out_root: Path,
    logs_root: Path,
    repo_root: Path,
    env_base: dict[str, str],
    timeout_sec: float,
    diagnostics: bool,
    max_retries: int,
    retry_backoff_sec: float,
    build_lock_timeout_sec: float | None,
) -> CaseResult:
    heartbeat_sec = 15.0
    target_marker = env_base.get("CARGO_TARGET_DIR", "")
    cmd = [
        "uv",
        "run",
        "--python",
        python_version,
        "python3",
        "-m",
        "molt.cli",
        "build",
        script_path,
        "--profile",
        case.profile,
        "--out-dir",
        str(out_root / case.name),
    ]
    if case.cache_mode == "cache-report":
        cmd.append("--cache-report")
    else:
        cmd.append("--no-cache")
    wrapped_cmd = cmd
    if os.name != "nt":
        # Keep child builds from orphaning under PID 1 if the harness process
        # itself is terminated externally; watcher kills the shell process group.
        parent_pid = os.getpid()
        cmd_joined = shlex.join(cmd)
        marker_q = shlex.quote(target_marker)
        wrapped_cmd = [
            "/bin/bash",
            "-lc",
            (
                f"parent_pid={parent_pid}; "
                '(while kill -0 "$parent_pid" 2>/dev/null; do sleep 1; done; '
                f"for _ in 1 2 3; do pkill -f -- {marker_q} >/dev/null 2>&1 || true; sleep 0.2; done; "
                "kill -TERM -- -$$ >/dev/null 2>&1 || true) & "
                "watcher=$!; "
                f"{cmd_joined}; "
                "rc=$?; "
                'kill "$watcher" >/dev/null 2>&1 || true; '
                'wait "$watcher" >/dev/null 2>&1 || true; '
                'exit "$rc"'
            ),
        ]

    env = env_base.copy()
    env["MOLT_BACKEND_DAEMON"] = "1" if case.daemon else "0"
    env.pop("MOLT_RELEASE_CARGO_PROFILE", None)
    if case.release_cargo_profile is not None:
        env["MOLT_RELEASE_CARGO_PROFILE"] = case.release_cargo_profile
    if build_lock_timeout_sec is not None and build_lock_timeout_sec > 0:
        env["MOLT_BUILD_LOCK_TIMEOUT"] = str(build_lock_timeout_sec)
    diagnostics_path: Path | None = None
    if diagnostics:
        diagnostics_path = out_root / "diagnostics" / f"{case.name}.json"
        diagnostics_path.parent.mkdir(parents=True, exist_ok=True)
        env["MOLT_BUILD_DIAGNOSTICS"] = "1"
        env["MOLT_BUILD_DIAGNOSTICS_FILE"] = str(diagnostics_path)

    def _execute(label: str) -> tuple[int, bool, str, str, float]:
        started = time.perf_counter()
        timed_out = False
        stdout = ""
        stderr = ""
        returncode = 124
        try:
            proc = subprocess.Popen(
                wrapped_cmd,
                cwd=repo_root,
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                start_new_session=(os.name != "nt"),
            )
            deadline = started + timeout_sec
            next_heartbeat = started + heartbeat_sec
            while proc.poll() is None:
                now = time.perf_counter()
                if now >= deadline:
                    timed_out = True
                    _terminate_process_group(proc)
                    killed = _kill_run_scoped_processes(
                        target_marker,
                        include_daemon=False,
                    )
                    if killed:
                        stderr = (
                            stderr
                            + "\n[kill-timeout] pids="
                            + ",".join(str(pid) for pid in killed)
                        )
                    break
                if now >= next_heartbeat:
                    print(
                        (
                            f"[compile-progress] heartbeat case={case.name} "
                            f"label={label} elapsed={now - started:.1f}s"
                        ),
                        flush=True,
                    )
                    next_heartbeat = now + heartbeat_sec
                sleep_sec = min(
                    1.0,
                    max(0.01, deadline - now),
                    max(0.01, next_heartbeat - now),
                )
                time.sleep(sleep_sec)
            if timed_out:
                try:
                    flushed_stdout, flushed_stderr = proc.communicate(timeout=2.0)
                    stdout = stdout + (flushed_stdout or "")
                    stderr = stderr + (flushed_stderr or "")
                except subprocess.TimeoutExpired:
                    pass
            else:
                stdout, stderr = proc.communicate()
                returncode = proc.returncode
        except OSError as exc:
            stderr = str(exc)
            returncode = 1
        if "proc" in locals() and proc.returncode is not None:
            returncode = proc.returncode
        elif timed_out:
            returncode = 124
        elapsed = round(time.perf_counter() - started, 3)
        (logs_root / f"{case.name}.{label}.stdout.log").write_text(stdout)
        (logs_root / f"{case.name}.{label}.stderr.log").write_text(stderr)
        return returncode, timed_out, stdout, stderr, elapsed

    for warmup_idx in range(case.warmup_runs):
        _execute(f"warmup{warmup_idx + 1}")

    attempts = 0
    retry_reason: str | None = None
    max_attempts = max(1, max_retries + 1)
    returncode = 124
    timed_out = False
    stdout = ""
    stderr = ""
    elapsed = 0.0
    while attempts < max_attempts:
        attempts += 1
        returncode, timed_out, stdout, stderr, elapsed = _execute(f"attempt{attempts}")
        reason = _is_retryable_failure(returncode, timed_out, stderr)
        if reason is None or attempts >= max_attempts:
            break
        retry_reason = reason
        if retry_backoff_sec > 0:
            time.sleep(retry_backoff_sec * attempts)
    _kill_run_scoped_processes(target_marker, include_daemon=True)

    (logs_root / f"{case.name}.stdout.log").write_text(stdout)
    (logs_root / f"{case.name}.stderr.log").write_text(stderr)
    diagnostics_total_sec: float | None = None
    diagnostics_phase_sec: dict[str, float] | None = None
    if diagnostics_path is not None and diagnostics_path.exists():
        try:
            payload = json.loads(diagnostics_path.read_text())
            total = payload.get("total_sec")
            if isinstance(total, (int, float)):
                diagnostics_total_sec = float(total)
            phase_sec = payload.get("phase_sec")
            if isinstance(phase_sec, dict):
                parsed: dict[str, float] = {}
                for key, value in phase_sec.items():
                    if isinstance(key, str) and isinstance(value, (int, float)):
                        parsed[key] = float(value)
                diagnostics_phase_sec = parsed
        except (OSError, json.JSONDecodeError):
            diagnostics_total_sec = None
            diagnostics_phase_sec = None

    return CaseResult(
        name=case.name,
        profile=case.profile,
        cache_mode=case.cache_mode,
        daemon=case.daemon,
        release_cargo_profile=case.release_cargo_profile,
        command=cmd,
        returncode=returncode,
        elapsed_sec=elapsed,
        timed_out=timed_out,
        cache_state=_extract_cache_state(stdout),
        stdout_tail=_tail(stdout),
        stderr_tail=_tail(stderr),
        diagnostics_path=str(diagnostics_path)
        if diagnostics_path is not None
        else None,
        diagnostics_total_sec=diagnostics_total_sec,
        diagnostics_phase_sec=diagnostics_phase_sec,
        attempts=attempts,
        warmup_runs=case.warmup_runs,
        retry_reason=retry_reason,
    )


def _render_markdown(
    *,
    output_root: Path,
    target_root: Path,
    cache_root: Path,
    script_path: str,
    python_version: str,
    timeout_sec: float,
    results: list[CaseResult],
) -> str:
    lines = [
        "# Compile Progress Snapshot",
        "",
        f"- `timestamp_utc`: {dt.datetime.now(dt.timezone.utc).isoformat()}",
        f"- `output_root`: `{output_root}`",
        f"- `script`: `{script_path}`",
        f"- `python_version`: `{python_version}`",
        f"- `timeout_sec`: `{timeout_sec}`",
        f"- `cache_root`: `{cache_root}`",
        f"- `target_root`: `{target_root}`",
        "",
        "| case | profile | release_cargo_profile | cache_mode | daemon | warmups | attempts | elapsed_sec | diag_total_sec | rc | timed_out | retry_reason | cache_state |",
        "| --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- | --- |",
    ]
    for item in results:
        diag_total = (
            f"{item.diagnostics_total_sec:.3f}"
            if item.diagnostics_total_sec is not None
            else "n/a"
        )
        retry_reason = item.retry_reason or "-"
        lines.append(
            "| "
            + " | ".join(
                [
                    item.name,
                    item.profile,
                    item.release_cargo_profile or "-",
                    item.cache_mode,
                    "on" if item.daemon else "off",
                    str(item.warmup_runs),
                    str(item.attempts),
                    f"{item.elapsed_sec:.3f}",
                    diag_total,
                    str(item.returncode),
                    "yes" if item.timed_out else "no",
                    retry_reason,
                    item.cache_state,
                ]
            )
            + " |"
        )
    return "\n".join(lines) + "\n"


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Run a standard compile-progress suite and write JSON + markdown "
            "snapshots for regression tracking."
        )
    )
    parser.add_argument(
        "--output-root",
        help=(
            "Output directory for logs/results. Defaults to "
            "/Volumes/APDataStore/Molt/... when available, else "
            "bench/results/compile_progress/..."
        ),
    )
    parser.add_argument(
        "--python-version",
        default="3.12",
        help="uv Python version for measurements (default: 3.12).",
    )
    parser.add_argument(
        "--script",
        default="examples/hello.py",
        help="Python script used for compile timing (default: examples/hello.py).",
    )
    parser.add_argument(
        "--cases",
        nargs="+",
        choices=[case.name for case in CASE_SPECS],
        default=list(DEFAULT_CASES),
        help=(
            "Subset of suite cases to run (default: baseline cases; "
            "queue cases are opt-in)."
        ),
    )
    parser.add_argument(
        "--timeout-sec",
        type=float,
        default=900.0,
        help="Per-case timeout in seconds (default: 900).",
    )
    parser.add_argument(
        "--clean-state",
        action="store_true",
        help="Remove target/cache roots under output-root before running.",
    )
    parser.add_argument(
        "--sccache-mode",
        default="1",
        help="Set MOLT_USE_SCCACHE (default: 1).",
    )
    parser.add_argument(
        "--cargo-incremental",
        default="0",
        help="Set CARGO_INCREMENTAL (default: 0).",
    )
    parser.add_argument(
        "--max-retries",
        type=int,
        default=2,
        help=(
            "Additional retries for timeout/lock-timeout failures "
            "(default: 2, meaning up to 3 attempts total)."
        ),
    )
    parser.add_argument(
        "--retry-backoff-sec",
        type=float,
        default=2.0,
        help="Linear backoff in seconds between retries (default: 2.0).",
    )
    parser.add_argument(
        "--build-lock-timeout-sec",
        type=float,
        default=60.0,
        help=(
            "Set MOLT_BUILD_LOCK_TIMEOUT for each case to fail fast under "
            "contention and allow retries (default: 60)."
        ),
    )
    parser.add_argument(
        "--diagnostics",
        action="store_true",
        help=(
            "Enable MOLT_BUILD_DIAGNOSTICS for each case and capture "
            "per-case diagnostics JSON files."
        ),
    )
    parser.add_argument(
        "--resume",
        action="store_true",
        help=(
            "Resume from an existing compile_progress.json in output-root by "
            "skipping already-completed cases."
        ),
    )
    return parser.parse_args()


def _coerce_case_result(payload: dict[str, Any]) -> CaseResult | None:
    name = payload.get("name")
    profile = payload.get("profile")
    cache_mode = payload.get("cache_mode")
    if (
        not isinstance(name, str)
        or not isinstance(profile, str)
        or not isinstance(cache_mode, str)
    ):
        return None
    command = payload.get("command")
    if isinstance(command, list):
        command_list = [str(item) for item in command]
    else:
        command_list = []
    return CaseResult(
        name=name,
        profile=profile,
        cache_mode=cache_mode,
        daemon=bool(payload.get("daemon", False)),
        release_cargo_profile=(
            str(payload["release_cargo_profile"])
            if payload.get("release_cargo_profile") is not None
            else None
        ),
        command=command_list,
        returncode=int(payload.get("returncode", 1)),
        elapsed_sec=float(payload.get("elapsed_sec", 0.0)),
        timed_out=bool(payload.get("timed_out", False)),
        cache_state=str(payload.get("cache_state", "n/a")),
        stdout_tail=str(payload.get("stdout_tail", "")),
        stderr_tail=str(payload.get("stderr_tail", "")),
        diagnostics_path=(
            str(payload["diagnostics_path"])
            if payload.get("diagnostics_path") is not None
            else None
        ),
        diagnostics_total_sec=(
            float(payload["diagnostics_total_sec"])
            if isinstance(payload.get("diagnostics_total_sec"), (int, float))
            else None
        ),
        diagnostics_phase_sec=(
            {
                str(k): float(v)
                for k, v in payload.get("diagnostics_phase_sec", {}).items()
                if isinstance(k, str) and isinstance(v, (int, float))
            }
            if isinstance(payload.get("diagnostics_phase_sec"), dict)
            else None
        ),
        attempts=int(payload.get("attempts", 1)),
        warmup_runs=int(payload.get("warmup_runs", 0)),
        retry_reason=(
            str(payload["retry_reason"])
            if payload.get("retry_reason") is not None
            else None
        ),
    )


def _write_snapshot(
    *,
    output_root: Path,
    repo_root: Path,
    target_root: Path,
    cache_root: Path,
    args: argparse.Namespace,
    results: list[CaseResult],
) -> tuple[Path, Path]:
    payload: dict[str, Any] = {
        "meta": {
            "timestamp_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
            "repo_root": str(repo_root),
            "output_root": str(output_root),
            "script": args.script,
            "python_version": args.python_version,
            "cases": args.cases,
            "completed_cases": [item.name for item in results],
            "timeout_sec": args.timeout_sec,
            "cache_root": str(cache_root),
            "target_root": str(target_root),
            "clean_state": bool(args.clean_state),
            "sccache_mode": args.sccache_mode,
            "cargo_incremental": args.cargo_incremental,
            "diagnostics": bool(args.diagnostics),
            "max_retries": int(args.max_retries),
            "retry_backoff_sec": float(args.retry_backoff_sec),
            "build_lock_timeout_sec": float(args.build_lock_timeout_sec),
        },
        "results": [asdict(item) for item in results],
    }
    json_path = output_root / "compile_progress.json"
    json_path.write_text(json.dumps(payload, indent=2) + "\n")

    markdown_path = output_root / "compile_progress.md"
    markdown_path.write_text(
        _render_markdown(
            output_root=output_root,
            target_root=target_root,
            cache_root=cache_root,
            script_path=args.script,
            python_version=args.python_version,
            timeout_sec=args.timeout_sec,
            results=results,
        )
    )
    return json_path, markdown_path


def main() -> int:
    args = _parse_args()
    repo_root = Path(__file__).resolve().parent.parent
    output_root = (
        Path(args.output_root).expanduser().resolve()
        if args.output_root
        else _default_output_root(repo_root)
    )
    output_root.mkdir(parents=True, exist_ok=True)

    target_root = output_root / "target"
    cache_root = output_root / "cache"
    logs_root = output_root / "logs"
    logs_root.mkdir(parents=True, exist_ok=True)

    if args.clean_state:
        for path in (target_root, cache_root):
            if path.exists():
                shutil.rmtree(path)

    target_root.mkdir(parents=True, exist_ok=True)
    cache_root.mkdir(parents=True, exist_ok=True)

    env_base = _base_env(
        cache_root=cache_root,
        target_root=target_root,
        sccache_mode=args.sccache_mode,
        cargo_incremental=args.cargo_incremental,
    )
    results: list[CaseResult] = []
    if args.resume:
        existing_json = output_root / "compile_progress.json"
        if existing_json.exists():
            try:
                existing_payload = json.loads(existing_json.read_text())
                existing_results = existing_payload.get("results", [])
                if isinstance(existing_results, list):
                    for item in existing_results:
                        if isinstance(item, dict):
                            restored = _coerce_case_result(item)
                            if restored is not None:
                                results.append(restored)
            except (OSError, json.JSONDecodeError):
                pass

    completed = {item.name for item in results}
    for name in args.cases:
        if name in completed:
            print(f"[compile-progress] case={name} skipped=resume", flush=True)
            continue
        case = CASE_BY_NAME[name]
        print(f"[compile-progress] case={case.name}", flush=True)
        result = _run_case(
            case=case,
            python_version=args.python_version,
            script_path=args.script,
            out_root=output_root / "out",
            logs_root=logs_root,
            repo_root=repo_root,
            env_base=env_base,
            timeout_sec=args.timeout_sec,
            diagnostics=bool(args.diagnostics),
            max_retries=max(0, int(args.max_retries)),
            retry_backoff_sec=max(0.0, float(args.retry_backoff_sec)),
            build_lock_timeout_sec=(
                None
                if args.build_lock_timeout_sec is None
                else float(args.build_lock_timeout_sec)
            ),
        )
        results.append(result)
        completed.add(result.name)
        print(
            (
                f"[compile-progress] case={case.name} "
                f"elapsed={result.elapsed_sec}s rc={result.returncode} "
                f"cache={result.cache_state} attempts={result.attempts} "
                f"warmups={result.warmup_runs}"
            ),
            flush=True,
        )
        _write_snapshot(
            output_root=output_root,
            repo_root=repo_root,
            target_root=target_root,
            cache_root=cache_root,
            args=args,
            results=results,
        )

    json_path, markdown_path = _write_snapshot(
        output_root=output_root,
        repo_root=repo_root,
        target_root=target_root,
        cache_root=cache_root,
        args=args,
        results=results,
    )

    print(f"wrote {json_path}")
    print(f"wrote {markdown_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
