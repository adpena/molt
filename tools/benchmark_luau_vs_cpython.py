#!/usr/bin/env python3
"""Benchmark: Molt-Luau (via Lune) vs CPython on procedural zone generation.

Usage:
    # Full benchmark (requires Molt build + Lune):
    uv run python tools/benchmark_luau_vs_cpython.py

    # CPython-only (no Molt/Lune needed):
    uv run python tools/benchmark_luau_vs_cpython.py --cpython-only

Environment:
    MOLT_EXT_ROOT=/Volumes/APDataStore/Molt
    CARGO_TARGET_DIR=/Volumes/APDataStore/Molt/cargo-target
    RUSTC_WRAPPER=""
    PYTHONPATH=src
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time

GENERATOR_SOURCE = '''\
def hash_float(x, y, z, seed):
    h = seed + x * 374761 + y * 668265 + z * 224682
    h = (h * 3266489) % 2147483647
    return h / 2147483647.0

def lerp(a, b, t):
    return a + (b - a) * t

def generate_platforms(seed, base_y, depth, step):
    platforms = []
    crystals = []
    anchors = []
    y = base_y
    while y < base_y + depth:
        x = -48
        while x <= 48:
            z = -48
            while z <= 48:
                density = hash_float(x, y, z, seed)
                if density > 0.55:
                    platforms.append([x, y, z, step])
                    crystal_roll = hash_float(x, y, z, seed + 1)
                    if crystal_roll > 0.94:
                        glow = 0.6 + hash_float(x, y, z, seed + 2) * 0.4
                        crystals.append([x, y + step, z, glow])
                    if crystal_roll < 0.03:
                        anchors.append([x, y + step + 2, z])
                z = z + step
            x = x + step
        y = y + step
    return [platforms, crystals, anchors]

result = generate_platforms(1337, -120, 64, 8)
print(len(result[0]))
print(len(result[1]))
print(len(result[2]))
'''

ITERATIONS = 10


def run_cpython_bench(source_path: str, iterations: int) -> dict:
    """Benchmark CPython execution."""
    times = []
    output = None
    for i in range(iterations):
        t0 = time.perf_counter()
        proc = subprocess.run(
            [sys.executable, source_path],
            capture_output=True, text=True, timeout=30,
        )
        elapsed = time.perf_counter() - t0
        if proc.returncode != 0:
            print(f"  CPython error: {proc.stderr.strip()}", file=sys.stderr)
            return {"error": proc.stderr.strip()}
        times.append(elapsed)
        if output is None:
            output = proc.stdout.strip()
    return {
        "runtime": "CPython",
        "version": sys.version.split()[0],
        "iterations": iterations,
        "times_ms": [round(t * 1000, 2) for t in times],
        "mean_ms": round(sum(times) / len(times) * 1000, 2),
        "min_ms": round(min(times) * 1000, 2),
        "max_ms": round(max(times) * 1000, 2),
        "output": output,
    }


def compile_to_luau(source_path: str, output_path: str) -> bool:
    """Compile Python source to Luau via Molt."""
    env = {
        **os.environ,
        "MOLT_EXT_ROOT": os.environ.get("MOLT_EXT_ROOT", "/Volumes/APDataStore/Molt"),
        "CARGO_TARGET_DIR": os.environ.get("CARGO_TARGET_DIR", "/Volumes/APDataStore/Molt/cargo-target"),
        "RUSTC_WRAPPER": "",
        "PYTHONPATH": "src",
    }
    proc = subprocess.run(
        ["uv", "run", "python", "-m", "molt.cli", "build",
         source_path, "--target", "luau", "--output", output_path],
        capture_output=True, text=True, timeout=120, env=env,
        cwd=os.path.expanduser("~/PycharmProjects/molt"),
    )
    if proc.returncode != 0:
        print(f"  Molt compile error: {proc.stderr.strip()}", file=sys.stderr)
        return False
    return True


def run_lune_bench(luau_path: str, iterations: int) -> dict:
    """Benchmark Lune (Luau VM) execution."""
    lune = os.path.expanduser("~/.aftman/bin/lune")
    if not os.path.exists(lune):
        lune = "lune"

    times = []
    output = None
    for i in range(iterations):
        t0 = time.perf_counter()
        proc = subprocess.run(
            [lune, "run", luau_path],
            capture_output=True, text=True, timeout=30,
        )
        elapsed = time.perf_counter() - t0
        if proc.returncode != 0:
            print(f"  Lune error: {proc.stderr.strip()}", file=sys.stderr)
            return {"error": proc.stderr.strip()}
        times.append(elapsed)
        if output is None:
            output = proc.stdout.strip()
    return {
        "runtime": "Lune (Luau VM)",
        "iterations": iterations,
        "times_ms": [round(t * 1000, 2) for t in times],
        "mean_ms": round(sum(times) / len(times) * 1000, 2),
        "min_ms": round(min(times) * 1000, 2),
        "max_ms": round(max(times) * 1000, 2),
        "output": output,
    }


def main():
    parser = argparse.ArgumentParser(description="Benchmark Molt-Luau vs CPython")
    parser.add_argument("--cpython-only", action="store_true")
    parser.add_argument("--iterations", type=int, default=ITERATIONS)
    args = parser.parse_args()

    with tempfile.NamedTemporaryFile(suffix=".py", mode="w", delete=False) as f:
        f.write(GENERATOR_SOURCE)
        py_path = f.name

    try:
        print(f"=== Molt Benchmark: Procedural Zone Generator ===")
        print(f"Iterations: {args.iterations}")
        print()

        # CPython
        print("Running CPython benchmark...")
        cpython_result = run_cpython_bench(py_path, args.iterations)
        if "error" not in cpython_result:
            print(f"  Mean: {cpython_result['mean_ms']:.2f} ms")
            print(f"  Min:  {cpython_result['min_ms']:.2f} ms")
            print(f"  Max:  {cpython_result['max_ms']:.2f} ms")
            print(f"  Output: {cpython_result['output']}")
        print()

        if args.cpython_only:
            print(json.dumps({"cpython": cpython_result}, indent=2))
            return

        # Molt → Luau compilation
        luau_path = py_path.replace(".py", ".luau")
        print("Compiling to Luau via Molt...")
        t0 = time.perf_counter()
        ok = compile_to_luau(py_path, luau_path)
        compile_time = time.perf_counter() - t0
        if not ok:
            print("  Compilation failed. Run with --cpython-only to skip.")
            return
        luau_size = os.path.getsize(luau_path)
        print(f"  Compile time: {compile_time * 1000:.0f} ms")
        print(f"  Output size: {luau_size} bytes ({luau_size // 1024} KB)")
        print()

        # Lune (Luau VM)
        print("Running Lune (Luau VM) benchmark...")
        lune_result = run_lune_bench(luau_path, args.iterations)
        if "error" not in lune_result:
            print(f"  Mean: {lune_result['mean_ms']:.2f} ms")
            print(f"  Min:  {lune_result['min_ms']:.2f} ms")
            print(f"  Max:  {lune_result['max_ms']:.2f} ms")
            print(f"  Output: {lune_result['output']}")
        print()

        # Comparison
        if "error" not in cpython_result and "error" not in lune_result:
            ratio = cpython_result["mean_ms"] / lune_result["mean_ms"]
            match = cpython_result["output"] == lune_result["output"]
            print("=== Results ===")
            print(f"  CPython mean:  {cpython_result['mean_ms']:.2f} ms")
            print(f"  Luau VM mean:  {lune_result['mean_ms']:.2f} ms")
            print(f"  Ratio:         {ratio:.2f}x {'(Luau faster)' if ratio > 1 else '(CPython faster)'}")
            print(f"  Output match:  {'YES' if match else 'NO (integer overflow divergence expected)'}")

        report = {
            "cpython": cpython_result,
            "luau": lune_result if not args.cpython_only else None,
            "compile_time_ms": round(compile_time * 1000, 0),
            "luau_output_bytes": luau_size,
        }
        print()
        print(json.dumps(report, indent=2))

    finally:
        os.unlink(py_path)
        luau_path = py_path.replace(".py", ".luau")
        if os.path.exists(luau_path):
            os.unlink(luau_path)


if __name__ == "__main__":
    main()
