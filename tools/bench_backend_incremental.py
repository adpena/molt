#!/usr/bin/env python3
"""Benchmark backend incremental transpilation for rust/luau targets.

Runs cold, warm, and edit builds per target/profile and writes JSON results
with timings and executed commands.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import shutil
import signal
import subprocess
import sys
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

TARGET_CHOICES = ("rust", "luau")
PROFILE_CHOICES = ("dev", "release")
TARGET_EXTENSION = {"rust": "rs", "luau": "luau"}
REPO_ROOT = Path(__file__).resolve().parent.parent


@dataclass
class PhaseResult:
    phase: str
    command: list[str]
    cwd: str
    returncode: int
    elapsed_sec: float
    timed_out: bool
    stdout_tail: str
    stderr_tail: str


@dataclass
class CaseResult:
    target: str
    profile: str
    source_copy: str
    output_file: str
    env_paths: dict[str, str]
    edit_marker: str
    phases: list[PhaseResult]


def _tail(text: str, lines: int = 12) -> str:
    if not text:
        return ""
    return "\n".join(text.splitlines()[-lines:])


def _default_artifact_root() -> Path:
    configured = os.environ.get("MOLT_EXT_ROOT", "").strip()
    if configured:
        return Path(configured).expanduser().resolve()
    return REPO_ROOT


def _resolve_output_root(output_root: str | None) -> tuple[Path, Path]:
    if output_root:
        root = Path(output_root).expanduser().resolve()
        return root, root
    stamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    artifact_root = _default_artifact_root()
    return (
        artifact_root / "tmp" / f"bench_backend_incremental_{stamp}",
        artifact_root,
    )


def _terminate_process(proc: subprocess.Popen[str]) -> None:
    if proc.poll() is not None:
        return
    if os.name != "nt":
        try:
            os.killpg(proc.pid, signal.SIGTERM)
        except ProcessLookupError:
            return
        try:
            proc.wait(timeout=3)
            return
        except subprocess.TimeoutExpired:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                return
    else:
        proc.terminate()
    try:
        proc.wait(timeout=2)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()


def _run_command(
    command: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout_sec: float,
    phase: str,
) -> PhaseResult:
    start = time.perf_counter()
    proc = subprocess.Popen(
        command,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=(os.name != "nt"),
    )
    timed_out = False
    stdout = ""
    stderr = ""
    returncode = 124
    try:
        stdout, stderr = proc.communicate(timeout=timeout_sec)
        returncode = proc.returncode
    except subprocess.TimeoutExpired:
        timed_out = True
        _terminate_process(proc)
        try:
            flushed_stdout, flushed_stderr = proc.communicate(timeout=2)
        except subprocess.TimeoutExpired:
            flushed_stdout = ""
            flushed_stderr = ""
        stdout = flushed_stdout or ""
        stderr = flushed_stderr or ""
        returncode = 124
    elapsed = round(time.perf_counter() - start, 3)
    return PhaseResult(
        phase=phase,
        command=command,
        cwd=str(cwd),
        returncode=returncode,
        elapsed_sec=elapsed,
        timed_out=timed_out,
        stdout_tail=_tail(stdout),
        stderr_tail=_tail(stderr),
    )


def _case_env(
    *,
    case_root: Path,
    molt_ext_root: Path,
) -> tuple[dict[str, str], dict[str, str]]:
    target_root = case_root / "target"
    cache_root = case_root / ".molt_cache"
    tmp_root = case_root / "tmp"
    diff_root = tmp_root / "diff"
    uv_cache_root = case_root / ".uv-cache"
    for path in (target_root, cache_root, diff_root, tmp_root, uv_cache_root):
        path.mkdir(parents=True, exist_ok=True)

    env = os.environ.copy()
    repo_src = str(REPO_ROOT / "src")
    current_pythonpath = env.get("PYTHONPATH", "")
    env["PYTHONPATH"] = (
        repo_src + os.pathsep + current_pythonpath if current_pythonpath else repo_src
    )
    env["PYTHONHASHSEED"] = "0"
    env["MOLT_HASH_SEED"] = "0"
    env["MOLT_EXT_ROOT"] = str(molt_ext_root)
    env["CARGO_TARGET_DIR"] = str(target_root)
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = str(target_root)
    env["MOLT_CACHE"] = str(cache_root)
    env["MOLT_DIFF_ROOT"] = str(diff_root)
    env["MOLT_DIFF_TMPDIR"] = str(tmp_root)
    env["UV_CACHE_DIR"] = str(uv_cache_root)
    env["TMPDIR"] = str(tmp_root)

    return env, {
        "MOLT_EXT_ROOT": str(molt_ext_root),
        "CARGO_TARGET_DIR": str(target_root),
        "MOLT_DIFF_CARGO_TARGET_DIR": str(target_root),
        "MOLT_CACHE": str(cache_root),
        "MOLT_DIFF_ROOT": str(diff_root),
        "MOLT_DIFF_TMPDIR": str(tmp_root),
        "UV_CACHE_DIR": str(uv_cache_root),
        "TMPDIR": str(tmp_root),
    }


def _build_command(
    *,
    source_path: Path,
    output_path: Path,
    target: str,
    profile: str,
) -> list[str]:
    return [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        str(source_path),
        "--target",
        target,
        "--profile",
        profile,
        "--output",
        str(output_path),
        "--cache-report",
    ]


def _apply_controlled_edit(source_copy: Path) -> str:
    marker = "__molt_incremental_edit_marker__ = 1"
    with source_copy.open("a", encoding="utf-8") as handle:
        handle.write("\n")
        handle.write(marker)
        handle.write("\n")
    return marker


def _run_case(
    *,
    repo_root: Path,
    source_path: Path,
    output_root: Path,
    molt_ext_root: Path,
    target: str,
    profile: str,
    timeout_sec: float,
) -> CaseResult:
    case_root = output_root / f"{target}_{profile}"
    if case_root.exists():
        shutil.rmtree(case_root)
    work_root = case_root / "work"
    artifacts_root = case_root / "artifacts"
    work_root.mkdir(parents=True, exist_ok=True)
    artifacts_root.mkdir(parents=True, exist_ok=True)

    source_copy = work_root / source_path.name
    shutil.copy2(source_path, source_copy)
    output_path = artifacts_root / f"program.{TARGET_EXTENSION[target]}"

    env, env_paths = _case_env(case_root=case_root, molt_ext_root=molt_ext_root)

    phases: list[PhaseResult] = []
    cold_cmd = _build_command(
        source_path=source_copy,
        output_path=output_path,
        target=target,
        profile=profile,
    )
    phases.append(
        _run_command(
            cold_cmd,
            cwd=repo_root,
            env=env,
            timeout_sec=timeout_sec,
            phase="cold",
        )
    )

    warm_cmd = _build_command(
        source_path=source_copy,
        output_path=output_path,
        target=target,
        profile=profile,
    )
    phases.append(
        _run_command(
            warm_cmd,
            cwd=repo_root,
            env=env,
            timeout_sec=timeout_sec,
            phase="warm",
        )
    )

    edit_marker = _apply_controlled_edit(source_copy)
    edit_cmd = _build_command(
        source_path=source_copy,
        output_path=output_path,
        target=target,
        profile=profile,
    )
    phases.append(
        _run_command(
            edit_cmd,
            cwd=repo_root,
            env=env,
            timeout_sec=timeout_sec,
            phase="edit",
        )
    )

    return CaseResult(
        target=target,
        profile=profile,
        source_copy=str(source_copy),
        output_file=str(output_path),
        env_paths=env_paths,
        edit_marker=edit_marker,
        phases=phases,
    )


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Benchmark Molt transpiler incremental behavior for rust/luau targets "
            "across cold, warm, and edit phases."
        )
    )
    parser.add_argument(
        "--source",
        default="examples/hello.py",
        help="Source file to benchmark (default: examples/hello.py).",
    )
    parser.add_argument(
        "--targets",
        nargs="+",
        choices=TARGET_CHOICES,
        default=list(TARGET_CHOICES),
        help="Targets to benchmark (default: rust luau).",
    )
    parser.add_argument(
        "--profiles",
        nargs="+",
        choices=PROFILE_CHOICES,
        default=list(PROFILE_CHOICES),
        help="Build profiles to benchmark (default: dev release).",
    )
    parser.add_argument(
        "--timeout-sec",
        type=float,
        default=600.0,
        help="Per-phase timeout in seconds (default: 600).",
    )
    parser.add_argument(
        "--output-root",
        help=(
            "Output directory root. If omitted, defaults under the configured "
            "artifact root (`MOLT_EXT_ROOT` when set, otherwise repo-local `tmp/`)."
        ),
    )
    parser.add_argument(
        "--json-out",
        help="Optional JSON output path (default: <output-root>/results.json).",
    )
    return parser.parse_args()


def main() -> int:
    args = _parse_args()
    repo_root = Path(__file__).resolve().parent.parent
    source_path = Path(args.source)
    if not source_path.is_absolute():
        source_path = (repo_root / source_path).resolve()
    if not source_path.is_file():
        print(f"error: source file not found: {source_path}", file=sys.stderr)
        return 2

    output_root, molt_ext_root = _resolve_output_root(args.output_root)
    output_root.mkdir(parents=True, exist_ok=True)
    json_path = (
        Path(args.json_out).expanduser().resolve()
        if args.json_out
        else output_root / "results.json"
    )
    json_path.parent.mkdir(parents=True, exist_ok=True)

    all_results: list[CaseResult] = []
    for target in args.targets:
        for profile in args.profiles:
            print(
                f"[bench-backend-incremental] target={target} profile={profile}",
                flush=True,
            )
            result = _run_case(
                repo_root=repo_root,
                source_path=source_path,
                output_root=output_root,
                molt_ext_root=molt_ext_root,
                target=target,
                profile=profile,
                timeout_sec=float(args.timeout_sec),
            )
            all_results.append(result)
            phase_summary = ", ".join(
                f"{phase.phase}:{phase.elapsed_sec:.3f}s(rc={phase.returncode})"
                for phase in result.phases
            )
            print(
                (
                    "[bench-backend-incremental] "
                    f"target={target} profile={profile} phases={phase_summary}"
                ),
                flush=True,
            )

    payload: dict[str, Any] = {
        "meta": {
            "timestamp_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
            "repo_root": str(repo_root),
            "source": str(source_path),
            "output_root": str(output_root),
            "json_path": str(json_path),
            "targets": list(args.targets),
            "profiles": list(args.profiles),
            "timeout_sec": float(args.timeout_sec),
            "python_executable": sys.executable,
        },
        "results": [asdict(item) for item in all_results],
    }
    json_path.write_text(json.dumps(payload, indent=2) + "\n")
    print(f"wrote {json_path}")

    failed = any(
        phase.returncode != 0 or phase.timed_out
        for case in all_results
        for phase in case.phases
    )
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
