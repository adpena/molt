import argparse
import datetime as dt
import importlib.util
import json
import os
import platform
import re
import shlex
import signal
import shutil
import socket
import statistics
import subprocess
import sys
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
    "tests/benchmarks/bench_counter_words.py",
    "tests/benchmarks/bench_etl_orders.py",
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
    "tests/benchmarks/bench_json_roundtrip.py",
]

SMOKE_BENCHMARKS = [
    "tests/benchmarks/bench_sum.py",
    "tests/benchmarks/bench_bytes_find.py",
]

WS_BENCHMARKS = [
    "tests/benchmarks/bench_ws_wait.py",
]

DYNAMIC_BUILTIN_SLICES = [
    "tests/benchmarks/bench_builtin_locals_slice.py",
    "tests/benchmarks/bench_builtin_dir_slice.py",
    "tests/benchmarks/bench_builtin_import_slice.py",
    "tests/benchmarks/bench_builtin_delattr_slice.py",
]

MOLT_ARGS_BY_BENCH = {
    "tests/benchmarks/bench_sum_list_hints.py": ["--type-hints", "trust"],
}

CODON_BENCH_RUNTIME_ARGS_BY_NAME = {
    "binary_trees.py": ["20"],
    "chaos.py": ["{DEVNULL}"],
    "fannkuch.py": ["11"],
    "nbody.py": ["10000000"],
    "set_partition.py": ["15"],
    "primes.py": ["100000"],
    "taq.py": ["{TAQ_FILE}"],
    "word_count.py": ["{WORD_FILE}"],
}


@dataclass(frozen=True)
class BenchRunner:
    cmd: list[str]
    script: str | None
    env: dict[str, str]
    build_s: float = 0.0
    size_kb: float | None = None


@dataclass(frozen=True)
class MoltBinary:
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


def _pid_alive(pid: int) -> bool:
    if pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def _kill_pid(pid: int, *, grace: float = 0.75) -> None:
    if pid <= 0:
        return
    try:
        os.kill(pid, signal.SIGTERM)
    except OSError:
        return
    deadline = time.monotonic() + max(0.05, grace)
    while time.monotonic() < deadline:
        if not _pid_alive(pid):
            return
        time.sleep(0.05)
    try:
        os.kill(pid, signal.SIGKILL)
    except OSError:
        return


def _daemon_ping(socket_path: Path, *, timeout: float = 0.75) -> bool:
    if os.name != "posix" or not socket_path.exists():
        return False
    payload = {"version": 1, "ping": True}
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.settimeout(timeout)
            sock.connect(str(socket_path))
            sock.sendall((json.dumps(payload) + "\n").encode("utf-8"))
            sock.shutdown(socket.SHUT_WR)
            chunks: list[bytes] = []
            while True:
                chunk = sock.recv(65536)
                if not chunk:
                    break
                chunks.append(chunk)
    except OSError:
        return False
    try:
        response = json.loads(b"".join(chunks).decode("utf-8", "replace").strip())
    except json.JSONDecodeError:
        return False
    return bool(response.get("ok")) and bool(response.get("pong"))


def _prune_backend_daemons() -> None:
    if os.name != "posix":
        return
    try:
        result = subprocess.run(
            ["ps", "-axo", "pid=,command="],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return
    pattern = re.compile(r"^\s*(\d+)\s+(.*)$")
    socket_pat = re.compile(r"--socket\s+(\S+)")
    groups: dict[Path, list[int]] = {}
    for line in result.stdout.splitlines():
        match = pattern.match(line)
        if match is None:
            continue
        pid = int(match.group(1))
        cmd = match.group(2)
        if "molt-backend" not in cmd or "--daemon" not in cmd:
            continue
        socket_match = socket_pat.search(cmd)
        if socket_match is None:
            continue
        socket_path = Path(socket_match.group(1)).expanduser()
        groups.setdefault(socket_path, []).append(pid)
    for socket_path, pids in groups.items():
        live = sorted({pid for pid in pids if _pid_alive(pid)})
        if not live:
            continue
        if not socket_path.exists():
            for pid in live:
                _kill_pid(pid)
            continue
        if len(live) > 1:
            for pid in live[:-1]:
                _kill_pid(pid)
            live = live[-1:]
        _daemon_ping(socket_path)


def _is_codon_bench_script(script: str) -> bool:
    normalized = Path(script).as_posix()
    return "codon_benchmarks/bench/codon/" in normalized


def _default_codon_taq_file() -> Path:
    explicit = os.environ.get("MOLT_BENCH_CODON_TAQ_FILE")
    if explicit:
        return Path(explicit).expanduser().resolve()
    repo_sample = Path("bench/friends/repos/codon_benchmarks/bench/data/taq.txt")
    if repo_sample.exists():
        return repo_sample.resolve()
    generated = Path(tempfile.gettempdir()) / "molt_codon_taq_sample.txt"
    if generated.exists():
        return generated.resolve()
    lines = ["timestamp|source|symbol|price|volume\n"]
    symbols = ("AAPL", "MSFT", "GOOG")
    for i in range(6000):
        timestamp = 1_700_000_000_000 + (i * 1_000_000)
        symbol = symbols[i % len(symbols)]
        volume = 100 + (i % 97)
        lines.append(f"{timestamp}|Q|{symbol}|0|{volume}\n")
    generated.write_text("".join(lines), encoding="utf-8")
    return generated.resolve()


def _default_codon_word_file() -> Path:
    explicit = os.environ.get("MOLT_BENCH_CODON_WORD_FILE")
    if explicit:
        return Path(explicit).expanduser().resolve()
    return _default_codon_taq_file()


def resolve_benchmark_run_args(script: str) -> list[str]:
    if not _is_codon_bench_script(script):
        return []
    args = CODON_BENCH_RUNTIME_ARGS_BY_NAME.get(Path(script).name, [])
    resolved: list[str] = []
    for arg in args:
        if arg == "{DEVNULL}":
            resolved.append(os.devnull)
        elif arg == "{TAQ_FILE}":
            resolved.append(str(_default_codon_taq_file()))
        elif arg == "{WORD_FILE}":
            resolved.append(str(_default_codon_word_file()))
        else:
            resolved.append(arg)
    return resolved


def measure_runtime(
    cmd_args,
    script=None,
    env=None,
    run_args=None,
    timeout_s: float | None = None,
    label: str | None = None,
):
    start = time.perf_counter()
    full_cmd = cmd_args + ([script] if script else [])
    if run_args:
        full_cmd.extend(run_args)
    try:
        res = subprocess.run(
            full_cmd,
            capture_output=True,
            text=True,
            env=env,
            timeout=timeout_s,
        )
    except subprocess.TimeoutExpired:
        msg = f" timed out after {timeout_s:.1f}s" if timeout_s is not None else ""
        bench_label = f" for {label}" if label else ""
        print(f"Benchmark run{bench_label}{msg}.", file=sys.stderr)
        return None
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
        "--trusted",
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


def measure_molt_run(
    binary: Path,
    env: dict[str, str] | None = None,
    label: str | None = None,
    run_args: list[str] | None = None,
    timeout_s: float | None = None,
) -> float | None:
    start = time.perf_counter()
    cmd = [str(binary)]
    if run_args:
        cmd.extend(run_args)
    try:
        res = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            env=env,
            timeout=timeout_s,
        )
    except subprocess.TimeoutExpired:
        msg = f" timed out after {timeout_s:.1f}s" if timeout_s is not None else ""
        if label:
            print(f"Molt run timed out for {label}{msg}.", file=sys.stderr)
        else:
            print(f"Molt run timed out{msg}.", file=sys.stderr)
        return None
    end = time.perf_counter()
    if res.returncode != 0:
        err = (res.stderr or res.stdout).strip()
        if err:
            prefix = f"Molt run failed for {label}: " if label else "Molt run failed: "
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


def _module_available(name: str) -> bool:
    return importlib.util.find_spec(name) is not None


def _find_compiled_binary(output_dir: Path, stem: str) -> Path | None:
    candidates = [
        output_dir / stem,
        output_dir / f"{stem}.bin",
        output_dir / f"{stem}.exe",
    ]
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    for candidate in sorted(output_dir.glob(f"{stem}*")):
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return candidate
    return None


def _nuitka_command(explicit_cmd: str | None) -> list[str] | None:
    if explicit_cmd:
        parts = shlex.split(explicit_cmd)
        return parts if parts else None
    nuitka = shutil.which("nuitka")
    if nuitka:
        return [nuitka]
    if _module_available("nuitka"):
        return [sys.executable, "-m", "nuitka"]
    return None


def _prepare_nuitka_runner(
    script_path: Path,
    build_root: Path,
    base_env: dict[str, str],
    *,
    tty: bool,
    nuitka_cmd: list[str] | None,
) -> BenchRunner | None:
    if nuitka_cmd is None:
        return None
    module_name = f"bench_nuitka_{script_path.stem}"
    module_dir = build_root / module_name
    module_dir.mkdir(parents=True, exist_ok=True)
    build_start = time.perf_counter()
    build = _run_cmd(
        [
            *nuitka_cmd,
            "--onefile",
            "--output-dir",
            str(module_dir),
            "--remove-output",
            str(script_path),
        ],
        env=base_env,
        capture=not tty,
        tty=tty,
    )
    build_s = time.perf_counter() - build_start
    if build.returncode != 0:
        return None
    binary_path = _find_compiled_binary(module_dir, script_path.stem)
    if binary_path is None:
        return None
    size_kb = binary_path.stat().st_size / 1024
    return BenchRunner(
        [str(binary_path)], None, base_env, build_s=build_s, size_kb=size_kb
    )


def _pyodide_command(explicit_cmd: str | None) -> list[str] | None:
    if explicit_cmd:
        parts = shlex.split(explicit_cmd)
        return parts if parts else None
    env_cmd = os.environ.get("MOLT_BENCH_PYODIDE_CMD", "").strip()
    if env_cmd:
        parts = shlex.split(env_cmd)
        return parts if parts else None
    return None


def _prepare_pyodide_runner(
    script_path: Path,
    base_env: dict[str, str],
    *,
    pyodide_cmd: list[str] | None,
) -> BenchRunner | None:
    if pyodide_cmd is None:
        return None
    return BenchRunner(pyodide_cmd, str(script_path), base_env)


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
    build_start = time.perf_counter()
    build = _run_cmd(
        arch_prefix
        + [codon, "build", "-release", str(script_path), "-o", str(binary_path)],
        env=env,
        capture=not tty,
        tty=tty,
    )
    build_s = time.perf_counter() - build_start
    if build.returncode != 0:
        return None
    if codon_home:
        libomp = Path(codon_home) / "lib" / "codon" / "libomp.dylib"
        target = module_dir / "libomp.dylib"
        if libomp.exists() and not target.exists():
            shutil.copy2(libomp, target)
    size_kb = binary_path.stat().st_size / 1024 if binary_path.exists() else None
    return BenchRunner(
        arch_prefix + [str(binary_path)], None, env, build_s=build_s, size_kb=size_kb
    )


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
    use_cpython,
    use_pypy,
    use_codon,
    use_nuitka,
    use_pyodide,
    super_run,
    runtime_timeout_s,
    *,
    tty: bool,
    nuitka_cmd: str | None,
    pyodide_cmd: str | None,
):
    runtimes = {}
    if use_cpython:
        runtimes["cpython"] = [sys.executable]
    if use_pypy:
        pypy_cmd = _pypy_command()
        if pypy_cmd:
            runtimes["pypy"] = pypy_cmd

    if use_codon and not shutil.which("codon"):
        print("Skipping Codon: codon not found.", file=sys.stderr)
        use_codon = False
    resolved_nuitka_cmd = _nuitka_command(nuitka_cmd) if use_nuitka else None
    if use_nuitka and resolved_nuitka_cmd is None:
        print(
            "Skipping Nuitka: nuitka not found (or pass --nuitka-cmd).",
            file=sys.stderr,
        )
        use_nuitka = False
    resolved_pyodide_cmd = _pyodide_command(pyodide_cmd) if use_pyodide else None
    if use_pyodide and resolved_pyodide_cmd is None:
        print(
            "Skipping Pyodide: set --pyodide-cmd or MOLT_BENCH_PYODIDE_CMD.",
            file=sys.stderr,
        )
        use_pyodide = False

    header = (
        f"{'Benchmark':<30} | {'CPython(s)':<10} | {'PyPy(s)':<10} | "
        f"{'CodonBuild':<10} | {'CodonRun':<10} | {'NuitkaBld':<10} | "
        f"{'NuitkaRun':<10} | {'PyodideRun':<10} | {'MoltBuild':<10} | "
        f"{'MoltRun':<10} | {'MoltKB':<10} | {'Speedup':<10} | {'M/PyPy':<10} | "
        f"{'M/Codon':<10} | {'M/Nuitka':<10} | {'M/Pyodide':<10}"
    )
    print(header)
    print("-" * len(header))

    base_env = _base_python_env()
    codon_root = Path("bench/codon")
    nuitka_root = Path("bench/nuitka")

    data = {}
    for script in benchmarks:
        name = os.path.basename(script)
        run_args = resolve_benchmark_run_args(script)
        results = {}
        runtime_ok = {}
        stats = {}
        for rt_name, cmd in runtimes.items():
            samples_list, ok = collect_samples(
                lambda: measure_runtime(
                    cmd,
                    script,
                    env=base_env,
                    run_args=run_args,
                    timeout_s=runtime_timeout_s,
                    label=f"{name} [{rt_name}]",
                ),
                samples,
                warmup=warmup,
            )
            results[rt_name] = statistics.mean(samples_list) if ok else None
            runtime_ok[rt_name] = ok
            if super_run and ok:
                stats[rt_name] = summarize_samples(samples_list)

        codon_time: float | None = None
        codon_build: float | None = None
        codon_size: float | None = None
        codon_ok = False
        if use_codon:
            runner = _prepare_codon_runner(Path(script), codon_root, base_env, tty=tty)
            if runner is not None:
                codon_build = runner.build_s
                codon_size = runner.size_kb
                codon_samples, codon_ok = collect_samples(
                    lambda: measure_runtime(
                        runner.cmd,
                        runner.script,
                        env=runner.env,
                        run_args=run_args,
                        timeout_s=runtime_timeout_s,
                        label=f"{name} [codon]",
                    ),
                    samples,
                    warmup=warmup,
                )
                if codon_ok:
                    codon_time = statistics.mean(codon_samples)
                    if super_run:
                        stats["codon"] = summarize_samples(codon_samples)
            else:
                print(f"Skipping Codon for {name}.", file=sys.stderr)

        nuitka_time: float | None = None
        nuitka_build: float | None = None
        nuitka_size: float | None = None
        nuitka_ok = False
        if use_nuitka:
            runner = _prepare_nuitka_runner(
                Path(script),
                nuitka_root,
                base_env,
                tty=tty,
                nuitka_cmd=resolved_nuitka_cmd,
            )
            if runner is not None:
                nuitka_build = runner.build_s
                nuitka_size = runner.size_kb
                nuitka_samples, nuitka_ok = collect_samples(
                    lambda: measure_runtime(
                        runner.cmd,
                        runner.script,
                        env=runner.env,
                        run_args=run_args,
                        timeout_s=runtime_timeout_s,
                        label=f"{name} [nuitka]",
                    ),
                    samples,
                    warmup=warmup,
                )
                if nuitka_ok:
                    nuitka_time = statistics.mean(nuitka_samples)
                    if super_run:
                        stats["nuitka"] = summarize_samples(nuitka_samples)
            else:
                print(f"Skipping Nuitka for {name}.", file=sys.stderr)

        pyodide_time: float | None = None
        pyodide_build: float | None = None
        pyodide_size: float | None = None
        pyodide_ok = False
        if use_pyodide:
            runner = _prepare_pyodide_runner(
                Path(script), base_env, pyodide_cmd=resolved_pyodide_cmd
            )
            if runner is not None:
                pyodide_samples, pyodide_ok = collect_samples(
                    lambda: measure_runtime(
                        runner.cmd,
                        runner.script,
                        env=runner.env,
                        run_args=run_args,
                        timeout_s=runtime_timeout_s,
                        label=f"{name} [pyodide]",
                    ),
                    samples,
                    warmup=warmup,
                )
                if pyodide_ok:
                    pyodide_time = statistics.mean(pyodide_samples)
                    if super_run:
                        stats["pyodide"] = summarize_samples(pyodide_samples)
            else:
                print(f"Skipping Pyodide for {name}.", file=sys.stderr)

        molt_time: float | None = None
        molt_size: float | None = None
        molt_build: float | None = None
        molt_args = MOLT_ARGS_BY_BENCH.get(script, [])
        molt_ok = False
        molt_samples: list[float] = []
        molt_runner = prepare_molt_binary(script, molt_args, env=base_env)
        if molt_runner is not None:
            try:
                molt_samples, molt_ok = collect_samples(
                    lambda: measure_molt_run(
                        molt_runner.path,
                        env=base_env,
                        label=name,
                        run_args=run_args,
                        timeout_s=runtime_timeout_s,
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

        cpython_time = (
            results.get("cpython") if runtime_ok.get("cpython", False) else None
        )
        pypy_time = results.get("pypy") if runtime_ok.get("pypy", False) else None
        speedup = (
            (cpython_time / molt_time)
            if (cpython_time is not None and molt_ok and molt_time > 0)
            else None
        )
        ratio = (
            molt_time / cpython_time
            if (molt_ok and cpython_time is not None and cpython_time > 0)
            else None
        )
        pypy_ratio = (
            (molt_time / pypy_time)
            if (molt_ok and pypy_time is not None and pypy_time > 0)
            else None
        )
        codon_ratio = (
            (molt_time / codon_time)
            if molt_ok and codon_ok and codon_time > 0
            else None
        )
        nuitka_ratio = (
            (molt_time / nuitka_time)
            if molt_ok and nuitka_ok and nuitka_time is not None and nuitka_time > 0
            else None
        )
        pyodide_ratio = (
            (molt_time / pyodide_time)
            if molt_ok and pyodide_ok and pyodide_time is not None and pyodide_time > 0
            else None
        )

        def _cell(value: float | None, width: int = 10) -> str:
            if value is None:
                return f"{'n/a':<{width}}"
            return f"{value:<{width}.4f}"

        def _ratio_cell(value: float | None, width: int = 10) -> str:
            if value is None:
                return f"{'n/a':<{width}}"
            return f"{value:<{width}.2f}x"

        cpython_cell = _cell(cpython_time)
        pypy_cell = _cell(pypy_time)
        codon_build_cell = _cell(codon_build)
        codon_run_cell = _cell(codon_time)
        nuitka_build_cell = _cell(nuitka_build)
        nuitka_run_cell = _cell(nuitka_time)
        pyodide_run_cell = _cell(pyodide_time)
        molt_build_cell = _cell(molt_build)
        molt_run_cell = _cell(molt_time)
        molt_size_cell = _cell(molt_size)
        speedup_cell = _ratio_cell(speedup)
        pypy_ratio_cell = _ratio_cell(pypy_ratio)
        codon_ratio_cell = _ratio_cell(codon_ratio)
        nuitka_ratio_cell = _ratio_cell(nuitka_ratio)
        pyodide_ratio_cell = _ratio_cell(pyodide_ratio)

        print(
            f"{name:<30} | {cpython_cell} | {pypy_cell} | {codon_build_cell} | "
            f"{codon_run_cell} | {nuitka_build_cell} | {nuitka_run_cell} | "
            f"{pyodide_run_cell} | {molt_build_cell} | {molt_run_cell} | "
            f"{molt_size_cell} | {speedup_cell} | {pypy_ratio_cell} | "
            f"{codon_ratio_cell} | {nuitka_ratio_cell} | {pyodide_ratio_cell}"
        )

        data[name] = {
            "cpython_time_s": cpython_time,
            "pypy_time_s": pypy_time,
            "codon_time_s": codon_time,
            "codon_build_s": codon_build,
            "codon_size_kb": codon_size,
            "nuitka_time_s": nuitka_time,
            "nuitka_build_s": nuitka_build,
            "nuitka_size_kb": nuitka_size,
            "pyodide_time_s": pyodide_time,
            "pyodide_build_s": pyodide_build,
            "pyodide_size_kb": pyodide_size,
            "molt_time_s": molt_time,
            "molt_build_s": molt_build,
            "molt_size_kb": molt_size,
            "molt_speedup": speedup,
            "molt_cpython_ratio": ratio,
            "molt_pypy_ratio": pypy_ratio,
            "molt_codon_ratio": codon_ratio,
            "molt_nuitka_ratio": nuitka_ratio,
            "molt_pyodide_ratio": pyodide_ratio,
            "molt_ok": molt_ok,
            "pypy_ok": runtime_ok.get("pypy", False),
            "molt_args": molt_args,
            "run_args": run_args,
            "codon_ok": codon_ok,
            "nuitka_ok": nuitka_ok,
            "pyodide_ok": pyodide_ok,
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
    parser.add_argument(
        "--no-cpython",
        action="store_true",
        help="Skip CPython timing lane (useful when focusing on Molt vs external lanes).",
    )
    parser.add_argument("--no-pypy", action="store_true")
    parser.add_argument("--no-codon", action="store_true")
    parser.add_argument("--no-nuitka", action="store_true")
    parser.add_argument("--no-pyodide", action="store_true")
    parser.add_argument(
        "--nuitka-cmd",
        default=None,
        help=(
            "Override Nuitka command prefix, e.g. 'python -m nuitka'. "
            "Default auto-probes `nuitka` then `python -m nuitka`."
        ),
    )
    parser.add_argument(
        "--pyodide-cmd",
        default=None,
        help=(
            "Pyodide run command prefix (also reads MOLT_BENCH_PYODIDE_CMD). "
            "The command must accept `<script> [args...]`."
        ),
    )
    parser.add_argument(
        "--ws",
        action="store_true",
        help="Include websocket wait benchmark (also honors MOLT_BENCH_WS=1).",
    )
    parser.add_argument(
        "--dynamic-builtin-only",
        action="store_true",
        help=(
            "Run only isolated locals/dir/__import__/delattr benchmark slices; "
            "kept out of core throughput KPI lanes."
        ),
    )
    parser.add_argument(
        "--script",
        action="append",
        help="Benchmark a custom script path (repeatable).",
    )
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
    parser.add_argument(
        "--runtime-timeout-sec",
        type=float,
        default=None,
        help="Optional per-run timeout in seconds for each benchmark process.",
    )
    args = parser.parse_args()

    if args.super and args.smoke:
        parser.error("--super cannot be combined with --smoke")
    if args.super and args.samples is not None:
        parser.error("--super cannot be combined with --samples")
    if args.dynamic_builtin_only and args.smoke:
        parser.error("--dynamic-builtin-only cannot be combined with --smoke")

    if args.script:
        if args.smoke:
            parser.error("--script cannot be combined with --smoke")
        if args.dynamic_builtin_only:
            parser.error("--script cannot be combined with --dynamic-builtin-only")
        benchmarks = [str(Path(path)) for path in args.script]
        missing = [path for path in benchmarks if not Path(path).exists()]
        if missing:
            parser.error(f"Script(s) not found: {', '.join(missing)}")
    else:
        if args.dynamic_builtin_only:
            benchmarks = list(DYNAMIC_BUILTIN_SLICES)
        else:
            benchmarks = list(SMOKE_BENCHMARKS) if args.smoke else list(BENCHMARKS)
    include_ws = not args.dynamic_builtin_only and (
        args.ws or os.environ.get("MOLT_BENCH_WS") == "1"
    )
    if include_ws:
        for bench in WS_BENCHMARKS:
            if bench not in benchmarks:
                benchmarks.append(bench)
    samples = (
        SUPER_SAMPLES
        if args.super
        else (args.samples if args.samples is not None else (1 if args.smoke else 3))
    )
    use_cpython = not args.no_cpython
    use_pypy = not args.no_pypy
    use_codon = not args.no_codon
    use_nuitka = not args.no_nuitka
    use_pyodide = not args.no_pyodide
    use_tty = args.tty or os.environ.get("MOLT_TTY") == "1"

    _prune_backend_daemons()

    warmup = args.warmup if args.warmup is not None else (0 if args.smoke else 1)
    results = bench_results(
        benchmarks,
        samples,
        warmup,
        use_cpython,
        use_pypy,
        use_codon,
        use_nuitka,
        use_pyodide,
        args.super,
        args.runtime_timeout_sec,
        tty=use_tty,
        nuitka_cmd=args.nuitka_cmd,
        pyodide_cmd=args.pyodide_cmd,
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
