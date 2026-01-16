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
    "bench_csv_parse.py",
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
DEFAULT_PERF_STAT_EVENTS = "cycles,instructions,cache-misses,branches,branch-misses"


@dataclass(frozen=True)
class ToolRunResult:
    tool: str
    command: list[str]
    log_path: str
    output_files: list[str]
    duration_s: float
    returncode: int
    metrics: dict[str, object] | None = None


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


def _apply_env_overrides(env: dict[str, str], overrides: list[str]) -> None:
    for item in overrides:
        if "=" not in item:
            raise ValueError(f"Invalid env override (expected KEY=VALUE): {item}")
        key, value = item.split("=", 1)
        env[key] = value


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


def _parse_time_metrics(log_text: str) -> dict[str, object]:
    metrics: dict[str, object] = {}
    if sys.platform == "darwin":
        for line in log_text.splitlines():
            if "maximum resident set size" in line:
                parts = line.strip().split()
                if parts:
                    try:
                        metrics["max_rss_bytes"] = int(parts[0])
                    except ValueError:
                        continue
            parts = line.strip().split()
            if len(parts) >= 6 and parts[1] == "real":
                try:
                    metrics["time_real_s"] = float(parts[0])
                    metrics["time_user_s"] = float(parts[2])
                    metrics["time_sys_s"] = float(parts[4])
                except ValueError:
                    continue
    else:
        for line in log_text.splitlines():
            if "Maximum resident set size" in line:
                _, _, val = line.partition(":")
                try:
                    metrics["max_rss_kb"] = int(val.strip())
                except ValueError:
                    continue
            if "User time (seconds)" in line:
                _, _, val = line.partition(":")
                try:
                    metrics["time_user_s"] = float(val.strip())
                except ValueError:
                    continue
            if "System time (seconds)" in line:
                _, _, val = line.partition(":")
                try:
                    metrics["time_sys_s"] = float(val.strip())
                except ValueError:
                    continue
            if "Elapsed (wall clock)" in line:
                _, _, val = line.partition(":")
                metrics["time_elapsed"] = val.strip()
    return metrics


def _parse_perf_stat(log_text: str) -> dict[str, object]:
    metrics: dict[str, object] = {}
    events: dict[str, float] = {}
    for line in log_text.splitlines():
        parts = [p.strip() for p in line.split(",")]
        if len(parts) < 3:
            continue
        value, _, event = parts[0], parts[1], parts[2]
        if value in {"<not supported>", "<not counted>", ""}:
            continue
        try:
            events[event] = float(value)
        except ValueError:
            continue
    if events:
        metrics["perf_stat"] = events
    return metrics


def _parse_molt_profile(log_text: str) -> dict[str, object] | None:
    last_line = None
    for line in log_text.splitlines():
        if line.startswith("molt_profile "):
            last_line = line
    if not last_line:
        return None
    payload = last_line[len("molt_profile ") :].strip()
    if not payload:
        return None
    parsed: dict[str, object] = {}
    for item in payload.split():
        if "=" not in item:
            continue
        key, raw = item.split("=", 1)
        try:
            parsed[key] = int(raw)
        except ValueError:
            parsed[key] = raw
    return parsed or None


def _parse_molt_profile_cpu_features(log_text: str) -> dict[str, object] | None:
    last_line = None
    for line in log_text.splitlines():
        if line.startswith("molt_profile_cpu_features "):
            last_line = line
    if not last_line:
        return None
    payload = last_line[len("molt_profile_cpu_features ") :].strip()
    if not payload:
        return None
    parsed: dict[str, object] = {}
    for item in payload.split():
        if "=" not in item:
            continue
        key, raw = item.split("=", 1)
        try:
            parsed[key] = int(raw)
        except ValueError:
            parsed[key] = raw
    return parsed or None


def _parse_molt_profile_string_sites(
    log_text: str,
) -> list[dict[str, object]] | None:
    sites: list[dict[str, object]] = []
    for line in log_text.splitlines():
        if not line.startswith("molt_profile_string_site "):
            continue
        payload = line[len("molt_profile_string_site ") :].strip()
        if not payload:
            continue
        parsed: dict[str, object] = {}
        for item in payload.split():
            if "=" not in item:
                continue
            key, raw = item.split("=", 1)
            if key in {"line", "count"}:
                try:
                    parsed[key] = int(raw)
                except ValueError:
                    parsed[key] = raw
            else:
                parsed[key] = raw
        if parsed:
            sites.append(parsed)
    return sites or None


def _merge_profile_metrics(
    metrics: dict[str, object], log_path: Path, collect_profile: bool
) -> dict[str, object]:
    if not collect_profile:
        return metrics
    log_text = log_path.read_text(encoding="utf-8", errors="replace")
    profile = _parse_molt_profile(log_text)
    if profile:
        metrics["molt_profile"] = profile
    cpu_features = _parse_molt_profile_cpu_features(log_text)
    if cpu_features:
        metrics["molt_profile_cpu_features"] = cpu_features
    string_sites = _parse_molt_profile_string_sites(log_text)
    if string_sites:
        metrics["molt_profile_string_sites"] = string_sites
    return metrics


def _collect_profile_summary(
    metadata: dict[str, object], top_n: int
) -> dict[str, object]:
    benches: list[dict[str, object]] = []
    missing_profile: list[str] = []
    string_sites: dict[tuple[str, int], int] = {}

    for entry in metadata.get("benchmarks", []):
        bench_name = entry.get("bench", "unknown")
        profile = None
        sites = None
        for run in entry.get("cpu_runs", []):
            metrics = run.get("metrics") or {}
            profile = profile or metrics.get("molt_profile")
            sites = sites or metrics.get("molt_profile_string_sites")
        if not profile:
            missing_profile.append(str(bench_name))
            continue
        call_dispatch = int(profile.get("call_dispatch", 0) or 0)
        alloc_count = int(profile.get("alloc_count", 0) or 0)
        allocs_per_call = (
            float(alloc_count) / float(call_dispatch) if call_dispatch else None
        )
        benches.append(
            {
                "bench": bench_name,
                "alloc_count": alloc_count,
                "string_allocs": int(profile.get("string_allocs", 0) or 0),
                "bytes_allocs": int(profile.get("bytes_allocs", 0) or 0),
                "bytearray_allocs": int(profile.get("bytearray_allocs", 0) or 0),
                "tuple_allocs": int(profile.get("tuple_allocs", 0) or 0),
                "list_allocs": int(profile.get("list_allocs", 0) or 0),
                "dict_allocs": int(profile.get("dict_allocs", 0) or 0),
                "iter_allocs": int(profile.get("iter_allocs", 0) or 0),
                "allocs_per_call_dispatch": allocs_per_call,
                "attr_lookup": int(profile.get("attr_lookup", 0) or 0),
                "call_dispatch": call_dispatch,
            }
        )
        if sites:
            for site in sites:
                file = site.get("file")
                line = site.get("line")
                count = site.get("count")
                if not isinstance(file, str) or not isinstance(line, int):
                    continue
                if not isinstance(count, int):
                    continue
                key = (file, line)
                string_sites[key] = string_sites.get(key, 0) + count

    def _top_by(field: str) -> list[dict[str, object]]:
        return sorted(
            benches,
            key=lambda item: item.get(field) or 0,
            reverse=True,
        )[:top_n]

    allocs_per_call = sorted(
        (item for item in benches if item.get("allocs_per_call_dispatch") is not None),
        key=lambda item: item.get("allocs_per_call_dispatch") or 0,
        reverse=True,
    )[:top_n]

    site_items = [
        {"file": key[0], "line": key[1], "count": count}
        for key, count in string_sites.items()
    ]
    site_items.sort(key=lambda item: item["count"], reverse=True)

    return {
        "profiled_benches": len(benches),
        "missing_profile": missing_profile,
        "top_alloc_count": _top_by("alloc_count"),
        "top_string_allocs": _top_by("string_allocs"),
        "top_bytes_allocs": _top_by("bytes_allocs"),
        "top_tuple_allocs": _top_by("tuple_allocs"),
        "top_list_allocs": _top_by("list_allocs"),
        "top_dict_allocs": _top_by("dict_allocs"),
        "top_iter_allocs": _top_by("iter_allocs"),
        "top_allocs_per_call_dispatch": allocs_per_call,
        "top_attr_lookup": _top_by("attr_lookup"),
        "top_string_alloc_sites": site_items[:top_n],
    }


def _run_with_time(
    cmd: list[str],
    *,
    env: dict[str, str],
    out_dir: Path,
    name: str,
    tool: str,
    collect_profile: bool,
) -> ToolRunResult:
    time_bin = _time_binary()
    log_path = out_dir / f"{name}_{tool}.log"
    command = [time_bin, *_time_flags(), *cmd]
    returncode, duration = _run_command(command, env=env, log_path=log_path)
    log_text = log_path.read_text(encoding="utf-8", errors="replace")
    metrics = _parse_time_metrics(log_text)
    metrics = _merge_profile_metrics(metrics, log_path, collect_profile)
    return ToolRunResult(
        tool=tool,
        command=command,
        log_path=str(log_path),
        output_files=[],
        duration_s=duration,
        returncode=returncode,
        metrics=metrics or None,
    )


def _run_with_perf(
    cmd: list[str],
    *,
    env: dict[str, str],
    out_dir: Path,
    name: str,
    collect_profile: bool,
) -> ToolRunResult:
    perf_bin = shutil.which("perf")
    if not perf_bin:
        raise FileNotFoundError("perf not found")
    perf_out = out_dir / f"{name}.perf.data"
    log_path = out_dir / f"{name}_perf.log"
    command = [perf_bin, "record", "-F", "99", "-g", "-o", str(perf_out), "--", *cmd]
    returncode, duration = _run_command(command, env=env, log_path=log_path)
    metrics = _merge_profile_metrics({}, log_path, collect_profile)
    return ToolRunResult(
        tool="perf",
        command=command,
        log_path=str(log_path),
        output_files=[str(perf_out)],
        duration_s=duration,
        returncode=returncode,
        metrics=metrics or None,
    )


def _run_with_perf_stat(
    cmd: list[str],
    *,
    env: dict[str, str],
    out_dir: Path,
    name: str,
    events: str,
    collect_profile: bool,
) -> ToolRunResult:
    perf_bin = shutil.which("perf")
    if not perf_bin:
        raise FileNotFoundError("perf not found")
    log_path = out_dir / f"{name}_perf_stat.log"
    command = [
        perf_bin,
        "stat",
        "-x",
        ",",
        "-e",
        events,
        "--",
        *cmd,
    ]
    returncode, duration = _run_command(command, env=env, log_path=log_path)
    log_text = log_path.read_text(encoding="utf-8", errors="replace")
    metrics = _parse_perf_stat(log_text)
    metrics = _merge_profile_metrics(metrics, log_path, collect_profile)
    return ToolRunResult(
        tool="perf-stat",
        command=command,
        log_path=str(log_path),
        output_files=[],
        duration_s=duration,
        returncode=returncode,
        metrics=metrics or None,
    )


def _run_with_heaptrack(
    cmd: list[str],
    *,
    env: dict[str, str],
    out_dir: Path,
    name: str,
    collect_profile: bool,
) -> ToolRunResult:
    heaptrack_bin = shutil.which("heaptrack")
    if not heaptrack_bin:
        raise FileNotFoundError("heaptrack not found")
    heap_out = out_dir / f"{name}.heaptrack"
    log_path = out_dir / f"{name}_heaptrack.log"
    command = [heaptrack_bin, "-o", str(heap_out), "--", *cmd]
    returncode, duration = _run_command(command, env=env, log_path=log_path)
    metrics = _merge_profile_metrics({}, log_path, collect_profile)
    return ToolRunResult(
        tool="heaptrack",
        command=command,
        log_path=str(log_path),
        output_files=[str(heap_out)],
        duration_s=duration,
        returncode=returncode,
        metrics=metrics or None,
    )


def _run_with_massif(
    cmd: list[str],
    *,
    env: dict[str, str],
    out_dir: Path,
    name: str,
    collect_profile: bool,
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
    metrics = _merge_profile_metrics({}, log_path, collect_profile)
    return ToolRunResult(
        tool="massif",
        command=command,
        log_path=str(log_path),
        output_files=[str(massif_out)],
        duration_s=duration,
        returncode=returncode,
        metrics=metrics or None,
    )


def _build_molt(bench: Path, extra_args: list[str], env: dict[str, str]) -> None:
    binary = ROOT / "hello_molt"
    if binary.exists():
        binary.unlink()
    command = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--output",
        "hello_molt",
        *extra_args,
        str(bench),
    ]
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
    tool: str,
    cmd: list[str],
    *,
    env: dict[str, str],
    out_dir: Path,
    name: str,
    collect_profile: bool,
    perf_stat_events: str,
) -> ToolRunResult | None:
    if tool == "none":
        return None
    if tool == "perf":
        return _run_with_perf(
            cmd, env=env, out_dir=out_dir, name=name, collect_profile=collect_profile
        )
    if tool == "perf-stat":
        return _run_with_perf_stat(
            cmd,
            env=env,
            out_dir=out_dir,
            name=name,
            events=perf_stat_events,
            collect_profile=collect_profile,
        )
    if tool == "heaptrack":
        return _run_with_heaptrack(
            cmd, env=env, out_dir=out_dir, name=name, collect_profile=collect_profile
        )
    if tool == "massif":
        return _run_with_massif(
            cmd, env=env, out_dir=out_dir, name=name, collect_profile=collect_profile
        )
    if tool == "time":
        return _run_with_time(
            cmd,
            env=env,
            out_dir=out_dir,
            name=name,
            tool=tool,
            collect_profile=collect_profile,
        )
    raise ValueError(f"Unknown tool: {tool}")


def _profile_bench(
    bench: Path,
    *,
    cpu_tool: str,
    alloc_tool: str,
    out_dir: Path,
    extra_args: list[str],
    runs: int,
    env_overrides: list[str],
    collect_profile: bool,
    collect_alloc_sites: bool,
    alloc_sites_limit: int | None,
    perf_stat_events: str,
) -> dict[str, object]:
    bench_name = bench.stem
    bench_dir = out_dir / bench_name
    bench_dir.mkdir(parents=True, exist_ok=True)
    extra = extra_args + MOLT_ARGS_BY_BENCH.get(bench.name, [])
    env = _base_env()
    _apply_env_overrides(env, env_overrides)
    if collect_profile or collect_alloc_sites:
        env["MOLT_PROFILE"] = env.get("MOLT_PROFILE", "1") or "1"
    if collect_alloc_sites:
        env["MOLT_PROFILE_ALLOC_SITES"] = (
            env.get("MOLT_PROFILE_ALLOC_SITES", "1") or "1"
        )
        if alloc_sites_limit is not None:
            env["MOLT_PROFILE_ALLOC_SITES_LIMIT"] = str(alloc_sites_limit)
    _build_molt(bench, extra, env)
    cmd = ["./hello_molt"]

    results: list[ToolRunResult] = []
    for idx in range(runs):
        suffix = f"{bench_name}_cpu_{idx + 1}" if runs > 1 else f"{bench_name}_cpu"
        cpu_result = _run_tool(
            cpu_tool,
            cmd,
            env=env,
            out_dir=bench_dir,
            name=suffix,
            collect_profile=collect_profile,
            perf_stat_events=perf_stat_events,
        )
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
                alloc_tool,
                cmd,
                env=env,
                out_dir=bench_dir,
                name=suffix,
                collect_profile=collect_profile,
                perf_stat_events=perf_stat_events,
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
    bench: Path,
    *,
    out_dir: Path,
    extra_args: list[str],
    runs: int,
    env_overrides: list[str],
) -> dict[str, object]:
    bench_name = bench.stem
    bench_dir = out_dir / bench_name
    bench_dir.mkdir(parents=True, exist_ok=True)
    extra = extra_args + MOLT_ARGS_BY_BENCH.get(bench.name, [])
    env = _base_env()
    _apply_env_overrides(env, env_overrides)
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
            "--output",
            "hello_molt",
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
        choices=("auto", "perf", "perf-stat", "time", "none"),
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
        "--perf-stat-events",
        default=DEFAULT_PERF_STAT_EVENTS,
        help="Comma-separated perf stat events (cpu-tool=perf-stat).",
    )
    parser.add_argument(
        "--profile-compiler",
        action="store_true",
        help="Profile compiler time via cProfile during build.",
    )
    parser.add_argument(
        "--molt-profile",
        action="store_true",
        help="Enable runtime counters via MOLT_PROFILE and parse output.",
    )
    parser.add_argument(
        "--molt-profile-alloc-sites",
        action="store_true",
        help="Record string allocation call sites via MOLT_PROFILE_ALLOC_SITES.",
    )
    parser.add_argument(
        "--molt-profile-alloc-sites-limit",
        type=int,
        default=None,
        help="Limit alloc site entries (MOLT_PROFILE_ALLOC_SITES_LIMIT).",
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
        "--env",
        action="append",
        default=[],
        help="Extra env vars for runs (KEY=VALUE).",
    )
    parser.add_argument(
        "--out-dir",
        default=None,
        help="Output directory (default logs/benchmarks/profile_<timestamp>).",
    )
    parser.add_argument(
        "--summary",
        action="store_true",
        help="Write a summary JSON with top alloc counters and call-site hotspots.",
    )
    parser.add_argument(
        "--summary-top",
        type=int,
        default=5,
        help="Top-N entries to include in summary output.",
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

    collect_profile = args.molt_profile or args.molt_profile_alloc_sites
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
        "perf_stat_events": args.perf_stat_events,
        "molt_profile": collect_profile,
        "molt_profile_alloc_sites": args.molt_profile_alloc_sites,
        "molt_profile_alloc_sites_limit": args.molt_profile_alloc_sites_limit,
        "env_overrides": args.env,
        "summary_enabled": args.summary,
        "summary_top_n": args.summary_top,
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
                env_overrides=args.env,
                collect_profile=collect_profile,
                collect_alloc_sites=args.molt_profile_alloc_sites,
                alloc_sites_limit=args.molt_profile_alloc_sites_limit,
                perf_stat_events=args.perf_stat_events,
            )
        )
        if args.profile_compiler:
            metadata["compiler_profiles"].append(
                _profile_compiler(
                    bench,
                    out_dir=out_dir,
                    extra_args=args.molt_arg,
                    runs=args.runs,
                    env_overrides=args.env,
                )
            )

    manifest_path = out_dir / "profile_manifest.json"
    manifest_path.write_text(json.dumps(metadata, indent=2) + "\n")
    if args.summary:
        summary = _collect_profile_summary(metadata, args.summary_top)
        summary_path = out_dir / "profile_summary.json"
        summary_path.write_text(json.dumps(summary, indent=2) + "\n")
    print(f"Profile outputs saved to {out_dir}")
    print(f"Manifest: {manifest_path}")
    if args.summary:
        print(f"Summary: {summary_path}")


if __name__ == "__main__":
    main()
