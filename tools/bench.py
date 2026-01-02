import time
import subprocess
import os
import sys
import statistics

BENCHMARKS = [
    "tests/benchmarks/bench_fib.py",
    "tests/benchmarks/bench_sum.py",
    "tests/benchmarks/bench_struct.py",
    "tests/benchmarks/bench_deeply_nested_loop.py",
    "tests/benchmarks/bench_matrix_math.py",
]


def measure_runtime(cmd_args, script):
    start = time.perf_counter()
    res = subprocess.run(cmd_args + [script], capture_output=True, text=True)
    end = time.perf_counter()
    if res.returncode != 0:
        return None
    return end - start


def measure_molt(script):
    # Clean up stale binary
    if os.path.exists("./hello_molt"):
        os.remove("./hello_molt")

    # Build
    env = os.environ.copy()
    env["PYTHONPATH"] = "src"
    res = subprocess.run(
        ["python3", "-m", "molt.cli", "build", script],
        env=env,
        capture_output=True,
        text=True,
    )

    if res.returncode != 0:
        return None, 0

    binary_size = os.path.getsize("./hello_molt") / 1024  # KB

    # Run
    start = time.perf_counter()
    res = subprocess.run(["./hello_molt"], capture_output=True, text=True)
    end = time.perf_counter()

    if res.returncode != 0:
        return None, binary_size

    return end - start, binary_size


def run_bench():
    runtimes = {
        "CPython": [sys.executable],
        "PyPy": ["uv", "run", "--python", "pypy@3.11", "python"],
    }

    header = f"{'Benchmark':<30} | {'CPython (s)':<10} | {'PyPy (s)':<10} | {'Molt (s)':<10} | {'Molt Speedup':<12} | {'Molt Size'}"
    print(header)
    print("-" * len(header))

    for script in BENCHMARKS:
        name = os.path.basename(script)

        results = {}
        for rt_name, cmd in runtimes.items():
            times = [measure_runtime(cmd, script) for _ in range(3)]
            valid_times = [t for t in times if t is not None]
            results[rt_name] = statistics.mean(valid_times) if valid_times else 0.0

        molt_time, molt_size = 0.0, 0
        molt_res = [measure_molt(script) for _ in range(3)]
        valid_molt = [r[0] for r in molt_res if r[0] is not None]
        if valid_molt:
            molt_time = statistics.mean(valid_molt)
            molt_size = molt_res[0][1]

        speedup = results["CPython"] / molt_time if molt_time > 0 else 0.0

        print(
            f"{name:<30} | {results['CPython']:<10.4f} | {results['PyPy']:<10.4f} | {molt_time:<10.4f} | {speedup:<12.2f}x | {molt_size:.1f} KB"
        )


if __name__ == "__main__":
    run_bench()
