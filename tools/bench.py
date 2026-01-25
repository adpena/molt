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
import tempfile
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


@dataclass(frozen=True)
class BenchRunner:
    cmd: list[str]
    script: str | None
    env: dict[str, str]


@dataclass(frozen=True)
class MoltBinary:
    path: Path
    temp_dir: tempfile.TemporaryDirectory
    build_s: float
    size_kb: float


@dataclass(frozen=True)
class DepylerBinary:
    path: Path
    temp_dir: tempfile.TemporaryDirectory
    build_s: float
    size_kb: float


@dataclass(frozen=True)
class _RunResult:
    returncode: int
    stdout: str = ""
    stderr: str = ""


def _enable_line_buffering() -> None:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(line_buffering=True)
        except AttributeError:
            continue


def _run_with_pty(cmd: list[str], env: dict[str, str]) -> _RunResult:
    import os
    import pty

    master_fd, slave_fd = pty.openpty()
    try:
        proc = subprocess.Popen(
            cmd,
            env=env,
            stdin=slave_fd,
            stdout=slave_fd,
            stderr=slave_fd,
        )
    finally:
        os.close(slave_fd)

    try:
        while True:
            data = os.read(master_fd, 1024)
            if not data:
                break
            if hasattr(sys.stdout, "buffer"):
                sys.stdout.buffer.write(data)
                sys.stdout.buffer.flush()
            else:
                sys.stdout.write(data.decode(errors="replace"))
                sys.stdout.flush()
    except KeyboardInterrupt:
        proc.terminate()
        raise
    finally:
        os.close(master_fd)

    return _RunResult(returncode=proc.wait())


def _run_cmd(
    cmd: list[str], env: dict[str, str], *, capture: bool, tty: bool
) -> _RunResult:
    if tty and not capture and os.name == "posix":
        return _run_with_pty(cmd, env)
    res = subprocess.run(cmd, capture_output=capture, text=True, env=env)
    return _RunResult(res.returncode, res.stdout or "", res.stderr or "")


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
    env.setdefault("PYTHONHASHSEED", "0")
    env.setdefault("PYTHONUNBUFFERED", "1")
    return _prepend_pythonpath(env, "src")


def measure_runtime(cmd_args, script=None, env=None):
    start = time.perf_counter()
    full_cmd = cmd_args + ([script] if script else [])
    res = subprocess.run(full_cmd, capture_output=True, text=True, env=env)
    end = time.perf_counter()
    if res.returncode != 0:
        return None
    return end - start


def _resolve_molt_output(payload: dict) -> Path | None:
    output_str = payload.get("data", {}).get("output") or payload.get("output")
    if not output_str:
        return None
    output_path = Path(output_str)
    if output_path.exists():
        return output_path
    fallback = output_path.with_suffix(".exe")
    if fallback.exists():
        return fallback
    return None


def _resolve_depyler_output(output_path: Path) -> Path | None:
    if output_path.exists():
        return output_path
    fallback = output_path.with_suffix(".exe")
    if fallback.exists():
        return fallback
    return None


def prepare_molt_binary(
    script: str, extra_args: list[str] | None = None, env: dict[str, str] | None = None
) -> MoltBinary | None:
    env = (env or os.environ.copy()).copy()
    env["PYTHONPATH"] = "src"
    temp_dir = tempfile.TemporaryDirectory(prefix="molt-bench-")
    out_dir = Path(temp_dir.name)
    args = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--json",
        "--out-dir",
        str(out_dir),
    ]
    if extra_args:
        args.extend(extra_args)
    args.append(script)
    start = time.perf_counter()
    res = subprocess.run(
        args,
        env=env,
        capture_output=True,
        text=True,
    )
    build_s = time.perf_counter() - start

    if res.returncode != 0:
        temp_dir.cleanup()
        return None

    try:
        payload = json.loads(res.stdout.strip() or "{}")
    except json.JSONDecodeError:
        temp_dir.cleanup()
        return None

    output_path = _resolve_molt_output(payload)
    if output_path is None:
        temp_dir.cleanup()
        return None

    binary_size = output_path.stat().st_size / 1024
    return MoltBinary(output_path, temp_dir, build_s, binary_size)


def prepare_depyler_binary(
    script: str,
    *,
    env: dict[str, str] | None = None,
    profile: str = "release",
    tty: bool = False,
) -> DepylerBinary | None:
    env = (env or os.environ.copy()).copy()
    temp_dir = tempfile.TemporaryDirectory(prefix="depyler-bench-")
    out_dir = Path(temp_dir.name)
    output_path = out_dir / Path(script).stem
    env.setdefault("CARGO_TARGET_DIR", str(out_dir / "cargo-target"))
    cmd = [
        "depyler",
        "compile",
        script,
        "--output",
        str(output_path),
        "--profile",
        profile,
    ]
    start = time.perf_counter()
    res = _run_cmd(cmd, env, capture=not tty, tty=tty)
    build_s = time.perf_counter() - start
    if res.returncode != 0:
        err = (res.stderr or res.stdout).strip()
        if err:
            print(f"Depyler compile failed for {script}: {err}", file=sys.stderr)
        temp_dir.cleanup()
        return None
    resolved = _resolve_depyler_output(output_path)
    if resolved is None:
        temp_dir.cleanup()
        return None
    binary_size = resolved.stat().st_size / 1024
    return DepylerBinary(resolved, temp_dir, build_s, binary_size)


def measure_molt_run(
    binary: Path, env: dict[str, str] | None = None, label: str | None = None
) -> float | None:
    start = time.perf_counter()
    res = subprocess.run([str(binary)], capture_output=True, text=True, env=env)
    end = time.perf_counter()
    if res.returncode != 0:
        err = (res.stderr or res.stdout).strip()
        if err:
            prefix = f"Molt run failed for {label}: " if label else "Molt run failed: "
            print(f"{prefix}{err}", file=sys.stderr)
        return None
    return end - start


def measure_depyler_run(
    binary: Path, env: dict[str, str] | None = None, label: str | None = None
) -> float | None:
    start = time.perf_counter()
    res = subprocess.run([str(binary)], capture_output=True, text=True, env=env)
    end = time.perf_counter()
    if res.returncode != 0:
        err = (res.stderr or res.stdout).strip()
        if err:
            prefix = (
                f"Depyler run failed for {label}: " if label else "Depyler run failed: "
            )
            print(f"{prefix}{err}", file=sys.stderr)
        return None
    return end - start


def collect_samples(measure_fn, samples, warmup=0):
    for _ in range(warmup):
        if measure_fn() is None:
            return [], False
    times = [measure_fn() for _ in range(samples)]
    valid_times = [t for t in times if t is not None]
    return valid_times, bool(valid_times)


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
    script_path: Path, build_root: Path, base_env: dict[str, str], *, tty: bool
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
    warm = _run_cmd([sys.executable, str(runner_path)], env, capture=not tty, tty=tty)
    if warm.returncode != 0:
        return None
    return BenchRunner([sys.executable], str(runner_path), env)


def _prepare_numba_runner(
    script_path: Path, build_root: Path, base_env: dict[str, str], *, tty: bool
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
    warm = _run_cmd([sys.executable, str(runner_path)], env, capture=not tty, tty=tty)
    if warm.returncode != 0:
        return None
    return BenchRunner([sys.executable], str(runner_path), env)


def _prepare_codon_runner(
    script_path: Path, build_root: Path, base_env: dict[str, str], *, tty: bool
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
    build = _run_cmd(
        arch_prefix
        + [codon, "build", "-release", str(script_path), "-o", str(binary_path)],
        env=env,
        capture=not tty,
        tty=tty,
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


def bench_results(
    benchmarks,
    samples,
    warmup,
    use_pypy,
    use_cython,
    use_numba,
    use_codon,
    use_depyler,
    super_run,
    *,
    tty: bool,
):
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
    if use_depyler and not shutil.which("depyler"):
        print("Skipping Depyler: depyler not found.", file=sys.stderr)
        use_depyler = False

    header = (
        f"{'Benchmark':<30} | {'CPython (s)':<12} | {'PyPy (s)':<12} | "
        f"{'Cython (s)':<12} | {'Numba (s)':<12} | {'Codon (s)':<12} | "
        f"{'Depyler (s)':<12} | {'Molt/Codon':<12} | {'Molt/Depyler':<12} | "
        f"{'Molt (s)':<10} | "
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
        stats = {}
        for rt_name, cmd in runtimes.items():
            samples_list, ok = collect_samples(
                lambda: measure_runtime(cmd, script, env=base_env),
                samples,
                warmup=warmup,
            )
            results[rt_name] = statistics.mean(samples_list) if ok else 0.0
            runtime_ok[rt_name] = ok
            if super_run and ok:
                stats[rt_name.lower()] = summarize_samples(samples_list)

        cython_time = 0.0
        cython_ok = False
        if use_cython:
            runner = _prepare_cython_runner(
                Path(script), cython_root, base_env, tty=tty
            )
            if runner is not None:
                cython_samples, cython_ok = collect_samples(
                    lambda: measure_runtime(runner.cmd, runner.script, env=runner.env),
                    samples,
                    warmup=warmup,
                )
                if cython_ok:
                    cython_time = statistics.mean(cython_samples)
                    if super_run:
                        stats["cython"] = summarize_samples(cython_samples)
            else:
                print(f"Skipping Cython for {name}.", file=sys.stderr)

        numba_time = 0.0
        numba_ok = False
        if use_numba:
            runner = _prepare_numba_runner(Path(script), numba_root, base_env, tty=tty)
            if runner is not None:
                numba_samples, numba_ok = collect_samples(
                    lambda: measure_runtime(runner.cmd, runner.script, env=runner.env),
                    samples,
                    warmup=warmup,
                )
                if numba_ok:
                    numba_time = statistics.mean(numba_samples)
                    if super_run:
                        stats["numba"] = summarize_samples(numba_samples)
            else:
                print(f"Skipping Numba for {name}.", file=sys.stderr)

        codon_time = 0.0
        codon_ok = False
        if use_codon:
            runner = _prepare_codon_runner(Path(script), codon_root, base_env, tty=tty)
            if runner is not None:
                codon_samples, codon_ok = collect_samples(
                    lambda: measure_runtime(runner.cmd, runner.script, env=runner.env),
                    samples,
                    warmup=warmup,
                )
                if codon_ok:
                    codon_time = statistics.mean(codon_samples)
                    if super_run:
                        stats["codon"] = summarize_samples(codon_samples)
            else:
                print(f"Skipping Codon for {name}.", file=sys.stderr)

        depyler_time = 0.0
        depyler_ok = False
        depyler_build = 0.0
        depyler_size = 0.0
        depyler_samples: list[float] = []
        if use_depyler:
            depyler_runner = prepare_depyler_binary(script, env=base_env, tty=tty)
            if depyler_runner is not None:
                try:
                    depyler_samples, depyler_ok = collect_samples(
                        lambda: measure_depyler_run(
                            depyler_runner.path, env=base_env, label=name
                        ),
                        samples,
                        warmup=warmup,
                    )
                    if depyler_ok:
                        depyler_time = statistics.mean(depyler_samples)
                        if super_run:
                            stats["depyler"] = summarize_samples(depyler_samples)
                    depyler_build = depyler_runner.build_s
                    depyler_size = depyler_runner.size_kb
                finally:
                    depyler_runner.temp_dir.cleanup()
            else:
                print(f"Depyler build/run failed for {name}.", file=sys.stderr)

        molt_time, molt_size, molt_build = 0.0, 0.0, 0.0
        molt_args = MOLT_ARGS_BY_BENCH.get(script, [])
        molt_ok = False
        molt_samples: list[float] = []
        molt_runner = prepare_molt_binary(script, molt_args, env=base_env)
        if molt_runner is not None:
            try:
                molt_samples, molt_ok = collect_samples(
                    lambda: measure_molt_run(
                        molt_runner.path, env=base_env, label=name
                    ),
                    samples,
                    warmup=warmup,
                )
                if molt_ok:
                    molt_time = statistics.mean(molt_samples)
                    if super_run:
                        stats["molt"] = summarize_samples(molt_samples)
                molt_build = molt_runner.build_s
                molt_size = molt_runner.size_kb
            finally:
                molt_runner.temp_dir.cleanup()
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
        depyler_ratio = (
            (molt_time / depyler_time)
            if molt_ok and depyler_ok and depyler_time > 0
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
        depyler_cell = f"{depyler_time:<12.4f}" if depyler_ok else f"{'n/a':<12}"
        codon_ratio_cell = (
            f"{codon_ratio:<12.2f}x" if codon_ratio is not None else f"{'n/a':<12}"
        )
        depyler_ratio_cell = (
            f"{depyler_ratio:<12.2f}x" if depyler_ratio is not None else f"{'n/a':<12}"
        )

        print(
            f"{name:<30} | {cpython_cell} | {pypy_cell} | {cython_cell} | "
            f"{numba_cell} | {codon_cell} | {depyler_cell} | {codon_ratio_cell} | "
            f"{depyler_ratio_cell} | "
            f"{molt_time:<10.4f} | {speedup:<12.2f}x | "
            f"{molt_size:.1f} KB"
        )

        data[name] = {
            "cpython_time_s": results.get("CPython", 0.0),
            "pypy_time_s": results.get("PyPy", 0.0),
            "cython_time_s": cython_time,
            "numba_time_s": numba_time,
            "codon_time_s": codon_time,
            "depyler_time_s": depyler_time,
            "molt_time_s": molt_time,
            "molt_build_s": molt_build,
            "molt_size_kb": molt_size,
            "depyler_build_s": depyler_build,
            "depyler_size_kb": depyler_size,
            "molt_speedup": speedup,
            "molt_cpython_ratio": ratio,
            "molt_codon_ratio": codon_ratio,
            "molt_depyler_ratio": depyler_ratio,
            "molt_ok": molt_ok,
            "molt_args": molt_args,
            "cython_ok": cython_ok,
            "numba_ok": numba_ok,
            "codon_ok": codon_ok,
            "depyler_ok": depyler_ok,
        }
        if super_run:
            data[name]["super_stats"] = stats

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
    _enable_line_buffering()
    parser = argparse.ArgumentParser(description="Run Molt benchmark suite.")
    parser.add_argument("--json-out", type=Path, default=None)
    parser.add_argument("--baseline", type=Path, default=None)
    parser.add_argument("--update-baseline", action="store_true")
    parser.add_argument("--max-regression", type=float, default=0.15)
    parser.add_argument("--samples", type=int, default=None)
    parser.add_argument(
        "--warmup",
        type=int,
        default=None,
        help="Warmup runs per benchmark before sampling (default: 1, or 0 for --smoke).",
    )
    parser.add_argument("--smoke", action="store_true")
    parser.add_argument("--no-pypy", action="store_true")
    parser.add_argument("--no-cython", action="store_true")
    parser.add_argument("--no-numba", action="store_true")
    parser.add_argument("--no-codon", action="store_true")
    parser.add_argument("--no-depyler", action="store_true")
    parser.add_argument(
        "--super",
        action="store_true",
        help="Run all benchmarks 10x and emit mean/median/variance/range stats.",
    )
    parser.add_argument(
        "--tty",
        action="store_true",
        help="Attach subprocesses to a pseudo-TTY for immediate output.",
    )
    args = parser.parse_args()

    if args.super and args.smoke:
        parser.error("--super cannot be combined with --smoke")
    if args.super and args.samples is not None:
        parser.error("--super cannot be combined with --samples")

    benchmarks = SMOKE_BENCHMARKS if args.smoke else BENCHMARKS
    samples = (
        SUPER_SAMPLES
        if args.super
        else (args.samples if args.samples is not None else (1 if args.smoke else 3))
    )
    use_pypy = not args.no_pypy
    use_cython = not args.no_cython
    use_numba = not args.no_numba
    use_codon = not args.no_codon
    use_depyler = not args.no_depyler
    use_tty = args.tty or os.environ.get("MOLT_TTY") == "1"

    warmup = args.warmup if args.warmup is not None else (0 if args.smoke else 1)
    results = bench_results(
        benchmarks,
        samples,
        warmup,
        use_pypy,
        use_cython,
        use_numba,
        use_codon,
        use_depyler,
        args.super,
        tty=use_tty,
    )

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
