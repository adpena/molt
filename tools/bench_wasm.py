import argparse
import datetime as dt
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

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
    "tests/benchmarks/bench_deeply_nested_loop.py",
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
    env.setdefault("MOLT_MACOSX_DEPLOYMENT_TARGET", "26.2")
    return env


def _append_rustflags(env: dict[str, str], flags: str) -> None:
    existing = env.get("RUSTFLAGS", "")
    joined = f"{existing} {flags}".strip()
    env["RUSTFLAGS"] = joined


def build_runtime_wasm(*, reloc: bool, output: Path) -> bool:
    env = os.environ.copy()
    base_flags = "-C link-arg=--import-memory"
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


def _link_wasm(env: dict[str, str]) -> Path | None:
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
            "output.wasm",
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


def measure_wasm(script: str) -> tuple[float | None, float, bool]:
    if os.path.exists("./output.wasm"):
        os.remove("./output.wasm")

    env = _base_env()
    extra_args = MOLT_ARGS_BY_BENCH.get(script, [])
    build_res = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--target",
            "wasm",
            *extra_args,
            script,
        ],
        env=env,
        capture_output=True,
        text=True,
    )
    if build_res.returncode != 0:
        err = build_res.stderr.strip() or build_res.stdout.strip()
        if err:
            print(f"WASM build failed for {script}: {err}", file=sys.stderr)
        return None, 0.0, False

    output_path = Path("output.wasm")
    if not output_path.exists():
        print(f"WASM build produced no output.wasm for {script}", file=sys.stderr)
        return None, 0.0, False

    linked = _link_wasm(env)
    linked_used = linked is not None
    wasm_path = linked if linked_used else output_path
    wasm_size = wasm_path.stat().st_size / 1024
    run_env = env.copy()
    if linked is not None:
        run_env["MOLT_WASM_LINKED"] = "1"
        run_env["MOLT_WASM_LINKED_PATH"] = str(linked)
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
            print(f"WASM run failed for {script}: {err}", file=sys.stderr)
        return None, wasm_size, linked_used
    return end - start, wasm_size, linked_used


def collect_samples(script: str, samples: int) -> tuple[float, float, bool, bool]:
    runs = [measure_wasm(script) for _ in range(samples)]
    valid = [t for t, _, _ in runs if t is not None]
    if not valid:
        return 0.0, runs[0][1] if runs else 0.0, False, False
    avg = statistics.mean(valid)
    size = runs[0][1]
    linked_used = any(r[2] for r in runs)
    return avg, size, True, linked_used


def bench_results(benchmarks: list[str], samples: int) -> dict[str, dict]:
    data: dict[str, dict] = {}
    print(f"{'Benchmark':<30} | {'WASM (s)':<12} | {'WASM size':<10}")
    print("-" * 60)
    for script in benchmarks:
        name = Path(script).stem
        wasm_time, wasm_size, ok, linked_used = collect_samples(script, samples)
        time_cell = f"{wasm_time:<12.4f}" if ok else f"{'n/a':<12}"
        print(f"{name:<30} | {time_cell} | {wasm_size:>8.1f} KB")
        data[name] = {
            "molt_wasm_time_s": wasm_time,
            "molt_wasm_size_kb": wasm_size,
            "molt_wasm_ok": ok,
            "molt_wasm_linked": linked_used,
        }
    return data


def write_json(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser(description="Run Molt WASM benchmark suite.")
    parser.add_argument("--json-out", type=Path, default=None)
    parser.add_argument("--samples", type=int, default=None)
    parser.add_argument("--smoke", action="store_true")
    parser.add_argument(
        "--linked",
        action="store_true",
        help="Attempt single-module wasm linking with wasm-ld when available.",
    )
    args = parser.parse_args()

    if args.linked:
        os.environ["MOLT_WASM_LINK"] = "1"

    if not build_runtime_wasm(reloc=False, output=RUNTIME_WASM):
        sys.exit(1)
    if _want_linked() and not build_runtime_wasm(reloc=True, output=RUNTIME_WASM_RELOC):
        print(
            "Relocatable runtime build failed; falling back to non-linked wasm runs.",
            file=sys.stderr,
        )

    benchmarks = SMOKE_BENCHMARKS if args.smoke else BENCHMARKS
    samples = args.samples if args.samples is not None else (1 if args.smoke else 3)
    results = bench_results(benchmarks, samples)

    payload = {
        "schema_version": 1,
        "created_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "git_rev": _git_rev(),
        "system": {
            "platform": platform.platform(),
            "python": platform.python_version(),
            "machine": platform.machine(),
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
