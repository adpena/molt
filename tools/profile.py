#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import platform
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BENCH_DIR = ROOT / "tests" / "benchmarks"

TOP_BENCHES = [
    "bench_deeply_nested_loop.py",
    "bench_sum_list.py",
    "bench_tuple_index.py",
    "bench_tuple_pack.py",
    "bench_attr_access.py",
    "bench_dict_ops.py",
    "bench_list_ops.py",
    "bench_str_find_unicode.py",
    "bench_str_count_unicode.py",
    "bench_bytes_find.py",
]

SMOKE_BENCHES = [
    "bench_sum.py",
    "bench_bytes_find.py",
]

MOLT_ARGS_BY_BENCH = {
    "bench_sum_list_hints.py": ["--type-hints", "trust"],
}


@dataclass(frozen=True)
class ToolRunResult:
    tool: str
    command: list[str]
    log_path: str
    output_files: list[str]
    duration_s: float
    returncode: int


def _git_rev() -> str | None:
    try:
        res = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            check=False,
            cwd=ROOT,
        )
    except OSError:
        return None
    if res.returncode != 0:
        return None
    return res.stdout.strip() or None


def _utc_stamp() -> str:
    return dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def _base_env() -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = "src"
    return env


def _resolve_bench_path(name: str) -> Path:
    path = Path(name)
    if path.is_file():
        return path
    candidate = BENCH_DIR / name
    if candidate.is_file():
        return candidate
    if not name.endswith(".py"):
        candidate = BENCH_DIR / f"{name}.py"
        if candidate.is_file():
            return candidate
    raise FileNotFoundError(f"Unknown benchmark: {name}")


def _list_all_benches() -> list[Path]:
    return sorted(BENCH_DIR.glob("bench_*.py"))


def _run_command(
    command: list[str], *, env: dict[str, str], log_path: Path
) -> tuple[int, float]:
    start = time.perf_counter()
    with log_path.open("w", encoding="utf-8") as log_file:
        res = subprocess.run(
            command,
            env=env,
            cwd=ROOT,
            text=True,
            stdout=log_file,
            stderr=log_file,
        )
    end = time.perf_counter()
    return res.returncode, end - start


def _time_binary() -> str:
    for candidate in ("/usr/bin/time", shutil.which("time")):
        if candidate and Path(candidate).exists():
            return candidate
    raise FileNotFoundError("time binary not found")


def _time_flags() -> list[str]:
    if sys.platform == "darwin":
        return ["-l"]
    return ["-v"]


def _run_with_time(
    cmd: list[str], *, env: dict[str, str], out_dir: Path, name: str, tool: str
) -> ToolRunResult:
    time_bin = _time_binary()
    log_path = out_dir / f"{name}_{tool}.log"
    command = [time_bin, *_time_flags(), *cmd]
    returncode, duration = _run_command(command, env=env, log_path=log_path)
    return ToolRunResult(
        tool=tool,
        command=command,
        log_path=str(log_path),
        output_files=[],
        duration_s=duration,
        returncode=returncode,
    )


def _run_with_perf(
    cmd: list[str], *, env: dict[str, str], out_dir: Path, name: str
) -> ToolRunResult:
    perf_bin = shutil.which("perf")
    if not perf_bin:
        raise FileNotFoundError("perf not found")
    perf_out = out_dir / f"{name}.perf.data"
    log_path = out_dir / f"{name}_perf.log"
    command = [perf_bin, "record", "-F", "99", "-g", "-o", str(perf_out), "--", *cmd]
    returncode, duration = _run_command(command, env=env, log_path=log_path)
    return ToolRunResult(
        tool="perf",
        command=command,
        log_path=str(log_path),
        output_files=[str(perf_out)],
        duration_s=duration,
        returncode=returncode,
    )


def _run_with_heaptrack(
    cmd: list[str], *, env: dict[str, str], out_dir: Path, name: str
) -> ToolRunResult:
    heaptrack_bin = shutil.which("heaptrack")
    if not heaptrack_bin:
        raise FileNotFoundError("heaptrack not found")
    heap_out = out_dir / f"{name}.heaptrack"
    log_path = out_dir / f"{name}_heaptrack.log"
    command = [heaptrack_bin, "-o", str(heap_out), "--", *cmd]
    returncode, duration = _run_command(command, env=env, log_path=log_path)
    return ToolRunResult(
        tool="heaptrack",
        command=command,
        log_path=str(log_path),
        output_files=[str(heap_out)],
        duration_s=duration,
        returncode=returncode,
    )


def _run_with_massif(
    cmd: list[str], *, env: dict[str, str], out_dir: Path, name: str
) -> ToolRunResult:
    valgrind_bin = shutil.which("valgrind")
    if not valgrind_bin:
        raise FileNotFoundError("valgrind not found")
    massif_out = out_dir / f"{name}.massif.out"
    log_path = out_dir / f"{name}_massif.log"
    command = [
        valgrind_bin,
        "--tool=massif",
        f"--massif-out-file={massif_out}",
        "--",
        *cmd,
    ]
    returncode, duration = _run_command(command, env=env, log_path=log_path)
    return ToolRunResult(
        tool="massif",
        command=command,
        log_path=str(log_path),
        output_files=[str(massif_out)],
        duration_s=duration,
        returncode=returncode,
    )


def _build_molt(bench: Path, extra_args: list[str]) -> None:
    binary = ROOT / "hello_molt"
    if binary.exists():
        binary.unlink()
    env = _base_env()
    command = [sys.executable, "-m", "molt.cli", "build", *extra_args, str(bench)]
    res = subprocess.run(command, env=env, cwd=ROOT, capture_output=True, text=True)
    if res.returncode != 0:
        stderr = res.stderr.strip() or res.stdout.strip()
        raise RuntimeError(f"molt build failed: {stderr}")


def _resolve_benches(args: argparse.Namespace) -> list[Path]:
    if args.bench:
        return [_resolve_bench_path(name) for name in args.bench]
    if args.suite == "smoke":
        return [_resolve_bench_path(name) for name in SMOKE_BENCHES]
    if args.suite == "top":
        return [_resolve_bench_path(name) for name in TOP_BENCHES]
    return _list_all_benches()


def _pick_cpu_tool(requested: str) -> str:
    if requested != "auto":
        return requested
    if shutil.which("perf"):
        return "perf"
    return "time"


def _pick_alloc_tool(requested: str) -> str:
    if requested != "auto":
        return requested
    if shutil.which("heaptrack"):
        return "heaptrack"
    if shutil.which("valgrind"):
        return "massif"
    return "time"


def _run_tool(
    tool: str, cmd: list[str], *, env: dict[str, str], out_dir: Path, name: str
) -> ToolRunResult | None:
    if tool == "none":
        return None
    if tool == "perf":
        return _run_with_perf(cmd, env=env, out_dir=out_dir, name=name)
    if tool == "heaptrack":
        return _run_with_heaptrack(cmd, env=env, out_dir=out_dir, name=name)
    if tool == "massif":
        return _run_with_massif(cmd, env=env, out_dir=out_dir, name=name)
    if tool == "time":
        return _run_with_time(cmd, env=env, out_dir=out_dir, name=name, tool=tool)
    raise ValueError(f"Unknown tool: {tool}")


def _profile_bench(
    bench: Path,
    *,
    cpu_tool: str,
    alloc_tool: str,
    out_dir: Path,
    extra_args: list[str],
    runs: int,
) -> dict[str, object]:
    bench_name = bench.stem
    bench_dir = out_dir / bench_name
    bench_dir.mkdir(parents=True, exist_ok=True)
    extra = extra_args + MOLT_ARGS_BY_BENCH.get(bench.name, [])
    _build_molt(bench, extra)
    cmd = ["./hello_molt"]
    env = _base_env()

    results: list[ToolRunResult] = []
    for idx in range(runs):
        suffix = f"{bench_name}_cpu_{idx + 1}" if runs > 1 else f"{bench_name}_cpu"
        cpu_result = _run_tool(cpu_tool, cmd, env=env, out_dir=bench_dir, name=suffix)
        if cpu_result:
            results.append(cpu_result)

    if alloc_tool == cpu_tool:
        alloc_results = list(results)
    else:
        alloc_results = []
        for idx in range(runs):
            suffix = (
                f"{bench_name}_alloc_{idx + 1}" if runs > 1 else f"{bench_name}_alloc"
            )
            alloc_result = _run_tool(
                alloc_tool, cmd, env=env, out_dir=bench_dir, name=suffix
            )
            if alloc_result:
                alloc_results.append(alloc_result)

    return {
        "bench": bench.name,
        "binary": str(ROOT / "hello_molt"),
        "cpu_tool": cpu_tool,
        "alloc_tool": alloc_tool,
        "cpu_runs": [result.__dict__ for result in results],
        "alloc_runs": [result.__dict__ for result in alloc_results],
    }


def _profile_compiler(
    bench: Path, *, out_dir: Path, extra_args: list[str], runs: int
) -> dict[str, object]:
    bench_name = bench.stem
    bench_dir = out_dir / bench_name
    bench_dir.mkdir(parents=True, exist_ok=True)
    extra = extra_args + MOLT_ARGS_BY_BENCH.get(bench.name, [])
    env = _base_env()
    results: list[ToolRunResult] = []
    for idx in range(runs):
        suffix = (
            f"{bench_name}_compile_{idx + 1}" if runs > 1 else f"{bench_name}_compile"
        )
        log_path = bench_dir / f"{suffix}.log"
        pstats_path = bench_dir / f"{suffix}.pstats"
        command = [
            sys.executable,
            "-m",
            "cProfile",
            "-o",
            str(pstats_path),
            "-m",
            "molt.cli",
            "build",
            *extra,
            str(bench),
        ]
        returncode, duration = _run_command(command, env=env, log_path=log_path)
        results.append(
            ToolRunResult(
                tool="cProfile",
                command=command,
                log_path=str(log_path),
                output_files=[str(pstats_path)],
                duration_s=duration,
                returncode=returncode,
            )
        )
    return {
        "bench": bench.name,
        "compiler_runs": [result.__dict__ for result in results],
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Profile Molt benchmarks.")
    parser.add_argument("--bench", action="append", help="Benchmark file or name.")
    parser.add_argument(
        "--suite",
        choices=("top", "smoke", "all"),
        default="top",
        help="Bench suite to profile when --bench is omitted.",
    )
    parser.add_argument(
        "--cpu-tool",
        choices=("auto", "perf", "time", "none"),
        default="auto",
        help="CPU profiling tool (auto prefers perf, else time).",
    )
    parser.add_argument(
        "--alloc-tool",
        choices=("auto", "heaptrack", "massif", "time", "none"),
        default="auto",
        help="Allocation tool (auto prefers heaptrack/massif, else time).",
    )
    parser.add_argument(
        "--profile-compiler",
        action="store_true",
        help="Profile compiler time via cProfile during build.",
    )
    parser.add_argument(
        "--runs",
        type=int,
        default=1,
        help="Repeat each profiling run this many times.",
    )
    parser.add_argument(
        "--molt-arg",
        action="append",
        default=[],
        help="Extra args passed to `molt.cli build`.",
    )
    parser.add_argument(
        "--out-dir",
        default=None,
        help="Output directory (default logs/benchmarks/profile_<timestamp>).",
    )
    args = parser.parse_args()

    benches = _resolve_benches(args)
    stamp = _utc_stamp()
    out_dir = (
        Path(args.out_dir)
        if args.out_dir
        else ROOT / "logs" / "benchmarks" / f"profile_{stamp}"
    )
    out_dir.mkdir(parents=True, exist_ok=True)

    cpu_tool = _pick_cpu_tool(args.cpu_tool)
    alloc_tool = _pick_alloc_tool(args.alloc_tool)

    metadata: dict[str, object] = {
        "timestamp": stamp,
        "git_rev": _git_rev(),
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": sys.version.split()[0],
        },
        "cpu_tool": cpu_tool,
        "alloc_tool": alloc_tool,
        "benchmarks": [],
        "compiler_profiles": [],
    }

    for bench in benches:
        metadata["benchmarks"].append(
            _profile_bench(
                bench,
                cpu_tool=cpu_tool,
                alloc_tool=alloc_tool,
                out_dir=out_dir,
                extra_args=args.molt_arg,
                runs=args.runs,
            )
        )
        if args.profile_compiler:
            metadata["compiler_profiles"].append(
                _profile_compiler(
                    bench, out_dir=out_dir, extra_args=args.molt_arg, runs=args.runs
                )
            )

    manifest_path = out_dir / "profile_manifest.json"
    manifest_path.write_text(json.dumps(metadata, indent=2) + "\n")
    print(f"Profile outputs saved to {out_dir}")
    print(f"Manifest: {manifest_path}")


if __name__ == "__main__":
    main()
