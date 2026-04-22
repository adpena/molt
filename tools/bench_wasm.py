import argparse
import datetime as dt
import hashlib
import json
import os
import platform
import re
import shlex
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import TextIO

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))
from molt._wasm_runtime_exports import wasm_runtime_export_link_args

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
    "tests/benchmarks/bench_startup.py",
    "tests/benchmarks/bench_gc_pressure.py",
    "tests/benchmarks/bench_class_hierarchy.py",
    "tests/benchmarks/bench_set_ops.py",
    "tests/benchmarks/bench_exception_heavy.py",
    "tests/benchmarks/bench_dict_comprehension.py",
    "tests/benchmarks/bench_procedural_gen.py",
    "tests/benchmarks/bench_import_time.py",
]

SMOKE_BENCHMARKS = [
    "tests/benchmarks/bench_sum.py",
    "tests/benchmarks/bench_bytes_find.py",
]

WS_BENCHMARKS = [
    "tests/benchmarks/bench_ws_wait.py",
]

MOLT_ARGS_BY_BENCH = {
    "tests/benchmarks/bench_sum_list_hints.py": ["--type-hints", "trust"],
}


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


def _wasm_runtime_root() -> Path:
    env_root = os.environ.get("MOLT_WASM_RUNTIME_DIR")
    if env_root:
        return Path(env_root).expanduser()
    ext_root = os.environ.get("MOLT_EXT_ROOT")
    external_root = Path(ext_root).expanduser() if ext_root else None
    if external_root is not None and external_root.is_dir():
        return external_root / "wasm"
    return Path("wasm")


_RUNTIME_ROOT = _wasm_runtime_root()
RUNTIME_WASM = _RUNTIME_ROOT / "molt_runtime.wasm"
RUNTIME_WASM_RELOC = _RUNTIME_ROOT / "molt_runtime_reloc.wasm"
WASM_LD = shutil.which("wasm-ld")
_LINK_WARNED = False
_LINK_DISABLED = False
_LAST_BUILD_FAILURE_DETAIL: str | None = None
_NODE_BIN_CACHE: str | None = None
_MIN_NODE_MAJOR = 18
_RUNTIME_SOURCE_MTIME: float | None = None


@dataclass(frozen=True)
class WasmBinary:
    run_env: dict[str, str]
    temp_dir: tempfile.TemporaryDirectory
    build_s: float
    size_kb: float
    linked_used: bool
    import_count_total: int | None
    import_count_functions: int | None
    import_count_tables: int | None


@dataclass(frozen=True)
class _RunResult:
    returncode: int
    stdout: str = ""
    stderr: str = ""
    timed_out: bool = False


@dataclass(frozen=True)
class _SampleResult:
    elapsed_s: float | None
    returncode: int
    error: str | None
    error_class: str | None


def _is_valid_wasm(path: Path) -> bool:
    try:
        with path.open("rb") as fh:
            magic = fh.read(4)
    except OSError:
        return False
    return magic == b"\x00asm"


def _external_root() -> Path | None:
    configured = os.environ.get("MOLT_EXT_ROOT", "").strip()
    if configured:
        root = Path(configured).expanduser().resolve()
        if root.is_dir():
            return root
    return None


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def _cargo_target_root() -> Path:
    env_root = os.environ.get("CARGO_TARGET_DIR")
    if env_root:
        return Path(env_root).expanduser()
    external_root = _external_root()
    if external_root is not None:
        return external_root / "target"
    return Path("target")


def _runtime_source_mtime() -> float:
    global _RUNTIME_SOURCE_MTIME
    if _RUNTIME_SOURCE_MTIME is not None:
        return _RUNTIME_SOURCE_MTIME
    repo = _repo_root()
    runtime_root = repo / "runtime" / "molt-runtime"
    candidates: list[Path] = [
        repo / "Cargo.lock",
        runtime_root / "Cargo.toml",
    ]
    candidates.extend(runtime_root.glob("src/**/*.rs"))
    latest = 0.0
    for candidate in candidates:
        try:
            stat = candidate.stat()
        except OSError:
            continue
        if stat.st_mtime > latest:
            latest = stat.st_mtime
    _RUNTIME_SOURCE_MTIME = latest
    return latest


def _runtime_artifact_stale(path: Path) -> bool:
    try:
        artifact_mtime = path.stat().st_mtime
    except OSError:
        return True
    return _runtime_source_mtime() > artifact_mtime


def _read_wasm_varuint(data: bytes, offset: int) -> tuple[int, int]:
    result = 0
    shift = 0
    while True:
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading varuint")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            return result, offset
        shift += 7
        if shift > 35:
            raise ValueError("varuint too large")


def _read_wasm_string(data: bytes, offset: int) -> tuple[str, int]:
    length, offset = _read_wasm_varuint(data, offset)
    end = offset + length
    if end > len(data):
        raise ValueError("Unexpected EOF while reading string")
    return data[offset:end].decode("utf-8"), end


def _read_wasm_table_min(path: Path) -> int | None:
    try:
        data = path.read_bytes()
    except OSError:
        return None
    if len(data) < 8 or data[:4] != b"\0asm" or data[4:8] != b"\x01\x00\x00\x00":
        return None
    offset = 8
    try:
        while offset < len(data):
            section_id = data[offset]
            offset += 1
            size, offset = _read_wasm_varuint(data, offset)
            end = offset + size
            if end > len(data):
                raise ValueError("Unexpected EOF while reading section")
            if section_id != 2:
                offset = end
                continue
            payload = data[offset:end]
            offset = end
            cursor = 0
            count, cursor = _read_wasm_varuint(payload, cursor)
            for _ in range(count):
                module, cursor = _read_wasm_string(payload, cursor)
                name, cursor = _read_wasm_string(payload, cursor)
                if cursor >= len(payload):
                    raise ValueError("Unexpected EOF while reading import")
                kind = payload[cursor]
                cursor += 1
                if kind == 0:
                    _, cursor = _read_wasm_varuint(payload, cursor)
                elif kind == 1:
                    if cursor >= len(payload):
                        raise ValueError("Unexpected EOF while reading table type")
                    cursor += 1
                    flags, cursor = _read_wasm_varuint(payload, cursor)
                    minimum, cursor = _read_wasm_varuint(payload, cursor)
                    if flags & 0x1:
                        _, cursor = _read_wasm_varuint(payload, cursor)
                    if module == "env" and name == "__indirect_function_table":
                        return minimum
                elif kind == 2:
                    flags, cursor = _read_wasm_varuint(payload, cursor)
                    _, cursor = _read_wasm_varuint(payload, cursor)
                    if flags & 0x1:
                        _, cursor = _read_wasm_varuint(payload, cursor)
                elif kind == 3:
                    if cursor + 2 > len(payload):
                        raise ValueError("Unexpected EOF while reading global type")
                    cursor += 2
                else:
                    raise ValueError("Unknown import kind")
    except ValueError:
        return None
    return None


def _read_wasm_import_metrics(path: Path) -> dict[str, int] | None:
    try:
        data = path.read_bytes()
    except OSError:
        return None
    if len(data) < 8 or data[:4] != b"\0asm" or data[4:8] != b"\x01\x00\x00\x00":
        return None
    offset = 8
    total_imports = 0
    function_imports = 0
    table_imports = 0
    try:
        while offset < len(data):
            section_id = data[offset]
            offset += 1
            size, offset = _read_wasm_varuint(data, offset)
            end = offset + size
            if end > len(data):
                raise ValueError("Unexpected EOF while reading section")
            if section_id != 2:
                offset = end
                continue
            payload = data[offset:end]
            offset = end
            cursor = 0
            count, cursor = _read_wasm_varuint(payload, cursor)
            for _ in range(count):
                _, cursor = _read_wasm_string(payload, cursor)
                _, cursor = _read_wasm_string(payload, cursor)
                if cursor >= len(payload):
                    raise ValueError("Unexpected EOF while reading import")
                kind = payload[cursor]
                cursor += 1
                total_imports += 1
                if kind == 0:
                    function_imports += 1
                    _, cursor = _read_wasm_varuint(payload, cursor)
                elif kind == 1:
                    table_imports += 1
                    if cursor >= len(payload):
                        raise ValueError("Unexpected EOF while reading table type")
                    cursor += 1
                    flags, cursor = _read_wasm_varuint(payload, cursor)
                    _, cursor = _read_wasm_varuint(payload, cursor)
                    if flags & 0x1:
                        _, cursor = _read_wasm_varuint(payload, cursor)
                elif kind == 2:
                    flags, cursor = _read_wasm_varuint(payload, cursor)
                    _, cursor = _read_wasm_varuint(payload, cursor)
                    if flags & 0x1:
                        _, cursor = _read_wasm_varuint(payload, cursor)
                elif kind == 3:
                    if cursor + 2 > len(payload):
                        raise ValueError("Unexpected EOF while reading global type")
                    cursor += 2
                else:
                    raise ValueError("Unknown import kind")
            break
    except ValueError:
        return None
    return {
        "total": total_imports,
        "functions": function_imports,
        "tables": table_imports,
    }


def _parse_node_major(version_text: str) -> int | None:
    text = version_text.strip()
    if text.startswith("v"):
        text = text[1:]
    head = text.split(".", 1)[0]
    try:
        return int(head)
    except ValueError:
        return None


def _node_major_for_binary(path: str) -> int | None:
    try:
        res = subprocess.run(
            [path, "-p", "process.versions.node"],
            capture_output=True,
            text=True,
        )
    except OSError:
        return None
    if res.returncode != 0:
        return None
    return _parse_node_major(res.stdout)


def resolve_node_binary() -> str:
    global _NODE_BIN_CACHE
    if _NODE_BIN_CACHE is not None:
        return _NODE_BIN_CACHE

    requested = os.environ.get("MOLT_NODE_BIN", "").strip()
    if requested:
        major = _node_major_for_binary(requested)
        if major is None:
            raise RuntimeError(f"MOLT_NODE_BIN is not executable: {requested}")
        if major < _MIN_NODE_MAJOR:
            raise RuntimeError(
                f"MOLT_NODE_BIN must be Node >= {_MIN_NODE_MAJOR} (got {major}): {requested}"
            )
        _NODE_BIN_CACHE = requested
        return requested

    candidates: list[str] = []
    seen: set[str] = set()
    for candidate in (
        shutil.which("node"),
        "/opt/homebrew/bin/node",
        "/usr/local/bin/node",
    ):
        if not candidate:
            continue
        if candidate in seen:
            continue
        seen.add(candidate)
        candidates.append(candidate)

    best_path: str | None = None
    best_major = -1
    for candidate in candidates:
        major = _node_major_for_binary(candidate)
        if major is None:
            continue
        if major > best_major:
            best_path = candidate
            best_major = major

    if best_path is None:
        raise RuntimeError(
            "Node binary not found; install Node >= 18 or set MOLT_NODE_BIN."
        )
    if best_major < _MIN_NODE_MAJOR:
        raise RuntimeError(
            f"Detected Node {best_major} at {best_path}; Node >= {_MIN_NODE_MAJOR} required."
        )
    _NODE_BIN_CACHE = best_path
    return best_path


def _enable_line_buffering() -> None:
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if callable(reconfigure):
            reconfigure(line_buffering=True)


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
    timeout_s: float | None = None,
) -> _RunResult:
    if tty and not capture and os.name == "posix" and timeout_s is None:
        return _run_with_pty(cmd, env, log)
    if tty and timeout_s is not None and not capture:
        print(
            "TTY mode requested with timeout; using non-TTY subprocess mode.",
            file=sys.stderr,
        )
    timeout_term_grace_s = _parse_env_float(
        "MOLT_WASM_TIMEOUT_TERM_GRACE_SEC", default=1.0
    )

    if log is not None:
        _log_command(log, cmd)
    if timeout_s is not None:
        proc = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=env,
        )
        try:
            stdout, stderr = proc.communicate(timeout=timeout_s)
            res = subprocess.CompletedProcess(cmd, proc.returncode, stdout, stderr)
        except subprocess.TimeoutExpired as exc:
            out = (exc.stdout or "") if isinstance(exc.stdout, str) else ""
            err = (exc.stderr or "") if isinstance(exc.stderr, str) else ""
            try:
                if os.name == "posix":
                    proc.terminate()
                    try:
                        term_out, term_err = proc.communicate(
                            timeout=timeout_term_grace_s
                        )
                    except subprocess.TimeoutExpired as term_exc:
                        term_out = (
                            term_exc.stdout if isinstance(term_exc.stdout, str) else ""
                        )
                        term_err = (
                            term_exc.stderr if isinstance(term_exc.stderr, str) else ""
                        )
                        proc.kill()
                        kill_out, kill_err = proc.communicate()
                        term_out += kill_out or ""
                        term_err += kill_err or ""
                    out += term_out or ""
                    err += term_err or ""
                else:
                    proc.kill()
                    kill_out, kill_err = proc.communicate()
                    out += kill_out or ""
                    err += kill_err or ""
            except OSError:
                pass
            if log is not None:
                if out:
                    _log_write(log, out)
                if err:
                    _log_write(log, err)
                _log_write(
                    log,
                    f"# timeout after {timeout_s:.1f}s (command aborted)\n",
                )
            if not capture:
                if out:
                    sys.stdout.write(out)
                    sys.stdout.flush()
                if err:
                    sys.stderr.write(err)
                    sys.stderr.flush()
            return _RunResult(returncode=124, stdout=out, stderr=err, timed_out=True)
    else:
        res = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            env=env,
        )

    stdout = res.stdout or ""
    stderr = res.stderr or ""
    if log is not None:
        if stdout:
            _log_write(log, stdout)
        if stderr:
            _log_write(log, stderr)
    if not capture:
        if stdout:
            sys.stdout.write(stdout)
            sys.stdout.flush()
        if stderr:
            sys.stderr.write(stderr)
            sys.stderr.flush()
        return _RunResult(res.returncode, timed_out=False)
    return _RunResult(res.returncode, stdout, stderr, False)


def _summarize_error_text(
    text: str, *, max_lines: int = 8, max_chars: int = 1200
) -> str:
    trimmed = text.strip()
    if not trimmed:
        return ""
    lines = trimmed.splitlines()
    if len(lines) > max_lines:
        trimmed = "\n".join(lines[:max_lines]) + "\n... (truncated)"
    if len(trimmed) > max_chars:
        trimmed = trimmed[:max_chars].rstrip() + "... (truncated)"
    return trimmed


def _write_build_timeout_diag(
    *,
    output_path: Path,
    script: str,
    cmd: list[str],
    env: dict[str, str],
    timeout_s: float | None,
    attempt: str,
    result: _RunResult,
) -> None:
    diag_path = output_path.parent / f"build_timeout_diag_{attempt}.json"
    payload = {
        "script": script,
        "attempt": attempt,
        "timeout_s": timeout_s,
        "command": cmd,
        "env": {
            key: env.get(key)
            for key in (
                "PYTHONHASHSEED",
                "CARGO_TARGET_DIR",
                "MOLT_BUILD_STATE_DIR",
                "MOLT_BUILD_LOCK_TIMEOUT",
                "MOLT_FRONTEND_PHASE_TIMEOUT",
                "MOLT_MIDEND_FAIL_OPEN",
                "MOLT_MIDEND_MAX_ROUNDS",
                "MOLT_SCCP_MAX_ITERS",
                "MOLT_CSE_MAX_ITERS",
            )
        },
        "timed_out": result.timed_out,
        "returncode": result.returncode,
        "stdout_tail": (result.stdout or "")[-4000:],
        "stderr_tail": (result.stderr or "")[-4000:],
        "timestamp_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
    }
    try:
        diag_path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
        print(f"Wrote build timeout diagnostic: {diag_path}", file=sys.stderr)
    except OSError as exc:
        print(f"Failed to write build timeout diagnostic: {exc}", file=sys.stderr)


def _classify_failure(error: str, *, runner: str, returncode: int) -> str:
    text = error.lower()
    if runner == "node":
        if "zone allocation failed" in text:
            return "node_v8_zone_oom"
        if "fatal process out of memory: zone" in text:
            return "node_v8_zone_oom"
        if (
            "fatal process out of memory" in text
            or "javascript heap out of memory" in text
        ):
            return "node_v8_heap_oom"
        if "webassembly.instantiate" in text and "out of memory" in text:
            return "node_wasm_compile_oom"
        if "compiled wasm function limit" in text:
            return "node_wasm_function_limit"
    if returncode < 0:
        return "process_signal"
    if "out of memory" in text:
        return "runner_oom"
    if "trap" in text:
        return "wasm_trap"
    if "linked wasm required" in text:
        return "linked_artifact_missing"
    if "wasi proc_exit" in text:
        return "wasi_proc_exit"
    return "runner_error"


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
    # Keep wasm benchmark compiles deterministic and bounded when mid-end
    # optimization passes regress on specific stress benchmarks.
    env.setdefault("MOLT_SCCP_MAX_ITERS", "8")
    env.setdefault("MOLT_CSE_MAX_ITERS", "8")
    env.setdefault("MOLT_MIDEND_MAX_ROUNDS", "3")
    env.setdefault("MOLT_MIDEND_FAIL_OPEN", "1")
    env.setdefault(
        "MOLT_MIDEND_SKIP_MODULE_PREFIXES",
        "_collections_abc,abc,asyncio,collections",
    )
    env.setdefault("MOLT_BUILD_LOCK_TIMEOUT", "60")
    env.setdefault("MOLT_FRONTEND_PHASE_TIMEOUT", "60")
    external_root = _external_root()
    if external_root is not None:
        env.setdefault("CARGO_TARGET_DIR", str(external_root / "target"))
        env.setdefault("MOLT_WASM_RUNTIME_DIR", str(external_root / "wasm"))
    env.setdefault("MOLT_RUNTIME_WASM", str(RUNTIME_WASM))
    return env


def _open_log(log_path: Path | None) -> TextIO | None:
    if log_path is None:
        return None
    log_path.parent.mkdir(parents=True, exist_ok=True)
    return log_path.open("a", encoding="utf-8", buffering=1)


def _python_cmd() -> list[str]:
    """Return the command prefix for invoking Python in a uv-managed env.

    Hardcodes ``--python 3.12`` to match the project's target version
    policy and ensure ``packaging`` and other build-time dependencies
    are available even when the harness itself runs under a bare
    uv-managed interpreter.
    """
    uv = shutil.which("uv")
    if uv:
        return [uv, "run", "--python", "3.12", "python3"]
    exe = Path(sys.executable)
    if exe.exists():
        return [sys.executable]
    base = getattr(sys, "_base_executable", None)
    if base and Path(base).exists():
        return [base]
    return [sys.executable]


def _parse_env_int(name: str) -> int | None:
    value = os.environ.get(name)
    if value is None or not value.strip():
        return None
    try:
        parsed = int(value)
    except ValueError:
        raise RuntimeError(f"{name} must be an integer, got: {value!r}") from None
    if parsed <= 0:
        raise RuntimeError(f"{name} must be > 0, got: {parsed}")
    return parsed


def _parse_env_float(name: str, *, default: float | None = None) -> float | None:
    value = os.environ.get(name)
    if value is None or not value.strip():
        return default
    try:
        parsed = float(value)
    except ValueError:
        raise RuntimeError(f"{name} must be a float, got: {value!r}") from None
    if parsed <= 0:
        raise RuntimeError(f"{name} must be > 0, got: {parsed}")
    return parsed


def _append_rustflags(env: dict[str, str], flags: str) -> None:
    existing = env.get("RUSTFLAGS", "")
    joined = f"{existing} {flags}".strip()
    env["RUSTFLAGS"] = joined


def build_runtime_wasm(
    *, reloc: bool, output: Path, tty: bool, log: TextIO | None
) -> bool:
    runtime_build_timeout = _parse_env_float(
        "MOLT_WASM_RUNTIME_BUILD_TIMEOUT_SEC", default=300.0
    )
    env = os.environ.copy()
    target_root = _cargo_target_root()
    if os.environ.get("MOLT_WASM_RUNTIME_FORCE_LOCAL_TARGET") == "1":
        target_root = _repo_root() / "target"
    env["CARGO_TARGET_DIR"] = str(target_root)
    if reloc:
        base_flags = (
            "-C link-arg=--relocatable -C link-arg=--no-gc-sections"
            " -C relocation-model=pic"
        )
    else:
        base_flags = (
            "-C link-arg=--import-memory -C link-arg=--import-table"
            " -C link-arg=--growable-table -C link-arg=--export-dynamic"
            + wasm_runtime_export_link_args()
        )
    _append_rustflags(env, base_flags)
    build_cmd = [
        "cargo",
        "build",
        "--release",
        "--package",
        "molt-runtime",
        "--target",
        "wasm32-wasip1",
    ]
    res = _run_cmd(
        build_cmd,
        env=env,
        capture=not tty,
        tty=tty,
        log=log,
        timeout_s=runtime_build_timeout,
    )
    if res.timed_out:
        print(
            f"WASM runtime build timed out after {runtime_build_timeout:.1f}s.",
            file=sys.stderr,
        )
        return False
    if res.returncode != 0:
        if res.stderr or res.stdout:
            err = (res.stderr or res.stdout).strip()
            if err:
                print(f"WASM runtime build failed: {err}", file=sys.stderr)
        else:
            print("WASM runtime build failed.", file=sys.stderr)
        return False
    src = target_root / "wasm32-wasip1" / "release" / "molt_runtime.wasm"
    if not src.exists():
        print("WASM runtime build succeeded but artifact is missing.", file=sys.stderr)
        return False
    if not _is_valid_wasm(src):
        print(
            "WASM runtime artifact is invalid; forcing clean rebuild.",
            file=sys.stderr,
        )
        try:
            src.unlink(missing_ok=True)
        except OSError:
            pass
        clean_res = _run_cmd(
            [
                "cargo",
                "clean",
                "--target",
                "wasm32-wasip1",
            ],
            env=env,
            capture=not tty,
            tty=tty,
            log=log,
            timeout_s=runtime_build_timeout,
        )
        if clean_res.returncode != 0:
            err = (clean_res.stderr or clean_res.stdout).strip()
            if err:
                print(f"WASM runtime clean failed: {err}", file=sys.stderr)
            return False
        res = _run_cmd(
            build_cmd,
            env=env,
            capture=not tty,
            tty=tty,
            log=log,
            timeout_s=runtime_build_timeout,
        )
        if res.timed_out:
            print(
                f"WASM runtime rebuild timed out after {runtime_build_timeout:.1f}s.",
                file=sys.stderr,
            )
            return False
        if res.returncode != 0:
            err = (res.stderr or res.stdout).strip()
            if err:
                print(f"WASM runtime rebuild failed: {err}", file=sys.stderr)
            return False
        if not src.exists() or not _is_valid_wasm(src):
            # One more attempt: remove the artifact and rebuild from scratch.
            try:
                src.unlink(missing_ok=True)
            except OSError:
                pass
            res = _run_cmd(
                build_cmd,
                env=env,
                capture=not tty,
                tty=tty,
                log=log,
                timeout_s=runtime_build_timeout,
            )
            if res.timed_out:
                print(
                    "WASM runtime second rebuild timed out after "
                    f"{runtime_build_timeout:.1f}s.",
                    file=sys.stderr,
                )
                return False
            if res.returncode != 0:
                err = (res.stderr or res.stdout).strip()
                if err:
                    print(f"WASM runtime second rebuild failed: {err}", file=sys.stderr)
                return False
            if not src.exists() or not _is_valid_wasm(src):
                print(
                    "WASM runtime rebuild completed but artifact is still invalid.",
                    file=sys.stderr,
                )
                return False
    output.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, output)
    if not _is_valid_wasm(output):
        print(
            f"WASM runtime output is invalid after copy: {output}",
            file=sys.stderr,
        )
        return False
    return True


def _want_linked() -> bool:
    return os.environ.get("MOLT_WASM_LINK") == "1"


def _runtime_rebuild_policy() -> str:
    raw = os.environ.get("MOLT_WASM_RUNTIME_REBUILD", "auto").strip().lower()
    if raw in {"always", "1", "true", "yes"}:
        return "always"
    if raw in {"never", "0", "false", "no"}:
        return "never"
    return "auto"


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
    linked_wasm = input_path.with_name("output_linked.wasm")
    if linked_wasm.exists():
        linked_wasm.unlink()
    runtime_path = RUNTIME_WASM_RELOC if RUNTIME_WASM_RELOC.exists() else RUNTIME_WASM
    runtime_reloc = runtime_path == RUNTIME_WASM_RELOC
    if not _is_valid_wasm(runtime_path):
        print(
            f"Runtime wasm artifact is invalid; rebuilding: {runtime_path}",
            file=sys.stderr,
        )
        if not build_runtime_wasm(
            reloc=runtime_reloc, output=runtime_path, tty=False, log=log
        ):
            if require_linked:
                print("Linked output is required; aborting.", file=sys.stderr)
            return None
    res = _run_cmd(
        [
            *_python_cmd(),
            "tools/wasm_link.py",
            "--runtime",
            str(runtime_path),
            "--input",
            str(input_path),
            "--output",
            str(linked_wasm),
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
    if not linked_wasm.exists():
        print("WASM link produced no output artifact.", file=sys.stderr)
        return None
    if not _is_valid_wasm(linked_wasm):
        print("WASM link produced an invalid output artifact.", file=sys.stderr)
        try:
            linked_wasm.unlink(missing_ok=True)
        except OSError:
            pass
        _LINK_DISABLED = True
        return None
    return linked_wasm


def _build_wasm_output(
    python_cmd: list[str],
    env: dict[str, str],
    output_path: Path,
    script: str,
    *,
    tty: bool,
    log: TextIO | None,
) -> float | None:
    global _LAST_BUILD_FAILURE_DETAIL
    _LAST_BUILD_FAILURE_DETAIL = None
    build_timeout_s = _parse_env_float("MOLT_WASM_BUILD_TIMEOUT_SEC", default=90.0)
    extra_args = MOLT_ARGS_BY_BENCH.get(script, [])
    build_cmd = [
        *python_cmd,
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
    ]
    start = time.perf_counter()
    build_res = _run_cmd(
        build_cmd,
        env=env,
        capture=not tty,
        tty=tty,
        log=log,
        timeout_s=build_timeout_s,
    )
    build_s = time.perf_counter() - start
    if build_res.timed_out:
        print(
            f"WASM build timed out for {script} after {build_timeout_s:.1f}s; "
            "retrying once with stricter fail-open limits.",
            file=sys.stderr,
        )
        _write_build_timeout_diag(
            output_path=output_path,
            script=script,
            cmd=build_cmd,
            env=env,
            timeout_s=build_timeout_s,
            attempt="primary",
            result=build_res,
        )
        retry_env = env.copy()
        retry_env["MOLT_MIDEND_FAIL_OPEN"] = "1"
        retry_env["MOLT_MIDEND_MAX_ROUNDS"] = "1"
        retry_env["MOLT_SCCP_MAX_ITERS"] = "2"
        retry_env["MOLT_CSE_MAX_ITERS"] = "2"
        retry_env["MOLT_MIDEND_IDEMPOTENCE_CHECK"] = "0"
        retry_env["MOLT_BUILD_LOCK_TIMEOUT"] = "30"
        retry_env["MOLT_BUILD_STATE_DIR"] = str(
            output_path.parent / ".molt_state_wasm_retry"
        )
        start = time.perf_counter()
        build_res = _run_cmd(
            build_cmd,
            env=retry_env,
            capture=not tty,
            tty=tty,
            log=log,
            timeout_s=build_timeout_s,
        )
        build_s = time.perf_counter() - start
        if build_res.timed_out:
            _write_build_timeout_diag(
                output_path=output_path,
                script=script,
                cmd=build_cmd,
                env=retry_env,
                timeout_s=build_timeout_s,
                attempt="retry",
                result=build_res,
            )
            print(
                f"WASM build timed out again for {script}; aborting benchmark compile.",
                file=sys.stderr,
            )
            _LAST_BUILD_FAILURE_DETAIL = (
                f"build_timeout_after_retry timeout_s={build_timeout_s:.1f}"
            )
            return None

    lock_wait_timeout = "Timed out waiting for build lock"
    if build_res.returncode != 0 and lock_wait_timeout in (
        (build_res.stderr or "") + (build_res.stdout or "")
    ):
        print(
            f"WASM build hit build-lock timeout for {script}; retrying with isolated build state.",
            file=sys.stderr,
        )
        retry_env = env.copy()
        retry_env["MOLT_BUILD_LOCK_TIMEOUT"] = "30"
        retry_env["MOLT_BUILD_STATE_DIR"] = str(
            output_path.parent / ".molt_state_wasm_lock_retry"
        )
        start = time.perf_counter()
        build_res = _run_cmd(
            build_cmd,
            env=retry_env,
            capture=not tty,
            tty=tty,
            log=log,
            timeout_s=build_timeout_s,
        )
        build_s = time.perf_counter() - start

    if build_res.returncode != 0:
        if build_res.stderr or build_res.stdout:
            err = (build_res.stderr or build_res.stdout).strip()
            if err:
                print(f"WASM build failed for {script}: {err}", file=sys.stderr)
                _LAST_BUILD_FAILURE_DETAIL = _summarize_error_text(err)
        else:
            print(f"WASM build failed for {script}.", file=sys.stderr)
            _LAST_BUILD_FAILURE_DETAIL = "wasm_build_failed"
        return None
    if not output_path.exists():
        print(f"WASM build produced no output.wasm for {script}", file=sys.stderr)
        _LAST_BUILD_FAILURE_DETAIL = "wasm_output_missing"
        return None
    if not _is_valid_wasm(output_path):
        print(
            f"WASM build produced invalid output.wasm for {script}; retrying once.",
            file=sys.stderr,
        )
        try:
            output_path.unlink(missing_ok=True)
        except OSError:
            pass
        start = time.perf_counter()
        build_res = _run_cmd(
            build_cmd,
            env=env,
            capture=not tty,
            tty=tty,
            log=log,
            timeout_s=build_timeout_s,
        )
        build_s = time.perf_counter() - start
        if build_res.timed_out:
            _write_build_timeout_diag(
                output_path=output_path,
                script=script,
                cmd=build_cmd,
                env=env,
                timeout_s=build_timeout_s,
                attempt="integrity_retry",
                result=build_res,
            )
            print(
                f"WASM build retry timed out for {script}; aborting benchmark compile.",
                file=sys.stderr,
            )
            _LAST_BUILD_FAILURE_DETAIL = (
                f"build_timeout_integrity_retry timeout_s={build_timeout_s:.1f}"
            )
            return None
        if build_res.returncode != 0:
            err = (build_res.stderr or build_res.stdout).strip()
            if err:
                print(f"WASM build retry failed for {script}: {err}", file=sys.stderr)
                _LAST_BUILD_FAILURE_DETAIL = _summarize_error_text(err)
            else:
                print(f"WASM build retry failed for {script}.", file=sys.stderr)
                _LAST_BUILD_FAILURE_DETAIL = "wasm_build_retry_failed"
            return None
        if not output_path.exists() or not _is_valid_wasm(output_path):
            print(
                f"WASM build produced invalid output.wasm for {script} after retry.",
                file=sys.stderr,
            )
            _LAST_BUILD_FAILURE_DETAIL = "wasm_output_invalid_after_retry"
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
    _prune_backend_daemons()
    global _LAST_BUILD_FAILURE_DETAIL
    _LAST_BUILD_FAILURE_DETAIL = None
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
    python_cmd = _python_cmd()

    env = base_env.copy()
    want_linked = _want_linked() or require_linked
    if want_linked:
        env["MOLT_WASM_LINK"] = "1"
    else:
        env.pop("MOLT_WASM_LINK", None)

    if want_linked and "MOLT_WASM_TABLE_BASE" not in env:
        table_probe = (
            RUNTIME_WASM_RELOC if RUNTIME_WASM_RELOC.exists() else RUNTIME_WASM
        )
        table_base = _read_wasm_table_min(table_probe)
        if table_base is not None:
            env["MOLT_WASM_TABLE_BASE"] = str(table_base)

    build_s = _build_wasm_output(python_cmd, env, output_path, script, tty=tty, log=log)
    if build_s is None:
        print(
            "Backend build failed; pruning stale daemons and retrying...",
            file=sys.stderr,
        )
        _prune_backend_daemons()
        time.sleep(1)
        build_s = _build_wasm_output(
            python_cmd, env, output_path, script, tty=tty, log=log
        )
    if build_s is None:
        if _LAST_BUILD_FAILURE_DETAIL is None:
            _LAST_BUILD_FAILURE_DETAIL = "wasm_build_failed"
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
        _LAST_BUILD_FAILURE_DETAIL = "linked_wasm_required_unavailable"
        raise RuntimeError("linked wasm required")
    if want_linked and not linked_used:
        print(
            f"WASM link unavailable; falling back to non-linked build for {script}.",
            file=sys.stderr,
        )
        env = base_env.copy()
        env.pop("MOLT_WASM_LINK", None)
        env["MOLT_WASM_PREFER_LINKED"] = "0"
        build_s = _build_wasm_output(
            python_cmd,
            env,
            output_path,
            script,
            tty=tty,
            log=log,
        )
        if build_s is None:
            if _LAST_BUILD_FAILURE_DETAIL is None:
                _LAST_BUILD_FAILURE_DETAIL = "wasm_build_failed_after_link_fallback"
            if not keep_temp:
                temp_dir.cleanup()
            return None
        stale_linked = output_path.with_name("output_linked.wasm")
        if stale_linked.exists():
            stale_linked.unlink()

    if linked_used:
        assert linked is not None
        wasm_path = linked
    else:
        wasm_path = output_path
    wasm_size = wasm_path.stat().st_size / 1024
    import_metrics = _read_wasm_import_metrics(wasm_path)
    run_env = env.copy()
    # Runtime executions should not inherit host-Python import environment knobs.
    # Keep run-time behavior aligned with standalone compiled binaries.
    run_env.pop("PYTHONPATH", None)
    run_env.pop("PYTHONHASHSEED", None)
    run_env.pop("PYTHONUNBUFFERED", None)
    # Avoid noisy Node warnings in parity and benchmark lanes.
    run_env.setdefault("NODE_NO_WARNINGS", "1")
    if linked_used:
        run_env["MOLT_WASM_PATH"] = str(linked)
        run_env["MOLT_WASM_LINKED"] = "1"
        run_env["MOLT_WASM_LINKED_PATH"] = str(linked)
    else:
        run_env["MOLT_WASM_PREFER_LINKED"] = "0"
    return WasmBinary(
        run_env=run_env,
        temp_dir=temp_dir,
        build_s=build_s,
        size_kb=wasm_size,
        linked_used=linked_used,
        import_count_total=(
            import_metrics["total"] if import_metrics is not None else None
        ),
        import_count_functions=(
            import_metrics["functions"] if import_metrics is not None else None
        ),
        import_count_tables=(
            import_metrics["tables"] if import_metrics is not None else None
        ),
    )


def measure_wasm_run(
    run_env: dict[str, str],
    runner_cmd: list[str],
    *,
    runner_name: str,
    log: TextIO | None,
) -> _SampleResult:
    start = time.perf_counter()
    run_res = _run_cmd(runner_cmd, run_env, capture=True, tty=False, log=log)
    end = time.perf_counter()
    if run_res.returncode != 0:
        err = (run_res.stderr or run_res.stdout).strip()
        summarized = _summarize_error_text(err)
        error_class = _classify_failure(
            summarized,
            runner=runner_name,
            returncode=run_res.returncode,
        )
        if err:
            print(
                f"WASM run failed ({runner_name}, {error_class}): {summarized}",
                file=sys.stderr,
            )
        return _SampleResult(
            elapsed_s=None,
            returncode=run_res.returncode,
            error=summarized or None,
            error_class=error_class,
        )
    return _SampleResult(
        elapsed_s=end - start,
        returncode=run_res.returncode,
        error=None,
        error_class=None,
    )


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _extract_profile_json(text: str) -> dict[str, object] | None:
    profile: dict[str, object] | None = None
    prefix = "molt_profile_json "
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith(prefix):
            continue
        payload = stripped[len(prefix) :].strip()
        if not payload:
            continue
        try:
            parsed = json.loads(payload)
        except json.JSONDecodeError:
            continue
        if isinstance(parsed, dict):
            profile = parsed
    return profile


def _load_hostfed_call_bundle(path: Path) -> list[dict[str, object]]:
    raw = json.loads(path.read_text(encoding="utf-8"))
    calls = raw if isinstance(raw, list) else raw.get("calls")
    if not isinstance(calls, list):
        raise ValueError(f"Host-fed call bundle must contain a calls array: {path}")
    normalized: list[dict[str, object]] = []
    for index, call in enumerate(calls):
        if not isinstance(call, dict):
            raise ValueError(
                f"Host-fed call bundle entry {index} must be an object: {path}"
            )
        export = call.get("export")
        if not isinstance(export, str) or not export.strip():
            raise ValueError(
                f"Host-fed call bundle entry {index} is missing a valid export name: {path}"
            )
        args = call.get("args", [])
        if not isinstance(args, list):
            raise ValueError(
                f"Host-fed call bundle entry {index} must use an args array: {path}"
            )
        normalized.append({"export": export, "args": args})
    return normalized


def _run_hostfed_call_bundle(
    *,
    label: str,
    app_wasm: Path,
    runtime_wasm: Path,
    calls_path: Path,
    runner_cmd: list[str],
    runner_name: str,
    log: TextIO | None,
    timeout_s: float | None = None,
) -> dict[str, object]:
    calls = _load_hostfed_call_bundle(calls_path)
    env = _base_env()
    env["MOLT_WASM_DIRECT_LINK"] = "1"
    env["MOLT_WASM_PREFER_LINKED"] = "0"
    env["MOLT_RUNTIME_WASM"] = str(runtime_wasm)
    env["MOLT_WASM_PATH"] = str(app_wasm)
    env["MOLT_WASM_EXPORT_CALLS_JSON"] = str(calls_path)
    env.setdefault("NODE_NO_WARNINGS", "1")
    start = time.perf_counter()
    res = _run_cmd(
        [*runner_cmd, str(app_wasm)],
        env=env,
        capture=True,
        tty=False,
        log=log,
        timeout_s=timeout_s,
    )
    wall_s = time.perf_counter() - start
    payload: dict[str, object] = {
        "label": label,
        "runner": runner_name,
        "app_wasm": str(app_wasm),
        "app_wasm_sha256": _sha256(app_wasm),
        "runtime_wasm": str(runtime_wasm),
        "runtime_wasm_sha256": _sha256(runtime_wasm),
        "calls_path": str(calls_path),
        "calls_path_sha256": _sha256(calls_path),
        "calls": calls,
        "runner_wall_time_s": wall_s,
        "returncode": res.returncode,
        "ok": False,
        "timed_out": res.timed_out,
    }
    if timeout_s is not None:
        payload["timeout_s"] = timeout_s
    profile = _extract_profile_json(res.stderr or "")
    if profile is not None:
        payload["profile"] = profile
    if res.returncode != 0:
        err = (res.stderr or res.stdout).strip()
        if res.timed_out:
            summarized = (
                f"runner timed out after {timeout_s:.1f}s"
                if timeout_s is not None
                else "runner timed out"
            )
            error_class = "runner_timeout"
        else:
            summarized = _summarize_error_text(err)
            error_class = _classify_failure(
                summarized,
                runner=runner_name,
                returncode=res.returncode,
            )
        payload.update(
            {
                "error": summarized or None,
                "error_class": error_class,
            }
        )
        return payload
    try:
        parsed = json.loads(res.stdout or "[]")
    except json.JSONDecodeError as exc:
        payload.update(
            {
                "error": f"runner did not emit valid JSON: {exc}",
                "error_class": "runner_json_error",
            }
        )
        return payload
    if not isinstance(parsed, list):
        payload.update(
            {
                "error": "runner JSON must be a list of call results",
                "error_class": "runner_json_error",
            }
        )
        return payload

    result_entries: list[dict[str, object]] = []
    total_call_ms = 0
    for index, item in enumerate(parsed):
        if not isinstance(item, dict):
            payload.update(
                {
                    "error": f"runner result entry {index} must be an object",
                    "error_class": "runner_json_error",
                }
            )
            return payload
        duration_ms = item.get("duration_ms")
        if isinstance(duration_ms, (int, float)):
            total_call_ms += int(duration_ms)
        result_entries.append(dict(item))

    payload.update(
        {
            "ok": True,
            "call_count": len(result_entries),
            "runner_results": result_entries,
            "call_duration_ms_total": total_call_ms,
            "first_call_duration_ms": (
                result_entries[0].get("duration_ms") if result_entries else None
            ),
            "second_call_duration_ms": (
                result_entries[1].get("duration_ms")
                if len(result_entries) > 1
                else None
            ),
            "exports": [str(call.get("export")) for call in calls],
        }
    )
    return payload


def collect_samples(
    wasm: WasmBinary,
    samples: int,
    warmup: int,
    runner_cmd: list[str],
    runner_name: str,
    *,
    log: TextIO | None,
) -> tuple[list[float], bool, _SampleResult | None]:
    for _ in range(warmup):
        result = measure_wasm_run(
            wasm.run_env,
            runner_cmd,
            runner_name=runner_name,
            log=log,
        )
        if result.elapsed_s is None:
            return [], False, result
    timings: list[float] = []
    first_failure: _SampleResult | None = None
    for _ in range(samples):
        result = measure_wasm_run(
            wasm.run_env,
            runner_cmd,
            runner_name=runner_name,
            log=log,
        )
        if result.elapsed_s is None:
            if first_failure is None:
                first_failure = result
            continue
        timings.append(result.elapsed_s)
    return timings, bool(timings), first_failure


def _resolve_runner(
    runner: str,
    *,
    tty: bool,
    log: TextIO | None,
    node_max_old_space_mb: int | None,
) -> list[str]:
    if runner == "node":
        cmd = [resolve_node_binary()]
        # Keep Node wasm execution deterministic and avoid post-run V8 tiering/OOM
        # incidents seen on large linked modules.
        cmd.extend(
            [
                "--no-warnings",
                "--no-wasm-tier-up",
                "--no-wasm-dynamic-tiering",
                "--wasm-num-compilation-tasks=1",
            ]
        )
        if node_max_old_space_mb is not None:
            cmd.append(f"--max-old-space-size={node_max_old_space_mb}")
        extra_options = os.environ.get("MOLT_WASM_NODE_OPTIONS")
        if extra_options:
            cmd.extend(shlex.split(extra_options))
        cmd.append("wasm/run_wasm.js")
        return cmd
    if runner != "wasmtime":
        raise ValueError(f"Unsupported wasm runner: {runner}")
    host_override = os.environ.get("MOLT_WASM_HOST_PATH")
    if host_override:
        host_path = Path(host_override).expanduser()
        if not host_path.exists():
            raise RuntimeError(f"MOLT_WASM_HOST_PATH does not exist: {host_path}")
        return [str(host_path)]
    target = _cargo_target_root() / "release" / "molt-wasm-host"
    if not target.exists():
        build_env = os.environ.copy()
        build_env.setdefault("CARGO_TARGET_DIR", str(_cargo_target_root()))
        res = _run_cmd(
            ["cargo", "build", "--release", "--package", "molt-wasm-host"],
            env=build_env,
            capture=not tty,
            tty=tty,
            log=log,
        )
        if res.returncode != 0:
            err = (res.stderr or res.stdout).strip()
            raise RuntimeError(f"Failed to build molt-wasm-host: {err}")
    if target.exists():
        return [str(target)]
    path = shutil.which("molt-wasm-host")
    if path:
        return [path]
    raise RuntimeError("molt-wasm-host binary not found after build")


def _node_has_websocket(log: TextIO | None) -> bool:
    try:
        node_bin = resolve_node_binary()
    except RuntimeError:
        return False
    cmd = [
        node_bin,
        "-e",
        (
            "let ws=globalThis.WebSocket; "
            "if (!ws) { try { ws=require('undici').WebSocket; } catch (e) {} } "
            "process.exit(ws ? 0 : 1);"
        ),
    ]
    try:
        res = _run_cmd(cmd, env=os.environ.copy(), capture=True, tty=False, log=log)
    except OSError:
        return False
    return res.returncode == 0


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
    runner_name: str,
    control_runner_cmd: list[str] | None,
    control_runner_name: str | None,
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
        failed_sample: _SampleResult | None = None
        control_sample: _SampleResult | None = None
        try:
            wasm_binary = prepare_wasm_binary(
                script,
                require_linked=require_linked,
                tty=tty,
                log=log,
                keep_temp=keep_temp,
            )
        except RuntimeError as exc:
            print(f"WASM benchmark setup failed for {script}: {exc}", file=sys.stderr)
            wasm_binary = None
        if wasm_binary is not None:
            try:
                wasm_samples, ok, failed_sample = collect_samples(
                    wasm_binary,
                    samples,
                    warmup,
                    runner_cmd,
                    runner_name,
                    log=log,
                )
                wasm_time = statistics.mean(wasm_samples) if ok else 0.0
                wasm_size = wasm_binary.size_kb
                wasm_build = wasm_binary.build_s
                linked_used = wasm_binary.linked_used
                if (
                    not ok
                    and control_runner_cmd is not None
                    and control_runner_name is not None
                ):
                    control_sample = measure_wasm_run(
                        wasm_binary.run_env,
                        control_runner_cmd,
                        runner_name=control_runner_name,
                        log=log,
                    )
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
        if wasm_binary is not None:
            if wasm_binary.import_count_total is not None:
                data[name]["molt_wasm_import_count"] = wasm_binary.import_count_total
            if wasm_binary.import_count_functions is not None:
                data[name]["molt_wasm_function_import_count"] = (
                    wasm_binary.import_count_functions
                )
            if wasm_binary.import_count_tables is not None:
                data[name]["molt_wasm_table_import_count"] = (
                    wasm_binary.import_count_tables
                )
            if (
                wasm_binary.import_count_functions is not None
                and wasm_binary.size_kb > 0
            ):
                data[name]["molt_wasm_function_imports_per_kb"] = round(
                    wasm_binary.import_count_functions / wasm_binary.size_kb,
                    6,
                )
        if failed_sample is not None:
            data[name]["molt_wasm_failure_class"] = failed_sample.error_class
            data[name]["molt_wasm_failure_returncode"] = failed_sample.returncode
            data[name]["molt_wasm_failure"] = failed_sample.error
        elif not ok and _LAST_BUILD_FAILURE_DETAIL:
            data[name]["molt_wasm_failure_class"] = "build_setup_error"
            data[name]["molt_wasm_failure_returncode"] = -1
            data[name]["molt_wasm_failure"] = _LAST_BUILD_FAILURE_DETAIL
        if control_runner_name is not None and control_sample is not None:
            data[name]["molt_wasm_control_runner"] = control_runner_name
            data[name]["molt_wasm_control_ok"] = control_sample.elapsed_s is not None
            if control_sample.elapsed_s is not None:
                data[name]["molt_wasm_control_time_s"] = control_sample.elapsed_s
            else:
                data[name]["molt_wasm_control_failure_class"] = (
                    control_sample.error_class
                )
                data[name]["molt_wasm_control_failure_returncode"] = (
                    control_sample.returncode
                )
                data[name]["molt_wasm_control_failure"] = control_sample.error
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
        "--bench",
        action="append",
        default=None,
        help=(
            "Run only selected benchmark(s). Accepts full path, "
            "tests/benchmarks/<name>.py, or stem (repeatable)."
        ),
    )
    parser.add_argument(
        "--runner",
        choices=["node", "wasmtime"],
        default=os.environ.get("MOLT_WASM_RUNNER", "node"),
        help="Runner to execute wasm benchmarks (default: node).",
    )
    parser.add_argument(
        "--control-runner",
        choices=["none", "node", "wasmtime"],
        default=os.environ.get("MOLT_WASM_CONTROL_RUNNER", "none"),
        help=(
            "Optional control runner for failed benches. "
            "Use 'wasmtime' to classify node-specific failures."
        ),
    )
    parser.add_argument(
        "--node-max-old-space-mb",
        type=int,
        default=None,
        help=(
            "Pass --max-old-space-size=<MB> to node runner "
            "(also honors MOLT_WASM_NODE_MAX_OLD_SPACE_MB)."
        ),
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
        "--allow-unlinked",
        action="store_true",
        help=("Allow unlinked wasm artifacts when linking is unavailable."),
    )
    parser.add_argument(
        "--ws",
        action="store_true",
        help="Include websocket wait benchmark (also honors MOLT_WASM_BENCH_WS=1).",
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
    env_node_max_old_space_mb = _parse_env_int("MOLT_WASM_NODE_MAX_OLD_SPACE_MB")
    if args.node_max_old_space_mb is None:
        args.node_max_old_space_mb = env_node_max_old_space_mb
    elif args.node_max_old_space_mb <= 0:
        parser.error("--node-max-old-space-mb must be > 0")
    if args.control_runner == args.runner:
        parser.error("--control-runner must differ from --runner (or be 'none')")
    if args.require_linked and args.allow_unlinked:
        parser.error("--allow-unlinked cannot be combined with --require-linked")
    if args.require_linked:
        args.linked = True

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

    runner_cmd = _resolve_runner(
        args.runner,
        tty=use_tty,
        log=log_file,
        node_max_old_space_mb=args.node_max_old_space_mb,
    )
    control_runner_name: str | None = None
    control_runner_cmd: list[str] | None = None
    if args.control_runner != "none":
        control_runner_name = args.control_runner
        control_runner_cmd = _resolve_runner(
            control_runner_name,
            tty=use_tty,
            log=log_file,
            node_max_old_space_mb=args.node_max_old_space_mb,
        )
    runtime_policy = _runtime_rebuild_policy()
    shared_runtime_invalid = not _is_valid_wasm(RUNTIME_WASM)
    shared_runtime_stale = (
        runtime_policy == "auto"
        and not shared_runtime_invalid
        and _runtime_artifact_stale(RUNTIME_WASM)
    )
    need_shared_runtime = (
        runtime_policy == "always" or shared_runtime_invalid or shared_runtime_stale
    )
    if need_shared_runtime:
        if runtime_policy == "never":
            print(
                f"Runtime rebuild disabled but runtime artifact is missing/invalid: {RUNTIME_WASM}",
                file=sys.stderr,
            )
            if log_file is not None:
                log_file.close()
            sys.exit(1)
        if shared_runtime_stale:
            msg = f"Runtime sources changed; rebuilding runtime wasm: {RUNTIME_WASM}"
            print(msg, file=sys.stderr)
            if log_file is not None:
                _log_write(log_file, f"# {msg}\n")
        if not build_runtime_wasm(
            reloc=False, output=RUNTIME_WASM, tty=use_tty, log=log_file
        ):
            if log_file is not None:
                log_file.close()
            sys.exit(1)
    elif log_file is not None:
        _log_write(log_file, f"# reusing cached runtime wasm {RUNTIME_WASM}\n")

    if _want_linked():
        reloc_runtime_invalid = not _is_valid_wasm(RUNTIME_WASM_RELOC)
        reloc_runtime_stale = (
            runtime_policy == "auto"
            and not reloc_runtime_invalid
            and _runtime_artifact_stale(RUNTIME_WASM_RELOC)
        )
        need_reloc_runtime = (
            runtime_policy == "always" or reloc_runtime_invalid or reloc_runtime_stale
        )
        if need_reloc_runtime:
            if runtime_policy == "never":
                if args.require_linked:
                    print(
                        "Relocatable runtime rebuild disabled and artifact missing/invalid; "
                        "linked output is required.",
                        file=sys.stderr,
                    )
                    if log_file is not None:
                        log_file.close()
                    sys.exit(1)
            if reloc_runtime_stale:
                msg = (
                    "Runtime sources changed; rebuilding reloc runtime wasm: "
                    f"{RUNTIME_WASM_RELOC}"
                )
                print(msg, file=sys.stderr)
                if log_file is not None:
                    _log_write(log_file, f"# {msg}\n")
            if not build_runtime_wasm(
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
        elif log_file is not None:
            _log_write(
                log_file, f"# reusing cached reloc runtime wasm {RUNTIME_WASM_RELOC}\n"
            )

    benchmarks = list(SMOKE_BENCHMARKS) if args.smoke else list(BENCHMARKS)
    if args.bench:
        by_path = {bench: bench for bench in benchmarks}
        by_name = {Path(bench).name: bench for bench in benchmarks}
        by_stem = {Path(bench).stem: bench for bench in benchmarks}
        selected: list[str] = []
        missing: list[str] = []
        for raw in args.bench:
            token = raw.strip()
            candidate = by_path.get(token)
            if candidate is None:
                candidate = by_path.get(f"tests/benchmarks/{token}")
            if candidate is None and not token.endswith(".py"):
                candidate = by_name.get(f"{token}.py")
            if candidate is None:
                candidate = by_name.get(token)
            if candidate is None and token.endswith(".py"):
                candidate = by_stem.get(Path(token).stem)
            if candidate is None:
                candidate = by_stem.get(token)
            if candidate is None:
                missing.append(raw)
                continue
            if candidate not in selected:
                selected.append(candidate)
        if missing:
            parser.error(f"Unknown benchmark selection(s): {', '.join(missing)}")
        benchmarks = selected
    include_ws = args.ws or os.environ.get("MOLT_WASM_BENCH_WS") == "1"
    if include_ws:
        if args.runner == "node" and not _node_has_websocket(log_file):
            print(
                "Skipping websocket bench: node runner has no WebSocket support.",
                file=sys.stderr,
            )
        else:
            for bench in WS_BENCHMARKS:
                if bench not in benchmarks:
                    benchmarks.append(bench)
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
            runner_name=args.runner,
            control_runner_cmd=control_runner_cmd,
            control_runner_name=control_runner_name,
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
        "runner": args.runner,
        "control_runner": control_runner_name,
        "node_max_old_space_mb": args.node_max_old_space_mb,
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
