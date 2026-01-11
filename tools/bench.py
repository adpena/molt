import argparse
import datetime as dt
import importlib.util
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import textwrap
import time
from dataclasses import dataclass
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


@dataclass(frozen=True)
class BenchRunner:
    cmd: list[str]
    script: str | None
    env: dict[str, str]


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


def _prepend_pythonpath(env: dict[str, str], path: str) -> dict[str, str]:
    current = env.get("PYTHONPATH", "")
    env["PYTHONPATH"] = f"{path}{os.pathsep}{current}" if current else path
    return env


def _base_python_env() -> dict[str, str]:
    env = os.environ.copy()
    return _prepend_pythonpath(env, "src")


def measure_runtime(cmd_args, script=None, env=None):
    start = time.perf_counter()
    full_cmd = cmd_args + ([script] if script else [])
    res = subprocess.run(full_cmd, capture_output=True, text=True, env=env)
    end = time.perf_counter()
    if res.returncode != 0:
        return None
    return end - start


def measure_molt(script, extra_args=None):
    if os.path.exists("./hello_molt"):
        os.remove("./hello_molt")

    env = os.environ.copy()
    env["PYTHONPATH"] = "src"
    args = [sys.executable, "-m", "molt.cli", "build"]
    if extra_args:
        args.extend(extra_args)
    args.append(script)
    res = subprocess.run(
        args,
        env=env,
        capture_output=True,
        text=True,
    )

    if res.returncode != 0:
        return None, 0

    binary_size = os.path.getsize("./hello_molt") / 1024

    start = time.perf_counter()
    res = subprocess.run(["./hello_molt"], capture_output=True, text=True)
    end = time.perf_counter()

    if res.returncode != 0:
        return None, binary_size

    return end - start, binary_size


def collect_samples(measure_fn, samples):
    times = [measure_fn() for _ in range(samples)]
    valid_times = [t for t in times if t is not None]
    if not valid_times:
        return 0.0, False
    return statistics.mean(valid_times), True


def _split_imports(source: str) -> tuple[list[str], list[str]]:
    imports: list[str] = []
    body: list[str] = []
    seen_body = False
    for line in source.splitlines():
        stripped = line.strip()
        if not seen_body:
            if stripped == "" or stripped.startswith("#"):
                imports.append(line)
                continue
            if stripped.startswith("import ") or stripped.startswith("from "):
                imports.append(line)
                continue
        seen_body = True
        body.append(line)
    return imports, body


def _rewrite_prints(body_lines: list[str]) -> list[str]:
    rewritten: list[str] = []
    has_print = False
    for line in body_lines:
        stripped = line.lstrip()
        if stripped.startswith("print(") and stripped.endswith(")"):
            indent = line[: len(line) - len(stripped)]
            expr = stripped[len("print(") : -1]
            rewritten.append(f"{indent}_molt_result = {expr}")
            has_print = True
        else:
            rewritten.append(line)
    if has_print:
        rewritten.append("return _molt_result")
    else:
        rewritten.append("return None")
    return rewritten


def _module_available(name: str) -> bool:
    return importlib.util.find_spec(name) is not None


def _prepare_cython_runner(
    script_path: Path, build_root: Path, base_env: dict[str, str]
) -> BenchRunner | None:
    if not _module_available("pyximport"):
        return None
    source = script_path.read_text()
    imports, body = _split_imports(source)
    body = _rewrite_prints(body)
    module_name = f"bench_cython_{script_path.stem}"
    module_dir = build_root / module_name
    module_dir.mkdir(parents=True, exist_ok=True)
    pyx_path = module_dir / f"{module_name}.pyx"
    build_dir = module_dir / "build"
    pyx_source = "# cython: language_level=3\n"
    if imports:
        pyx_source += "\n".join(imports) + "\n"
    pyx_source += "\n\ndef bench():\n"
    pyx_source += textwrap.indent("\n".join(body), "    ") + "\n"
    pyx_path.write_text(pyx_source)

    runner_path = module_dir / "runner.py"
    runner_source = f"""import importlib
import pyximport

pyximport.install(language_level=3, build_dir={str(build_dir)!r}, inplace=False)
mod = importlib.import_module("{module_name}")
mod.bench()
"""
    runner_path.write_text(runner_source)
    env = _prepend_pythonpath(base_env.copy(), str(module_dir))
    env["PYTHONWARNINGS"] = "ignore"
    warm = subprocess.run(
        [sys.executable, str(runner_path)], capture_output=True, text=True, env=env
    )
    if warm.returncode != 0:
        return None
    return BenchRunner([sys.executable], str(runner_path), env)


def _prepare_numba_runner(
    script_path: Path, build_root: Path, base_env: dict[str, str]
) -> BenchRunner | None:
    if not _module_available("numba"):
        return None
    source = script_path.read_text()
    imports, body = _split_imports(source)
    body = _rewrite_prints(body)
    module_name = f"bench_numba_{script_path.stem}"
    module_dir = build_root / module_name
    module_dir.mkdir(parents=True, exist_ok=True)
    runner_path = module_dir / f"{module_name}.py"
    module_source = ""
    if imports:
        module_source += "\n".join(imports) + "\n"
    module_source += "from numba import njit\n\n"
    module_source += "def _bench_py():\n"
    module_source += textwrap.indent("\n".join(body), "    ") + "\n"
    module_source += "bench = njit(cache=True)(_bench_py)\n\n"
    module_source += "if __name__ == '__main__':\n    bench()\n"
    runner_path.write_text(module_source)
    env = _prepend_pythonpath(base_env.copy(), str(module_dir))
    env["NUMBA_CACHE_DIR"] = str(module_dir / "cache")
    env["NUMBA_DISABLE_PERFORMANCE_WARNINGS"] = "1"
    warm = subprocess.run(
        [sys.executable, str(runner_path)], capture_output=True, text=True, env=env
    )
    if warm.returncode != 0:
        return None
    return BenchRunner([sys.executable], str(runner_path), env)


def _prepare_codon_runner(
    script_path: Path, build_root: Path, base_env: dict[str, str]
) -> BenchRunner | None:
    codon = shutil.which("codon")
    if not codon:
        return None
    arch_prefix: list[str] = []
    if platform.system() == "Darwin" and platform.machine() == "x86_64":
        arch_prefix = ["/usr/bin/arch", "-arm64"]
    module_name = f"bench_codon_{script_path.stem}"
    module_dir = build_root / module_name
    module_dir.mkdir(parents=True, exist_ok=True)
    binary_path = module_dir / module_name
    env = base_env.copy()
    codon_home: str | None = None
    if "CODON_HOME" not in env:
        codon_path = Path(codon).resolve()
        candidate = codon_path.parent.parent
        if (candidate / "lib" / "codon").exists():
            codon_home = str(candidate)
            env["CODON_HOME"] = codon_home
    else:
        codon_home = env.get("CODON_HOME")
    build = subprocess.run(
        arch_prefix
        + [codon, "build", "-release", str(script_path), "-o", str(binary_path)],
        capture_output=True,
        text=True,
        env=env,
    )
    if build.returncode != 0:
        return None
    if codon_home:
        libomp = Path(codon_home) / "lib" / "codon" / "libomp.dylib"
        target = module_dir / "libomp.dylib"
        if libomp.exists() and not target.exists():
            shutil.copy2(libomp, target)
    return BenchRunner(arch_prefix + [str(binary_path)], None, env)


def _pypy_command() -> list[str] | None:
    if not shutil.which("uv"):
        print("Skipping PyPy: uv not found.", file=sys.stderr)
        return None
    probe = subprocess.run(
        [
            "uv",
            "run",
            "--no-project",
            "--python",
            "pypy@3.11",
            "python",
            "-c",
            "print('ok')",
        ],
        capture_output=True,
        text=True,
    )
    if probe.returncode != 0:
        msg = (probe.stderr or probe.stdout).strip().splitlines()
        hint = msg[-1] if msg else "PyPy unavailable for this Python requirement"
        print(f"Skipping PyPy: {hint}", file=sys.stderr)
        return None
    return ["uv", "run", "--no-project", "--python", "pypy@3.11", "python"]


def bench_results(benchmarks, samples, use_pypy, use_cython, use_numba, use_codon):
    runtimes = {"CPython": [sys.executable]}
    if use_pypy:
        pypy_cmd = _pypy_command()
        if pypy_cmd:
            runtimes["PyPy"] = pypy_cmd

    if use_cython and not _module_available("pyximport"):
        print("Skipping Cython: pyximport not available.", file=sys.stderr)
        use_cython = False
    if use_numba and not _module_available("numba"):
        print("Skipping Numba: numba not available.", file=sys.stderr)
        use_numba = False
    if use_codon and not shutil.which("codon"):
        print("Skipping Codon: codon not found.", file=sys.stderr)
        use_codon = False

    header = (
        f"{'Benchmark':<30} | {'CPython (s)':<12} | {'PyPy (s)':<12} | "
        f"{'Cython (s)':<12} | {'Numba (s)':<12} | {'Codon (s)':<12} | "
        f"{'Molt/Codon':<12} | {'Molt (s)':<10} | "
        f"{'Molt Speedup':<12} | {'Molt Size'}"
    )
    print(header)
    print("-" * len(header))

    base_env = _base_python_env()
    cython_root = Path("bench/cython")
    numba_root = Path("bench/numba")
    codon_root = Path("bench/codon")

    data = {}
    for script in benchmarks:
        name = os.path.basename(script)
        results = {}
        runtime_ok = {}
        for rt_name, cmd in runtimes.items():
            result, ok = collect_samples(
                lambda: measure_runtime(cmd, script, env=base_env), samples
            )
            results[rt_name] = result
            runtime_ok[rt_name] = ok

        cython_time = 0.0
        cython_ok = False
        if use_cython:
            runner = _prepare_cython_runner(Path(script), cython_root, base_env)
            if runner is not None:
                cython_time, cython_ok = collect_samples(
                    lambda: measure_runtime(runner.cmd, runner.script, env=runner.env),
                    samples,
                )
            else:
                print(f"Skipping Cython for {name}.", file=sys.stderr)

        numba_time = 0.0
        numba_ok = False
        if use_numba:
            runner = _prepare_numba_runner(Path(script), numba_root, base_env)
            if runner is not None:
                numba_time, numba_ok = collect_samples(
                    lambda: measure_runtime(runner.cmd, runner.script, env=runner.env),
                    samples,
                )
            else:
                print(f"Skipping Numba for {name}.", file=sys.stderr)

        codon_time = 0.0
        codon_ok = False
        if use_codon:
            runner = _prepare_codon_runner(Path(script), codon_root, base_env)
            if runner is not None:
                codon_time, codon_ok = collect_samples(
                    lambda: measure_runtime(runner.cmd, runner.script, env=runner.env),
                    samples,
                )
            else:
                print(f"Skipping Codon for {name}.", file=sys.stderr)

        molt_time, molt_size = 0.0, 0.0
        molt_args = MOLT_ARGS_BY_BENCH.get(script, [])
        molt_runs = [measure_molt(script, molt_args) for _ in range(samples)]
        valid_molt = [r[0] for r in molt_runs if r[0] is not None]
        molt_ok = bool(valid_molt)
        if valid_molt:
            molt_time = statistics.mean(valid_molt)
            molt_size = molt_runs[0][1]
        else:
            print(f"Molt build/run failed for {name}.", file=sys.stderr)

        speedup = results.get("CPython", 0.0) / molt_time if molt_time > 0 else 0.0
        ratio = (
            molt_time / results["CPython"]
            if molt_ok and results.get("CPython", 0.0) > 0
            else None
        )
        codon_ratio = (
            (molt_time / codon_time)
            if molt_ok and codon_ok and codon_time > 0
            else None
        )

        cpython_cell = (
            f"{results.get('CPython', 0.0):<12.4f}"
            if runtime_ok.get("CPython", False)
            else f"{'n/a':<12}"
        )
        pypy_cell = (
            f"{results.get('PyPy', 0.0):<12.4f}"
            if runtime_ok.get("PyPy", False)
            else f"{'n/a':<12}"
        )
        cython_cell = f"{cython_time:<12.4f}" if cython_ok else f"{'n/a':<12}"
        numba_cell = f"{numba_time:<12.4f}" if numba_ok else f"{'n/a':<12}"
        codon_cell = f"{codon_time:<12.4f}" if codon_ok else f"{'n/a':<12}"
        codon_ratio_cell = (
            f"{codon_ratio:<12.2f}x" if codon_ratio is not None else f"{'n/a':<12}"
        )

        print(
            f"{name:<30} | {cpython_cell} | {pypy_cell} | {cython_cell} | "
            f"{numba_cell} | {codon_cell} | {codon_ratio_cell} | "
            f"{molt_time:<10.4f} | {speedup:<12.2f}x | "
            f"{molt_size:.1f} KB"
        )

        data[name] = {
            "cpython_time_s": results.get("CPython", 0.0),
            "pypy_time_s": results.get("PyPy", 0.0),
            "cython_time_s": cython_time,
            "numba_time_s": numba_time,
            "codon_time_s": codon_time,
            "molt_time_s": molt_time,
            "molt_size_kb": molt_size,
            "molt_speedup": speedup,
            "molt_cpython_ratio": ratio,
            "molt_codon_ratio": codon_ratio,
            "molt_ok": molt_ok,
            "molt_args": molt_args,
            "cython_ok": cython_ok,
            "numba_ok": numba_ok,
            "codon_ok": codon_ok,
        }

    return data


def write_json(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")


def load_json(path: Path) -> dict:
    return json.loads(path.read_text())


def compare_baseline(current: dict, baseline: dict, max_regression: float) -> list[str]:
    regressions = []
    baseline_bench = baseline.get("benchmarks", {})
    for name, stats in current.get("benchmarks", {}).items():
        current_ratio = stats.get("molt_cpython_ratio")
        base_ratio = baseline_bench.get(name, {}).get("molt_cpython_ratio")
        if current_ratio is None or base_ratio is None:
            continue
        if current_ratio > base_ratio * (1 + max_regression):
            regressions.append(
                f"{name}: ratio {current_ratio:.4f} > {base_ratio:.4f} * {1 + max_regression:.2f}"
            )
    return regressions


def main():
    parser = argparse.ArgumentParser(description="Run Molt benchmark suite.")
    parser.add_argument("--json-out", type=Path, default=None)
    parser.add_argument("--baseline", type=Path, default=None)
    parser.add_argument("--update-baseline", action="store_true")
    parser.add_argument("--max-regression", type=float, default=0.15)
    parser.add_argument("--samples", type=int, default=None)
    parser.add_argument("--smoke", action="store_true")
    parser.add_argument("--no-pypy", action="store_true")
    parser.add_argument("--no-cython", action="store_true")
    parser.add_argument("--no-numba", action="store_true")
    parser.add_argument("--no-codon", action="store_true")
    args = parser.parse_args()

    benchmarks = SMOKE_BENCHMARKS if args.smoke else BENCHMARKS
    samples = args.samples if args.samples is not None else (1 if args.smoke else 3)
    use_pypy = not args.no_pypy
    use_cython = not args.no_cython
    use_numba = not args.no_numba
    use_codon = not args.no_codon

    results = bench_results(
        benchmarks, samples, use_pypy, use_cython, use_numba, use_codon
    )

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
        json_out = Path("bench/results") / f"bench_{timestamp}.json"
    write_json(json_out, payload)

    baseline_path = args.baseline
    if args.update_baseline:
        if baseline_path is None:
            baseline_path = Path("bench/baseline.json")
        write_json(baseline_path, payload)
        print(f"Baseline updated: {baseline_path}")
        return

    if baseline_path is None or not baseline_path.exists():
        return

    baseline = load_json(baseline_path)
    regressions = compare_baseline(payload, baseline, args.max_regression)
    if regressions:
        print("Performance regression detected:")
        for line in regressions:
            print(f"  - {line}")
        raise SystemExit(1)


if __name__ == "__main__":
    main()
