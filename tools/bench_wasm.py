import argparse
import datetime as dt
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

SUPER_SAMPLES = 10

BENCHMARKS = [
    "tests/benchmarks/bench_fib.py",
    "tests/benchmarks/bench_sum.py",
    "tests/benchmarks/bench_sum_list.py",
    "tests/benchmarks/bench_sum_list_hints.py",
    "tests/benchmarks/bench_min_list.py",
    "tests/benchmarks/bench_max_list.py",
    "tests/benchmarks/bench_prod_list.py",
    "tests/benchmarks/bench_struct.py",
    "tests/benchmarks/bench_attr_access.py",
    "tests/benchmarks/bench_descriptor_property.py",
    "tests/benchmarks/bench_dict_ops.py",
    "tests/benchmarks/bench_dict_views.py",
    "tests/benchmarks/bench_list_ops.py",
    "tests/benchmarks/bench_list_slice.py",
    "tests/benchmarks/bench_tuple_index.py",
    "tests/benchmarks/bench_tuple_slice.py",
    "tests/benchmarks/bench_tuple_pack.py",
    "tests/benchmarks/bench_range_iter.py",
    "tests/benchmarks/bench_try_except.py",
    "tests/benchmarks/bench_generator_iter.py",
    "tests/benchmarks/bench_async_await.py",
    "tests/benchmarks/bench_channel_throughput.py",
    "tests/benchmarks/bench_ptr_registry.py",
    "tests/benchmarks/bench_deeply_nested_loop.py",
    "tests/benchmarks/bench_csv_parse.py",
    "tests/benchmarks/bench_csv_parse_wide.py",
    "tests/benchmarks/bench_matrix_math.py",
    "tests/benchmarks/bench_bytes_find.py",
    "tests/benchmarks/bench_bytes_find_only.py",
    "tests/benchmarks/bench_bytes_replace.py",
    "tests/benchmarks/bench_bytearray_find.py",
    "tests/benchmarks/bench_bytearray_replace.py",
    "tests/benchmarks/bench_str_find.py",
    "tests/benchmarks/bench_str_find_unicode.py",
    "tests/benchmarks/bench_str_find_unicode_warm.py",
    "tests/benchmarks/bench_str_split.py",
    "tests/benchmarks/bench_str_replace.py",
    "tests/benchmarks/bench_str_count.py",
    "tests/benchmarks/bench_str_count_unicode.py",
    "tests/benchmarks/bench_str_count_unicode_warm.py",
    "tests/benchmarks/bench_str_join.py",
    "tests/benchmarks/bench_str_startswith.py",
    "tests/benchmarks/bench_str_endswith.py",
    "tests/benchmarks/bench_memoryview_tobytes.py",
    "tests/benchmarks/bench_parse_msgpack.py",
]

SMOKE_BENCHMARKS = [
    "tests/benchmarks/bench_sum.py",
    "tests/benchmarks/bench_bytes_find.py",
]

MOLT_ARGS_BY_BENCH = {
    "tests/benchmarks/bench_sum_list_hints.py": ["--type-hints", "trust"],
}
RUNTIME_WASM = Path("wasm/molt_runtime.wasm")
RUNTIME_WASM_RELOC = Path("wasm/molt_runtime_reloc.wasm")
LINKED_WASM = Path("output_linked.wasm")
WASM_LD = shutil.which("wasm-ld")
_LINK_WARNED = False
_LINK_DISABLED = False


@dataclass(frozen=True)
class WasmBinary:
    run_env: dict[str, str]
    temp_dir: tempfile.TemporaryDirectory
    build_s: float
    size_kb: float
    linked_used: bool


def _git_rev() -> str | None:
    try:
        res = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if res.returncode != 0:
        return None
    return res.stdout.strip() or None


def _base_env() -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = "src"
    env.setdefault("PYTHONHASHSEED", "0")
    env.setdefault("MOLT_MACOSX_DEPLOYMENT_TARGET", "26.2")
    return env


def _python_executable() -> str:
    exe = Path(sys.executable)
    if exe.exists():
        return sys.executable
    base = getattr(sys, "_base_executable", None)
    if base and Path(base).exists():
        return base
    return sys.executable


def _append_rustflags(env: dict[str, str], flags: str) -> None:
    existing = env.get("RUSTFLAGS", "")
    joined = f"{existing} {flags}".strip()
    env["RUSTFLAGS"] = joined


def build_runtime_wasm(*, reloc: bool, output: Path) -> bool:
    env = os.environ.copy()
    base_flags = "-C link-arg=--import-memory -C link-arg=--import-table -C link-arg=--growable-table"
    if reloc:
        base_flags = (
            f"{base_flags} -C link-arg=--relocatable -C link-arg=--no-gc-sections"
            " -C relocation-model=pic"
        )
    _append_rustflags(env, base_flags)
    res = subprocess.run(
        [
            "cargo",
            "build",
            "--release",
            "--package",
            "molt-runtime",
            "--target",
            "wasm32-wasip1",
        ],
        env=env,
        capture_output=True,
        text=True,
    )
    if res.returncode != 0:
        err = res.stderr.strip() or res.stdout.strip()
        if err:
            print(f"WASM runtime build failed: {err}", file=sys.stderr)
        return False
    src = Path("target/wasm32-wasip1/release/molt_runtime.wasm")
    if not src.exists():
        print("WASM runtime build succeeded but artifact is missing.", file=sys.stderr)
        return False
    output.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, output)
    return True


def _want_linked() -> bool:
    return os.environ.get("MOLT_WASM_LINK") == "1"


def _link_wasm(env: dict[str, str], input_path: Path) -> Path | None:
    if not _want_linked():
        return None
    if WASM_LD is None:
        global _LINK_WARNED
        if not _LINK_WARNED:
            print(
                "Skipping wasm link: wasm-ld not found (install LLVM to enable).",
                file=sys.stderr,
            )
            _LINK_WARNED = True
        return None
    global _LINK_DISABLED
    if _LINK_DISABLED:
        return None
    if LINKED_WASM.exists():
        LINKED_WASM.unlink()
    runtime_path = RUNTIME_WASM_RELOC if RUNTIME_WASM_RELOC.exists() else RUNTIME_WASM
    res = subprocess.run(
        [
            sys.executable,
            "tools/wasm_link.py",
            "--runtime",
            str(runtime_path),
            "--input",
            str(input_path),
            "--output",
            str(LINKED_WASM),
        ],
        env=env,
        capture_output=True,
        text=True,
    )
    if res.returncode != 0:
        err = res.stderr.strip() or res.stdout.strip()
        if err:
            print(f"WASM link failed: {err}", file=sys.stderr)
            if (
                "not a relocatable wasm file" in err
                or "out of order section" in err
                or "invalid function symbol index" in err
                or "Stack dump" in err
            ):
                print(
                    "Disabling wasm linking for remaining benches (non-relocatable input).",
                    file=sys.stderr,
                )
                _LINK_DISABLED = True
        return None
    if not LINKED_WASM.exists():
        print("WASM link produced no output artifact.", file=sys.stderr)
        return None
    return LINKED_WASM


def prepare_wasm_binary(script: str) -> WasmBinary | None:
    temp_dir = tempfile.TemporaryDirectory(prefix="molt-wasm-bench-")
    output_path = Path(temp_dir.name) / "output.wasm"
    env = _base_env()
    env["MOLT_WASM_PATH"] = str(output_path)
    extra_args = MOLT_ARGS_BY_BENCH.get(script, [])
    python_exe = _python_executable()
    start = time.perf_counter()
    build_res = subprocess.run(
        [
            python_exe,
            "-m",
            "molt.cli",
            "build",
            "--no-cache",
            "--target",
            "wasm",
            "--out-dir",
            str(output_path.parent),
            *extra_args,
            script,
        ],
        env=env,
        capture_output=True,
        text=True,
    )
    build_s = time.perf_counter() - start
    if build_res.returncode != 0:
        err = build_res.stderr.strip() or build_res.stdout.strip()
        if err:
            print(f"WASM build failed for {script}: {err}", file=sys.stderr)
        temp_dir.cleanup()
        return None

    if not output_path.exists():
        print(f"WASM build produced no output.wasm for {script}", file=sys.stderr)
        temp_dir.cleanup()
        return None

    linked = _link_wasm(env, output_path)
    linked_used = linked is not None
    wasm_path = linked if linked_used else output_path
    wasm_size = wasm_path.stat().st_size / 1024
    run_env = env.copy()
    if linked is not None:
        run_env["MOLT_WASM_LINKED"] = "1"
        run_env["MOLT_WASM_LINKED_PATH"] = str(linked)
    return WasmBinary(run_env, temp_dir, build_s, wasm_size, linked_used)


def measure_wasm_run(run_env: dict[str, str]) -> float | None:
    start = time.perf_counter()
    run_res = subprocess.run(
        ["node", "run_wasm.js"],
        env=run_env,
        capture_output=True,
        text=True,
    )
    end = time.perf_counter()
    if run_res.returncode != 0:
        err = run_res.stderr.strip() or run_res.stdout.strip()
        if err:
            print(f"WASM run failed: {err}", file=sys.stderr)
        return None
    return end - start


def collect_samples(
    wasm: WasmBinary, samples: int, warmup: int
) -> tuple[list[float], bool]:
    for _ in range(warmup):
        if measure_wasm_run(wasm.run_env) is None:
            return [], False
    runs = [measure_wasm_run(wasm.run_env) for _ in range(samples)]
    valid = [t for t in runs if t is not None]
    return valid, bool(valid)


def summarize_samples(samples: list[float]) -> dict[str, float]:
    mean = statistics.mean(samples)
    median = statistics.median(samples)
    variance = statistics.pvariance(samples) if len(samples) > 1 else 0.0
    min_s = min(samples)
    max_s = max(samples)
    return {
        "mean_s": mean,
        "median_s": median,
        "variance_s": variance,
        "range_s": max_s - min_s,
        "min_s": min_s,
        "max_s": max_s,
    }


def bench_results(
    benchmarks: list[str], samples: int, warmup: int, super_run: bool
) -> dict[str, dict]:
    data: dict[str, dict] = {}
    print(f"{'Benchmark':<30} | {'WASM (s)':<12} | {'WASM size':<10}")
    print("-" * 60)
    for script in benchmarks:
        name = Path(script).stem
        wasm_time = 0.0
        wasm_size = 0.0
        wasm_build = 0.0
        linked_used = False
        ok = False
        wasm_samples: list[float] = []
        wasm_binary = prepare_wasm_binary(script)
        if wasm_binary is not None:
            try:
                wasm_samples, ok = collect_samples(wasm_binary, samples, warmup)
                wasm_time = statistics.mean(wasm_samples) if ok else 0.0
                wasm_size = wasm_binary.size_kb
                wasm_build = wasm_binary.build_s
                linked_used = wasm_binary.linked_used
            finally:
                wasm_binary.temp_dir.cleanup()
        time_cell = f"{wasm_time:<12.4f}" if ok else f"{'n/a':<12}"
        print(f"{name:<30} | {time_cell} | {wasm_size:>8.1f} KB")
        data[name] = {
            "molt_wasm_time_s": wasm_time,
            "molt_wasm_build_s": wasm_build,
            "molt_wasm_size_kb": wasm_size,
            "molt_wasm_ok": ok,
            "molt_wasm_linked": linked_used,
        }
        if super_run and ok:
            data[name]["molt_wasm_stats"] = summarize_samples(wasm_samples)
    return data


def write_json(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser(description="Run Molt WASM benchmark suite.")
    parser.add_argument("--json-out", type=Path, default=None)
    parser.add_argument("--samples", type=int, default=None)
    parser.add_argument(
        "--warmup",
        type=int,
        default=None,
        help="Warmup runs per benchmark before sampling (default: 1, or 0 for --smoke).",
    )
    parser.add_argument("--smoke", action="store_true")
    parser.add_argument(
        "--linked",
        action="store_true",
        help="Attempt single-module wasm linking with wasm-ld when available.",
    )
    parser.add_argument(
        "--super",
        action="store_true",
        help="Run all benchmarks 10x and emit mean/median/variance/range stats.",
    )
    args = parser.parse_args()

    if args.linked:
        os.environ["MOLT_WASM_LINK"] = "1"
    if args.super and args.smoke:
        parser.error("--super cannot be combined with --smoke")
    if args.super and args.samples is not None:
        parser.error("--super cannot be combined with --samples")

    if not build_runtime_wasm(reloc=False, output=RUNTIME_WASM):
        sys.exit(1)
    if _want_linked() and not build_runtime_wasm(reloc=True, output=RUNTIME_WASM_RELOC):
        print(
            "Relocatable runtime build failed; falling back to non-linked wasm runs.",
            file=sys.stderr,
        )

    benchmarks = SMOKE_BENCHMARKS if args.smoke else BENCHMARKS
    samples = (
        SUPER_SAMPLES
        if args.super
        else (args.samples if args.samples is not None else (1 if args.smoke else 3))
    )
    warmup = args.warmup if args.warmup is not None else (0 if args.smoke else 1)
    results = bench_results(benchmarks, samples, warmup, args.super)

    load_avg = None
    try:
        load_avg = os.getloadavg()
    except OSError:
        load_avg = None

    payload = {
        "schema_version": 1,
        "created_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "git_rev": _git_rev(),
        "super_run": args.super,
        "samples": samples,
        "warmup": warmup,
        "system": {
            "platform": platform.platform(),
            "python": platform.python_version(),
            "machine": platform.machine(),
            "cpu_count": os.cpu_count(),
            "load_avg": load_avg,
        },
        "benchmarks": results,
    }

    json_out = args.json_out
    if json_out is None:
        timestamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%d_%H%M%S")
        json_out = Path("bench/results") / f"bench_wasm_{timestamp}.json"
    write_json(json_out, payload)


if __name__ == "__main__":
    main()
