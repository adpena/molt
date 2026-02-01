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
from typing import TextIO

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


def _log_write(log: TextIO | None, text: str) -> None:
    if log is None:
        return
    log.write(text)
    log.flush()


def _log_command(log: TextIO | None, cmd: list[str]) -> None:
    if log is None:
        return
    ts = dt.datetime.now(dt.timezone.utc).isoformat()
    _log_write(log, f"\n# {ts} $ {' '.join(cmd)}\n")


def _run_with_pty(
    cmd: list[str], env: dict[str, str], log: TextIO | None
) -> _RunResult:
    import os
    import pty

    _log_command(log, cmd)
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
            _log_write(log, data.decode(errors="replace"))
    except KeyboardInterrupt:
        proc.terminate()
        raise
    finally:
        os.close(master_fd)

    return _RunResult(returncode=proc.wait())


def _run_cmd(
    cmd: list[str],
    env: dict[str, str],
    *,
    capture: bool,
    tty: bool,
    log: TextIO | None,
) -> _RunResult:
    if tty and not capture and os.name == "posix":
        return _run_with_pty(cmd, env, log)
    if log is None and not capture:
        res = subprocess.run(cmd, text=True, env=env)
        return _RunResult(res.returncode)
    if log is None and capture:
        res = subprocess.run(cmd, capture_output=True, text=True, env=env)
        return _RunResult(res.returncode, res.stdout or "", res.stderr or "")

    _log_command(log, cmd)
    proc = subprocess.Popen(
        cmd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    output: list[str] = []
    if proc.stdout is not None:
        for line in proc.stdout:
            if not capture:
                sys.stdout.write(line)
                sys.stdout.flush()
            _log_write(log, line)
            output.append(line)
    return _RunResult(proc.wait(), "".join(output), "")


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
    env.setdefault("PYTHONUNBUFFERED", "1")
    env.setdefault("MOLT_MACOSX_DEPLOYMENT_TARGET", "26.2")
    return env


def _open_log(log_path: Path | None) -> TextIO | None:
    if log_path is None:
        return None
    log_path.parent.mkdir(parents=True, exist_ok=True)
    return log_path.open("a", encoding="utf-8", buffering=1)


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


def build_runtime_wasm(
    *, reloc: bool, output: Path, tty: bool, log: TextIO | None
) -> bool:
    env = os.environ.copy()
    if reloc:
        base_flags = (
            "-C link-arg=--relocatable -C link-arg=--no-gc-sections"
            " -C relocation-model=pic"
        )
    else:
        base_flags = (
            "-C link-arg=--import-memory -C link-arg=--import-table"
            " -C link-arg=--growable-table"
        )
    _append_rustflags(env, base_flags)
    res = _run_cmd(
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
        capture=not tty,
        tty=tty,
        log=log,
    )
    if res.returncode != 0:
        if res.stderr or res.stdout:
            err = (res.stderr or res.stdout).strip()
            if err:
                print(f"WASM runtime build failed: {err}", file=sys.stderr)
        else:
            print("WASM runtime build failed.", file=sys.stderr)
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


def _link_wasm(
    env: dict[str, str],
    input_path: Path,
    *,
    require_linked: bool,
    log: TextIO | None,
) -> Path | None:
    if not _want_linked():
        return None
    if WASM_LD is None:
        global _LINK_WARNED
        msg = "Skipping wasm link: wasm-ld not found (install LLVM to enable)."
        if require_linked:
            print(f"{msg} Linked output is required.", file=sys.stderr)
        elif not _LINK_WARNED:
            print(msg, file=sys.stderr)
            _LINK_WARNED = True
        return None
    global _LINK_DISABLED
    if _LINK_DISABLED:
        if require_linked:
            print(
                "WASM link disabled after prior failure; linked output is required.",
                file=sys.stderr,
            )
        return None
    if LINKED_WASM.exists():
        LINKED_WASM.unlink()
    runtime_path = RUNTIME_WASM_RELOC if RUNTIME_WASM_RELOC.exists() else RUNTIME_WASM
    res = _run_cmd(
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
        capture=True,
        tty=False,
        log=log,
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
        if require_linked:
            print("Linked output is required; aborting.", file=sys.stderr)
        return None
    if not LINKED_WASM.exists():
        print("WASM link produced no output artifact.", file=sys.stderr)
        return None
    return LINKED_WASM


def _build_wasm_output(
    python_exe: str,
    env: dict[str, str],
    output_path: Path,
    script: str,
    *,
    tty: bool,
    log: TextIO | None,
) -> float | None:
    extra_args = MOLT_ARGS_BY_BENCH.get(script, [])
    start = time.perf_counter()
    build_res = _run_cmd(
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
        capture=not tty,
        tty=tty,
        log=log,
    )
    build_s = time.perf_counter() - start
    if build_res.returncode != 0:
        if build_res.stderr or build_res.stdout:
            err = (build_res.stderr or build_res.stdout).strip()
            if err:
                print(f"WASM build failed for {script}: {err}", file=sys.stderr)
        else:
            print(f"WASM build failed for {script}.", file=sys.stderr)
        return None
    if not output_path.exists():
        print(f"WASM build produced no output.wasm for {script}", file=sys.stderr)
        return None
    return build_s


def prepare_wasm_binary(
    script: str,
    *,
    require_linked: bool,
    tty: bool,
    log: TextIO | None,
    keep_temp: bool,
) -> WasmBinary | None:
    temp_dir = tempfile.TemporaryDirectory(prefix="molt-wasm-bench-")
    if keep_temp:
        # Prevent TemporaryDirectory cleanup on GC so artifacts stick around.
        try:
            temp_dir._finalizer.detach()  # type: ignore[attr-defined]
        except Exception:
            pass
    output_path = Path(temp_dir.name) / "output.wasm"
    base_env = _base_env()
    base_env["MOLT_WASM_PATH"] = str(output_path)
    python_exe = _python_executable()

    env = base_env.copy()
    want_linked = _want_linked() or require_linked
    if want_linked:
        env["MOLT_WASM_LINK"] = "1"
    else:
        env.pop("MOLT_WASM_LINK", None)

    build_s = _build_wasm_output(python_exe, env, output_path, script, tty=tty, log=log)
    if build_s is None:
        if not keep_temp:
            temp_dir.cleanup()
        return None

    linked = (
        _link_wasm(env, output_path, require_linked=require_linked, log=log)
        if want_linked
        else None
    )
    linked_used = linked is not None
    if require_linked and not linked_used:
        print(
            f"WASM link required but unavailable for {script}.",
            file=sys.stderr,
        )
        if not keep_temp:
            temp_dir.cleanup()
        raise RuntimeError("linked wasm required")
    if want_linked and not linked_used:
        print(
            f"WASM link unavailable; falling back to non-linked build for {script}.",
            file=sys.stderr,
        )
        env = base_env.copy()
        env.pop("MOLT_WASM_LINK", None)
        build_s = _build_wasm_output(
            python_exe, env, output_path, script, tty=tty, log=log
        )
        if build_s is None:
            if not keep_temp:
                temp_dir.cleanup()
            return None

    wasm_path = linked if linked_used else output_path
    wasm_size = wasm_path.stat().st_size / 1024
    run_env = env.copy()
    if linked_used:
        run_env["MOLT_WASM_LINKED"] = "1"
        run_env["MOLT_WASM_LINKED_PATH"] = str(linked)
    return WasmBinary(run_env, temp_dir, build_s, wasm_size, linked_used)


def measure_wasm_run(
    run_env: dict[str, str], runner_cmd: list[str], *, log: TextIO | None
) -> float | None:
    start = time.perf_counter()
    run_res = _run_cmd(runner_cmd, run_env, capture=True, tty=False, log=log)
    end = time.perf_counter()
    if run_res.returncode != 0:
        err = run_res.stderr.strip() or run_res.stdout.strip()
        if err:
            print(f"WASM run failed: {err}", file=sys.stderr)
        return None
    return end - start


def collect_samples(
    wasm: WasmBinary,
    samples: int,
    warmup: int,
    runner_cmd: list[str],
    *,
    log: TextIO | None,
) -> tuple[list[float], bool]:
    for _ in range(warmup):
        if measure_wasm_run(wasm.run_env, runner_cmd, log=log) is None:
            return [], False
    runs = [measure_wasm_run(wasm.run_env, runner_cmd, log=log) for _ in range(samples)]
    valid = [t for t in runs if t is not None]
    return valid, bool(valid)


def _resolve_runner(runner: str, *, tty: bool, log: TextIO | None) -> list[str]:
    if runner == "node":
        return ["node", "run_wasm.js"]
    if runner != "wasmtime":
        raise ValueError(f"Unsupported wasm runner: {runner}")
    path = shutil.which("molt-wasm-host")
    if path:
        return [path]
    target = Path("target") / "release" / "molt-wasm-host"
    if not target.exists():
        res = _run_cmd(
            ["cargo", "build", "--release", "--package", "molt-wasm-host"],
            env=os.environ.copy(),
            capture=not tty,
            tty=tty,
            log=log,
        )
        if res.returncode != 0:
            err = (res.stderr or res.stdout).strip()
            raise RuntimeError(f"Failed to build molt-wasm-host: {err}")
    if not target.exists():
        raise RuntimeError("molt-wasm-host binary not found after build")
    return [str(target)]


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
    benchmarks: list[str],
    samples: int,
    warmup: int,
    super_run: bool,
    *,
    require_linked: bool,
    runner_cmd: list[str],
    tty: bool,
    log: TextIO | None,
    keep_temp: bool,
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
        wasm_binary = prepare_wasm_binary(
            script,
            require_linked=require_linked,
            tty=tty,
            log=log,
            keep_temp=keep_temp,
        )
        if wasm_binary is not None:
            try:
                wasm_samples, ok = collect_samples(
                    wasm_binary, samples, warmup, runner_cmd, log=log
                )
                wasm_time = statistics.mean(wasm_samples) if ok else 0.0
                wasm_size = wasm_binary.size_kb
                wasm_build = wasm_binary.build_s
                linked_used = wasm_binary.linked_used
            finally:
                if keep_temp:
                    print(
                        "Keeping wasm artifacts in "
                        f"{wasm_binary.temp_dir.name} (MOLT_WASM_KEEP=1)",
                        file=sys.stderr,
                    )
                else:
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
    _enable_line_buffering()
    parser = argparse.ArgumentParser(description="Run Molt WASM benchmark suite.")
    parser.add_argument("--json-out", type=Path, default=None)
    parser.add_argument("--samples", type=int, default=None)
    parser.add_argument(
        "--runner",
        choices=["node", "wasmtime"],
        default=os.environ.get("MOLT_WASM_RUNNER", "node"),
        help="Runner to execute wasm benchmarks (default: node).",
    )
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
        "--require-linked",
        action="store_true",
        help="Require linked wasm artifacts; abort if linking is unavailable.",
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
        "--log-file",
        type=Path,
        default=None,
        help="Append subprocess output to a log file (also honors MOLT_WASM_LOG).",
    )
    parser.add_argument(
        "--keep-artifacts",
        action="store_true",
        help="Keep per-benchmark wasm temp dirs (also honors MOLT_WASM_KEEP=1).",
    )
    args = parser.parse_args()

    if args.linked or args.require_linked:
        os.environ["MOLT_WASM_LINK"] = "1"
    if args.super and args.smoke:
        parser.error("--super cannot be combined with --smoke")
    if args.super and args.samples is not None:
        parser.error("--super cannot be combined with --samples")

    use_tty = args.tty or os.environ.get("MOLT_TTY") == "1"
    log_path = args.log_file
    if log_path is None:
        env_log = os.environ.get("MOLT_WASM_LOG")
        if env_log:
            log_path = Path(env_log)
    log_file = _open_log(log_path)
    if log_file is not None:
        _log_write(
            log_file,
            f"# Molt wasm bench log {dt.datetime.now(dt.timezone.utc).isoformat()}\n",
        )
    keep_temp = args.keep_artifacts or os.environ.get("MOLT_WASM_KEEP") == "1"
    if args.keep_artifacts:
        os.environ["MOLT_WASM_KEEP"] = "1"

    runner_cmd = _resolve_runner(args.runner, tty=use_tty, log=log_file)
    if not build_runtime_wasm(
        reloc=False, output=RUNTIME_WASM, tty=use_tty, log=log_file
    ):
        if log_file is not None:
            log_file.close()
        sys.exit(1)
    if _want_linked() and not build_runtime_wasm(
        reloc=True, output=RUNTIME_WASM_RELOC, tty=use_tty, log=log_file
    ):
        if args.require_linked:
            print(
                "Relocatable runtime build failed; linked output is required.",
                file=sys.stderr,
            )
            if log_file is not None:
                log_file.close()
            sys.exit(1)
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
    try:
        results = bench_results(
            benchmarks,
            samples,
            warmup,
            args.super,
            require_linked=args.require_linked,
            runner_cmd=runner_cmd,
            tty=use_tty,
            log=log_file,
            keep_temp=keep_temp,
        )
    except RuntimeError as exc:
        print(f"WASM bench aborted: {exc}", file=sys.stderr)
        if log_file is not None:
            log_file.close()
        sys.exit(1)

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
    if log_file is not None:
        log_file.close()


if __name__ == "__main__":
    main()
