import concurrent.futures
import contextlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from collections.abc import Sequence
from functools import lru_cache
from pathlib import Path


def _resolve_python_exe(python_exe: str) -> str:
    if not python_exe:
        return sys.executable
    if os.sep in python_exe or Path(python_exe).is_absolute():
        candidate = Path(python_exe)
        if candidate.exists():
            return python_exe
        base_exe = getattr(sys, "_base_executable", "")
        if base_exe and Path(base_exe).exists():
            return base_exe
    return python_exe


def _collect_env_overrides(file_path: str) -> dict[str, str]:
    overrides: dict[str, str] = {}
    try:
        text = Path(file_path).read_text()
    except OSError:
        return overrides
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("# MOLT_ENV:"):
            continue
        payload = stripped[len("# MOLT_ENV:") :].strip()
        for token in payload.split():
            if "=" not in token:
                continue
            key, value = token.split("=", 1)
            overrides[key] = value
    return overrides


def _collect_meta(file_path: str) -> dict[str, list[str]]:
    meta: dict[str, list[str]] = {}
    try:
        text = Path(file_path).read_text()
    except OSError:
        return meta
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("# MOLT_META:"):
            continue
        payload = stripped[len("# MOLT_META:") :].strip()
        for token in payload.split():
            if "=" not in token:
                continue
            key, value = token.split("=", 1)
            values = [v for v in value.split(",") if v]
            if not values:
                values = [""]
            meta.setdefault(key, []).extend(values)
    return meta


def _parse_version(value: str) -> tuple[int, int] | None:
    parts = value.strip().split(".")
    if len(parts) < 2:
        return None
    try:
        major = int(parts[0])
        minor = int(parts[1])
    except ValueError:
        return None
    return major, minor


@lru_cache(maxsize=None)
def _python_exe_version(python_exe: str) -> tuple[int, int] | None:
    try:
        result = subprocess.run(
            [python_exe, "-c", "import sys; print(sys.version_info[:2])"],
            capture_output=True,
            text=True,
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    raw = result.stdout.strip().strip("()")
    if not raw:
        return None
    parts = raw.split(",")
    if len(parts) < 2:
        return None
    try:
        return int(parts[0]), int(parts[1])
    except ValueError:
        return None


def _host_platform_tags() -> set[str]:
    tags: set[str] = set()
    if os.name == "posix":
        tags.update({"posix", "unix"})
    if os.name == "nt":
        tags.add("windows")
    if sys.platform.startswith("linux"):
        tags.add("linux")
    elif sys.platform == "darwin":
        tags.add("macos")
    elif sys.platform.startswith("freebsd"):
        tags.add("freebsd")
    wasm_raw = os.environ.get("MOLT_TARGET", "").strip().lower()
    wasm_flag = os.environ.get("MOLT_WASM", "").strip().lower()
    if wasm_raw == "wasm" or wasm_flag in {"1", "true", "yes", "on"}:
        tags.add("wasm")
    return tags


def _normalize_output(text: str, normalize: set[str]) -> str:
    if "all" in normalize or "newlines" in normalize:
        text = text.replace("\r\n", "\n")
    if "all" in normalize or "paths" in normalize:
        text = text.replace("\\", "/")
    return text


def _truthy_flag(values: list[str]) -> bool:
    for value in values:
        if value.strip().lower() in {"1", "true", "yes", "on"}:
            return True
    return False


def _should_skip(
    meta: dict[str, list[str]],
    *,
    python_version: tuple[int, int] | None,
    host_tags: set[str],
) -> tuple[bool, str | None]:
    if _truthy_flag(meta.get("skip", [])):
        return True, "metadata skip"

    platforms = {
        p.lower() for p in meta.get("platforms", []) + meta.get("platform", [])
    }
    if platforms and host_tags.isdisjoint(platforms):
        return True, f"platform {sorted(platforms)}"

    wasm_flags = [v.lower() for v in meta.get("wasm", [])]
    if wasm_flags:
        wants_wasm = any(v in {"1", "true", "yes", "on", "only"} for v in wasm_flags)
        forbids_wasm = any(v in {"0", "false", "no"} for v in wasm_flags)
        if "wasm" in host_tags and forbids_wasm:
            return True, "wasm disabled"
        if "wasm" not in host_tags and wants_wasm:
            return True, "wasm only"

    allowed_versions = meta.get("py", []) + meta.get("python", [])
    if python_version is not None and allowed_versions:
        allowed = {_parse_version(v) for v in allowed_versions}
        allowed.discard(None)
        if allowed and python_version not in allowed:
            return True, f"python {python_version[0]}.{python_version[1]}"

    if python_version is not None:
        min_versions = [_parse_version(v) for v in meta.get("min_py", [])]
        max_versions = [_parse_version(v) for v in meta.get("max_py", [])]
        min_versions = [v for v in min_versions if v is not None]
        max_versions = [v for v in max_versions if v is not None]
        if min_versions:
            min_version = min_versions[0]
            if python_version < min_version:
                return True, f"min_py {min_version[0]}.{min_version[1]}"
        if max_versions:
            max_version = max_versions[0]
            if python_version > max_version:
                return True, f"max_py {max_version[0]}.{max_version[1]}"

    return False, None


def _diff_timeout() -> float | None:
    raw = os.environ.get("MOLT_DIFF_TIMEOUT", "")
    if not raw:
        return None
    try:
        val = float(raw)
    except ValueError:
        return None
    return val if val > 0 else None


def _diff_root() -> Path:
    raw = os.environ.get("MOLT_DIFF_ROOT", "").strip()
    if raw:
        root = Path(raw).expanduser()
    else:
        external_root = Path("/Volumes/APDataStore/Molt")
        if external_root.exists():
            root = external_root
        else:
            root = Path("logs") / "molt_diff"
    root.mkdir(parents=True, exist_ok=True)
    return root


def _diff_tmp_root() -> Path:
    raw = os.environ.get("MOLT_DIFF_TMPDIR", "").strip()
    if raw:
        root = Path(raw).expanduser()
    else:
        diff_root = _diff_root()
        if diff_root.as_posix().startswith("/Volumes/APDataStore/Molt"):
            root = diff_root / "tmp"
        else:
            root = diff_root
    root.mkdir(parents=True, exist_ok=True)
    return root


def _diff_keep_artifacts() -> bool:
    raw = os.environ.get("MOLT_DIFF_KEEP", "").strip().lower()
    return raw in {"1", "true", "yes", "on"}


def _diff_trusted_default() -> bool:
    raw = os.environ.get("MOLT_DIFF_TRUSTED", "").strip().lower()
    if raw:
        return raw in {"1", "true", "yes", "on"}
    raw = os.environ.get("MOLT_DEV_TRUSTED", "").strip().lower()
    if not raw:
        return True
    return raw not in {"0", "false", "no", "off"}


def _diff_measure_rss() -> bool:
    raw = os.environ.get("MOLT_DIFF_MEASURE_RSS", "").strip().lower()
    return raw in {"1", "true", "yes", "on"}


def _diff_glob() -> str:
    raw = os.environ.get("MOLT_DIFF_GLOB", "").strip()
    return raw or "*.py"


def _diff_run_id() -> str:
    raw = os.environ.get("MOLT_DIFF_RUN_ID", "").strip()
    if raw:
        return raw
    ts = time.strftime("%Y%m%d_%H%M%S", time.gmtime())
    return f"{ts}_{os.getpid()}"


def _diff_warm_cache() -> bool:
    raw = os.environ.get("MOLT_DIFF_WARM_CACHE", "").strip().lower()
    return raw in {"1", "true", "yes", "on"}


def _diff_retry_oom_default() -> bool:
    raw = os.environ.get("MOLT_DIFF_RETRY_OOM", "").strip().lower()
    if raw:
        return raw in {"1", "true", "yes", "on"}
    return True


def _parse_float_env(name: str) -> float | None:
    raw = os.environ.get(name, "").strip()
    if not raw:
        return None
    try:
        return float(raw)
    except ValueError:
        return None


def _memory_limit_bytes() -> int | None:
    gb = _parse_float_env("MOLT_DIFF_RLIMIT_GB")
    mb = _parse_float_env("MOLT_DIFF_RLIMIT_MB")
    if gb is not None:
        if gb <= 0:
            return None
        return int(gb * 1024 * 1024 * 1024)
    if mb is not None:
        if mb <= 0:
            return None
        return int(mb * 1024 * 1024)
    # Default to 10 GB when unset; disable by setting MOLT_DIFF_RLIMIT_GB=0.
    return 10 * 1024 * 1024 * 1024


_MEM_LIMIT_APPLIED = False


def _apply_memory_limit() -> None:
    global _MEM_LIMIT_APPLIED
    if _MEM_LIMIT_APPLIED:
        return
    limit = _memory_limit_bytes()
    if limit is None:
        _MEM_LIMIT_APPLIED = True
        return
    try:
        import resource  # type: ignore
    except Exception:
        _MEM_LIMIT_APPLIED = True
        return
    for name in ("RLIMIT_AS", "RLIMIT_DATA", "RLIMIT_RSS"):
        res = getattr(resource, name, None)
        if res is None:
            continue
        try:
            soft, hard = resource.getrlimit(res)
            new_soft = min(soft, limit) if soft != resource.RLIM_INFINITY else limit
            new_hard = min(hard, limit) if hard != resource.RLIM_INFINITY else limit
            resource.setrlimit(res, (new_soft, new_hard))
        except Exception:
            continue
    _MEM_LIMIT_APPLIED = True


def _available_memory_bytes() -> int | None:
    override = _parse_float_env("MOLT_DIFF_MEM_AVAILABLE_GB")
    if override is not None and override > 0:
        return int(override * 1024 * 1024 * 1024)
    system = sys.platform
    if system.startswith("linux"):
        try:
            text = Path("/proc/meminfo").read_text()
        except OSError:
            text = ""
        for line in text.splitlines():
            if line.startswith("MemAvailable:"):
                parts = line.split()
                if len(parts) >= 2 and parts[1].isdigit():
                    return int(parts[1]) * 1024
        for line in text.splitlines():
            if line.startswith("MemTotal:"):
                parts = line.split()
                if len(parts) >= 2 and parts[1].isdigit():
                    return int(parts[1]) * 1024
    if system == "darwin":
        try:
            page_size = os.sysconf("SC_PAGE_SIZE")
            pages = os.sysconf("SC_PHYS_PAGES")
            return int(page_size * pages * 0.6)
        except (OSError, ValueError):
            return None
    if system.startswith("win"):
        try:
            import ctypes

            class MemoryStatus(ctypes.Structure):
                _fields_ = [
                    ("length", ctypes.c_uint32),
                    ("memory_load", ctypes.c_uint32),
                    ("total_phys", ctypes.c_uint64),
                    ("avail_phys", ctypes.c_uint64),
                    ("total_page_file", ctypes.c_uint64),
                    ("avail_page_file", ctypes.c_uint64),
                    ("total_virtual", ctypes.c_uint64),
                    ("avail_virtual", ctypes.c_uint64),
                    ("avail_extended_virtual", ctypes.c_uint64),
                ]

            status = MemoryStatus()
            status.length = ctypes.sizeof(MemoryStatus)
            ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status))
            return int(status.avail_phys)
        except Exception:
            return None
    return None


def _default_jobs() -> int:
    count = os.cpu_count() or 1
    per_job_gb = _parse_float_env("MOLT_DIFF_MEM_PER_JOB_GB") or 2.0
    available = _available_memory_bytes()
    if available is not None:
        mem_jobs = int(available / (per_job_gb * 1024 * 1024 * 1024))
        count = min(count, max(1, mem_jobs))
    max_jobs = os.environ.get("MOLT_DIFF_MAX_JOBS", "").strip()
    if max_jobs.isdigit():
        count = min(count, max(1, int(max_jobs)))
    return max(1, count)


def _collect_test_files(target: Path) -> list[Path]:
    if target.is_dir():
        pattern = _diff_glob()
        return sorted(target.glob(pattern))
    return [target]


def _collect_test_files_multi(targets: Sequence[Path]) -> list[Path]:
    seen: set[Path] = set()
    files: list[Path] = []
    for target in targets:
        for path in _collect_test_files(target):
            if path in seen:
                continue
            seen.add(path)
            files.append(path)
    return files


def _order_test_files(files: list[Path], jobs: int) -> list[Path]:
    mode = os.environ.get("MOLT_DIFF_ORDER", "auto").strip().lower()
    if mode not in {"auto", "name", "size-asc", "size-desc"}:
        mode = "auto"
    if mode == "auto":
        mode = "size-desc" if jobs > 1 else "name"
    if mode == "name":
        return files

    def size_key(path: Path) -> int:
        try:
            return path.stat().st_size
        except OSError:
            return 0

    reverse = mode == "size-desc"
    return sorted(files, key=size_key, reverse=reverse)


def _log_path_for_test(log_dir: Path, file_path: str) -> Path:
    path = Path(file_path)
    try:
        rel = path.relative_to(Path.cwd())
    except ValueError:
        rel = path
    safe = "__".join(rel.parts)
    return log_dir / f"{safe}.log"


def _write_test_log(log_dir: Path, file_path: str, stdout: str, stderr: str) -> Path:
    log_path = _log_path_for_test(log_dir, file_path)
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open("w") as handle:
        if stdout:
            handle.write("STDOUT:\n")
            handle.write(stdout)
            if not stdout.endswith("\n"):
                handle.write("\n")
        if stderr:
            if stdout:
                handle.write("\n")
            handle.write("STDERR:\n")
            handle.write(stderr)
            if not stderr.endswith("\n"):
                handle.write("\n")
    return log_path


def _emit_line(
    line: str,
    log_handle: io.TextIOBase | None = None,
    *,
    echo: bool = True,
) -> None:
    if echo:
        print(line)
    if log_handle is not None:
        log_handle.write(line + "\n")
        log_handle.flush()


@contextlib.contextmanager
def _open_log_file(path: Path | None):
    if path is None:
        yield None
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    handle = path.open("a", buffering=1)
    try:
        yield handle
    finally:
        handle.close()


def _diff_worker(file_path: str, python_exe: str) -> dict[str, str]:
    buffer_out = io.StringIO()
    buffer_err = io.StringIO()
    with contextlib.redirect_stdout(buffer_out), contextlib.redirect_stderr(buffer_err):
        status = diff_test(file_path, python_exe)
    return {
        "path": file_path,
        "status": status,
        "stdout": buffer_out.getvalue(),
        "stderr": buffer_err.getvalue(),
    }


class _TeeStream(io.TextIOBase):
    def __init__(self, *handles: io.TextIOBase) -> None:
        self._handles = handles

    def write(self, s: str) -> int:
        for handle in self._handles:
            handle.write(s)
        return len(s)

    def flush(self) -> None:
        for handle in self._handles:
            handle.flush()


def _diff_run_single(file_path: str, python_exe: str) -> dict[str, str]:
    buffer_out = io.StringIO()
    buffer_err = io.StringIO()
    out_stream = _TeeStream(sys.stdout, buffer_out)
    err_stream = _TeeStream(sys.stderr, buffer_err)
    with contextlib.redirect_stdout(out_stream), contextlib.redirect_stderr(err_stream):
        status = diff_test(file_path, python_exe)
    return {
        "path": file_path,
        "status": status,
        "stdout": buffer_out.getvalue(),
        "stderr": buffer_err.getvalue(),
    }


def _append_aggregate_log(
    handle: io.TextIOBase,
    file_path: str,
    status: str,
    stdout: str,
    stderr: str,
) -> None:
    handle.write(f"=== [{status.upper()}] {file_path} ===\n")
    if stdout:
        handle.write("STDOUT:\n")
        handle.write(stdout)
        if not stdout.endswith("\n"):
            handle.write("\n")
    if stderr:
        if stdout:
            handle.write("\n")
        handle.write("STDERR:\n")
        handle.write(stderr)
        if not stderr.endswith("\n"):
            handle.write("\n")
    handle.write("\n")
    handle.flush()


def _time_tool() -> str | None:
    path = Path("/usr/bin/time")
    return str(path) if path.exists() else None


def _parse_time_metrics(path: Path) -> dict[str, int]:
    metrics: dict[str, int] = {}
    try:
        text = path.read_text()
    except OSError:
        return metrics
    for line in text.splitlines():
        raw = line.strip()
        if not raw:
            continue
        value: int | None = None
        if ":" in raw:
            maybe = raw.split(":", 1)[1].strip().split()[0]
            if maybe.isdigit():
                value = int(maybe)
        else:
            parts = raw.split()
            if parts and parts[0].isdigit():
                value = int(parts[0])
        if value is None:
            continue
        if "maximum resident set size" in raw or "Maximum resident set size" in raw:
            if sys.platform == "darwin":
                value = max(1, value // 1024)
            metrics["max_rss"] = value
        elif "peak memory footprint" in raw:
            if sys.platform == "darwin":
                value = max(1, value // 1024)
            metrics["peak_footprint"] = value
    return metrics


def _run_with_optional_time(
    cmd: list[str],
    *,
    env: dict[str, str],
    timeout: float | None,
    time_path: Path | None,
):
    run_cmd = cmd
    if time_path is not None:
        time_bin = _time_tool()
        if time_bin is not None:
            if sys.platform == "darwin":
                run_cmd = [time_bin, "-l", "-o", str(time_path), *cmd]
            else:
                run_cmd = [time_bin, "-v", "-o", str(time_path), *cmd]
    return subprocess.run(
        run_cmd,
        env=env,
        capture_output=True,
        text=True,
        errors="surrogateescape",
        timeout=timeout,
    )


def _record_rss_metrics(
    file_path: str,
    *,
    build_metrics: dict[str, int] | None,
    run_metrics: dict[str, int] | None,
    build_rc: int | None,
    run_rc: int | None,
    status: str,
) -> None:
    if not _diff_measure_rss():
        return
    run_id = os.environ.get("MOLT_DIFF_RUN_ID", "").strip() or None
    payload = {
        "run_id": run_id,
        "timestamp": time.time(),
        "file": file_path,
        "status": status,
        "build_rc": build_rc,
        "run_rc": run_rc,
        "build": build_metrics or {},
        "run": run_metrics or {},
    }
    summary_path = _diff_root() / "rss_metrics.jsonl"
    try:
        with summary_path.open("a") as fh:
            fh.write(json.dumps(payload, sort_keys=True) + "\n")
    except OSError:
        return


def run_cpython(file_path, python_exe=sys.executable):
    python_exe = _resolve_python_exe(python_exe)
    _apply_memory_limit()
    env = os.environ.copy()
    env["PYTHONHASHSEED"] = "0"
    env.update(_collect_env_overrides(file_path))
    bootstrap = "import runpy, sys; runpy.run_path(sys.argv[1], run_name='__main__')"
    timeout = _diff_timeout()
    try:
        result = subprocess.run(
            [python_exe, "-c", bootstrap, file_path],
            capture_output=True,
            text=True,
            errors="surrogateescape",
            env=env,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return "", f"Timeout after {timeout}s", 124
    return result.stdout, result.stderr, result.returncode


def run_molt(file_path):
    return _run_molt(file_path, build_only=False)


def run_molt_build_only(file_path: str) -> tuple[str, str, int]:
    return _run_molt(file_path, build_only=True)


def _run_molt(file_path: str, *, build_only: bool) -> tuple[str | None, str, int]:
    _apply_memory_limit()
    output_root = Path(tempfile.mkdtemp(prefix="molt_diff_", dir=_diff_tmp_root()))
    cache_root = output_root / "cache"
    tmp_root = output_root / "tmp"
    cache_root.mkdir(parents=True, exist_ok=True)
    tmp_root.mkdir(parents=True, exist_ok=True)
    output_binary = output_root / f"{Path(file_path).stem}_molt"
    metrics_dir = output_root / "metrics" if _diff_measure_rss() else None
    if metrics_dir is not None:
        metrics_dir.mkdir(parents=True, exist_ok=True)
    build_time_path = metrics_dir / "build.time" if metrics_dir is not None else None
    run_time_path = metrics_dir / "run.time" if metrics_dir is not None else None
    build_metrics: dict[str, int] | None = None
    run_metrics: dict[str, int] | None = None

    # Build
    env = os.environ.copy()
    env["PYTHONPATH"] = "src"
    env["PYTHONHASHSEED"] = "0"
    shared_cache = env.get("MOLT_CACHE")
    if shared_cache:
        Path(shared_cache).mkdir(parents=True, exist_ok=True)
    else:
        env["MOLT_CACHE"] = str(cache_root)
    env["TMPDIR"] = str(tmp_root)
    env["TEMP"] = str(tmp_root)
    env["TMP"] = str(tmp_root)
    if "MOLT_TRUSTED" not in env and _diff_trusted_default():
        env["MOLT_TRUSTED"] = "1"
    env.update(_collect_env_overrides(file_path))
    env.setdefault("MOLT_SYS_EXECUTABLE", _resolve_python_exe(sys.executable))
    ver = sys.version_info
    env.setdefault(
        "MOLT_SYS_VERSION_INFO",
        f"{ver.major},{ver.minor},{ver.micro},{ver.releaselevel},{ver.serial}",
    )
    env.setdefault("MOLT_SYS_VERSION", sys.version)
    timeout = _diff_timeout()
    try:
        build_cmd = [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            file_path,
            "--out-dir",
            str(output_root),
            "--output",
            str(output_binary),
        ]
        codec = env.get("MOLT_CODEC")
        if codec:
            build_cmd.extend(["--codec", codec])
        try:
            build_res = _run_with_optional_time(
                build_cmd,
                env=env,
                timeout=timeout,
                time_path=build_time_path,
            )
        except subprocess.TimeoutExpired:
            build_metrics = (
                _parse_time_metrics(build_time_path)
                if build_time_path is not None
                else None
            )
            _record_rss_metrics(
                file_path,
                build_metrics=build_metrics,
                run_metrics=None,
                build_rc=124,
                run_rc=None,
                status="build_timeout",
            )
            return None, f"Timeout after {timeout}s", 124
        if build_time_path is not None:
            build_metrics = _parse_time_metrics(build_time_path)
        if build_res.returncode != 0:
            _record_rss_metrics(
                file_path,
                build_metrics=build_metrics,
                run_metrics=None,
                build_rc=build_res.returncode,
                run_rc=None,
                status="build_failed",
            )
            return None, build_res.stderr, build_res.returncode

        if build_only:
            _record_rss_metrics(
                file_path,
                build_metrics=build_metrics,
                run_metrics=None,
                build_rc=build_res.returncode,
                run_rc=None,
                status="build_only_ok",
            )
            return "", "", 0

        # Run
        try:
            run_res = _run_with_optional_time(
                [str(output_binary)],
                env=env,
                timeout=timeout,
                time_path=run_time_path,
            )
        except subprocess.TimeoutExpired:
            run_metrics = (
                _parse_time_metrics(run_time_path)
                if run_time_path is not None
                else None
            )
            _record_rss_metrics(
                file_path,
                build_metrics=build_metrics,
                run_metrics=run_metrics,
                build_rc=build_res.returncode,
                run_rc=124,
                status="run_timeout",
            )
            return "", f"Timeout after {timeout}s", 124
        if run_time_path is not None:
            run_metrics = _parse_time_metrics(run_time_path)
        _record_rss_metrics(
            file_path,
            build_metrics=build_metrics,
            run_metrics=run_metrics,
            build_rc=build_res.returncode,
            run_rc=run_res.returncode,
            status="ok",
        )
        return run_res.stdout, run_res.stderr, run_res.returncode
    finally:
        if not _diff_keep_artifacts():
            shutil.rmtree(output_root, ignore_errors=True)


def _is_oom_returncode(code: int | None) -> bool:
    if code is None:
        return False
    if code in {137, 9}:
        return True
    if code < 0 and abs(code) in {9, 137}:
        return True
    return False


def _is_oom_error(stderr: str) -> bool:
    needle = stderr.lower()
    return any(token in needle for token in ("oom", "out of memory", "std::bad_alloc"))


def _should_retry_oom(code: int | None, stderr: str) -> bool:
    return _is_oom_returncode(code) or _is_oom_error(stderr)


def _aggregate_rss_metrics(run_id: str) -> dict[str, object]:
    if not _diff_measure_rss():
        return {}
    summary_path = _diff_root() / "rss_metrics.jsonl"
    if not summary_path.exists():
        return {}
    entries: list[dict[str, object]] = []
    try:
        for line in summary_path.read_text().splitlines():
            if not line.strip():
                continue
            try:
                payload = json.loads(line)
            except json.JSONDecodeError:
                continue
            if run_id and payload.get("run_id") != run_id:
                continue
            entries.append(payload)
    except OSError:
        return {}
    if not entries:
        return {}

    def metric_max(key: str, field: str) -> int | None:
        values: list[int] = []
        for item in entries:
            block = item.get(key) or {}
            if isinstance(block, dict):
                value = block.get(field)
                if isinstance(value, int):
                    values.append(value)
        return max(values) if values else None

    max_build_rss = metric_max("build", "max_rss")
    max_run_rss = metric_max("run", "max_rss")
    max_peak = metric_max("run", "peak_footprint")
    return {
        "entries": len(entries),
        "max_build_rss_kb": max_build_rss,
        "max_run_rss_kb": max_run_rss,
        "max_run_peak_footprint_kb": max_peak,
    }


def _top_rss_entries(run_id: str, limit: int) -> list[dict[str, object]]:
    if not _diff_measure_rss():
        return []
    summary_path = _diff_root() / "rss_metrics.jsonl"
    if not summary_path.exists():
        return []
    entries: list[dict[str, object]] = []
    try:
        for line in summary_path.read_text().splitlines():
            if not line.strip():
                continue
            try:
                payload = json.loads(line)
            except json.JSONDecodeError:
                continue
            if run_id and payload.get("run_id") != run_id:
                continue
            entries.append(payload)
    except OSError:
        return []

    def metric(entry: dict[str, object]) -> int:
        block = entry.get("run") or {}
        if isinstance(block, dict):
            value = block.get("max_rss")
            if isinstance(value, int):
                return value
        return 0

    ranked = sorted(entries, key=metric, reverse=True)
    return ranked[: max(0, limit)]


def _print_rss_top(run_id: str, limit: int) -> None:
    if limit <= 0:
        return
    entries = _top_rss_entries(run_id, limit)
    if not entries:
        return
    print(f"Top {len(entries)} RSS offenders (run phase):")
    for entry in entries:
        file_path = entry.get("file", "<unknown>")
        status = entry.get("status", "")
        run_block = entry.get("run") or {}
        build_block = entry.get("build") or {}
        run_rss = run_block.get("max_rss") if isinstance(run_block, dict) else None
        build_rss = (
            build_block.get("max_rss") if isinstance(build_block, dict) else None
        )

        def fmt(value: object) -> str:
            return f"{value} KB" if isinstance(value, int) and value > 0 else "-"

        print(
            f"- {file_path} | run={fmt(run_rss)} build={fmt(build_rss)} status={status}"
        )


def diff_test(file_path, python_exe=sys.executable):
    meta = _collect_meta(file_path)
    python_version = _python_exe_version(python_exe)
    host_tags = _host_platform_tags()
    skip, reason = _should_skip(
        meta,
        python_version=python_version,
        host_tags=host_tags,
    )
    if skip:
        note = f" ({reason})" if reason else ""
        print(f"[SKIP] {file_path}{note}")
        return "skip"

    normalize = {v.lower() for v in meta.get("normalize", [])}
    stderr_mode = (meta.get("stderr", ["ignore"])[0]).lower()

    print(f"Testing {file_path} against {python_exe}...")
    cp_out, cp_err, cp_ret = run_cpython(file_path, python_exe)
    if _should_retry_oom(cp_ret, cp_err):
        print(f"[OOM] {file_path} (cpython)")
        return "oom"
    if cp_ret != 0 and (
        "msgpack is required for parse_msgpack fallback" in cp_err
        or "cbor2 is required for parse_cbor fallback" in cp_err
    ):
        print(f"[SKIP] {file_path} (missing msgpack/cbor2 in CPython env)")
        return "skip"
    molt_out, molt_err, molt_ret = run_molt(file_path)
    if _should_retry_oom(molt_ret, molt_err):
        print(f"[OOM] {file_path}")
        return "oom"

    cp_out = _normalize_output(cp_out, normalize)
    cp_err = _normalize_output(cp_err, normalize)
    if molt_out is not None:
        molt_out = _normalize_output(molt_out, normalize)
    molt_err = _normalize_output(molt_err, normalize)

    if molt_out is None:

        def is_compile_error(err: str) -> bool:
            return any(
                tag in err for tag in ("SyntaxError", "IndentationError", "TabError")
            )

        if cp_ret != 0 and is_compile_error(cp_err) and is_compile_error(molt_err):
            print(f"[PASS] {file_path}")
            return "pass"

        print(f"[FAIL] Molt failed to build {file_path}")
        print(molt_err)
        return "fail"

    stderr_match = stderr_mode in {"match", "exact"}
    stderr_ok = True
    if stderr_match:
        stderr_ok = cp_err == molt_err

    if cp_out == molt_out and cp_ret == molt_ret and stderr_ok:
        print(f"[PASS] {file_path}")
        return "pass"
    else:
        print(f"[FAIL] {file_path} mismatch")
        print(f"  CPython stdout: {cp_out!r}")
        print(f"  Molt    stdout: {molt_out!r}")
        print(f"  CPython return: {cp_ret} stderr: {cp_err!r}")
        print(f"  Molt    return: {molt_ret} stderr: {molt_err!r}")
        return "fail"


def run_diff(
    target: Path | Sequence[Path],
    python_exe: str,
    *,
    jobs: int | None = None,
    log_dir: Path | None = None,
    log_file: Path | None = None,
    log_aggregate: Path | None = None,
    live: bool = False,
    fail_fast: bool = False,
    failures_output: Path | None = None,
    warm_cache: bool = False,
    retry_oom: bool = False,
) -> dict:
    results: list[tuple[str, str]] = []
    if isinstance(target, Path):
        test_files = _collect_test_files(target)
    else:
        test_files = _collect_test_files_multi(target)
    if jobs is None:
        jobs = _default_jobs() if len(test_files) > 1 else 1
    run_id = _diff_run_id()
    os.environ["MOLT_DIFF_RUN_ID"] = run_id
    test_files = _order_test_files(test_files, jobs)
    if warm_cache:
        shared_cache = os.environ.get("MOLT_CACHE")
        if not shared_cache:
            shared_cache = str(_diff_root() / "molt_cache")
            os.environ["MOLT_CACHE"] = shared_cache
        for file_path in test_files:
            _out, err, rc = run_molt_build_only(str(file_path))
            if rc != 0:
                print(f"[WARM-CACHE FAIL] {file_path}: {err.strip()}")
    if jobs <= 1:
        with _open_log_file(log_file) as log_handle:
            with _open_log_file(log_aggregate) as aggregate_handle:
                for file_path in test_files:
                    payload = _diff_run_single(str(file_path), python_exe)
                    path = payload["path"]
                    status = payload["status"]
                    results.append((path, status))
                    if log_handle is not None:
                        _emit_line(
                            f"[{status.upper()}] {path}",
                            log_handle,
                            echo=False,
                        )
                    if aggregate_handle is not None:
                        _append_aggregate_log(
                            aggregate_handle,
                            path,
                            status,
                            payload["stdout"],
                            payload["stderr"],
                        )
    else:
        if log_dir is not None:
            try:
                log_dir.mkdir(parents=True, exist_ok=True)
            except OSError as exc:
                print(f"Warning: failed to create log dir {log_dir}: {exc}")
                log_dir = None
        if not live:
            live = True
        outputs: dict[str, dict[str, str]] = {}
        with _open_log_file(log_file) as log_handle:
            with _open_log_file(log_aggregate) as aggregate_handle:
                with concurrent.futures.ProcessPoolExecutor(
                    max_workers=jobs
                ) as executor:
                    futures = {
                        executor.submit(_diff_worker, str(file_path), python_exe): str(
                            file_path
                        )
                        for file_path in test_files
                    }
                    for future in concurrent.futures.as_completed(futures):
                        result = future.result()
                        path = result["path"]
                        status = result["status"]
                        outputs[path] = result
                        results.append((path, status))
                        log_path = None
                        if log_dir is not None:
                            log_path = _write_test_log(
                                log_dir, path, result["stdout"], result["stderr"]
                            )
                        _emit_line(
                            f"[{status.upper()}] {path}",
                            log_handle,
                            echo=live,
                        )
                        if status == "fail" and log_path is not None:
                            _emit_line(f"  log: {log_path}", log_handle, echo=live)
                        if aggregate_handle is not None:
                            _append_aggregate_log(
                                aggregate_handle,
                                path,
                                status,
                                result["stdout"],
                                result["stderr"],
                            )
                        if fail_fast and status == "fail":
                            for pending in futures:
                                if pending is not future:
                                    pending.cancel()
                            break
        if not live and log_dir is None:
            for file_path in test_files:
                payload = outputs.get(str(file_path))
                if payload is None:
                    continue
                if payload["stdout"]:
                    print(payload["stdout"], end="")
                if payload["stderr"]:
                    print(payload["stderr"], end="", file=sys.stderr)
    status_by_path = {path: status for path, status in results}
    if jobs > 1 and retry_oom:
        oom_paths = [p for p, s in status_by_path.items() if s == "oom"]
        if oom_paths:
            _emit_line(
                f"[RETRY-OOM] Retrying {len(oom_paths)} test(s) with --jobs 1",
                None,
                echo=True,
            )
        for path in oom_paths:
            retry_payload = _diff_run_single(path, python_exe)
            status_by_path[path] = retry_payload["status"]
            outputs[path] = retry_payload
    discovered = len(status_by_path)
    failed_files = [
        path for path, status in status_by_path.items() if status in {"fail", "oom"}
    ]
    skipped_files = [
        path for path, status in status_by_path.items() if status == "skip"
    ]
    failed = len(failed_files)
    passed = len([None for status in status_by_path.values() if status == "pass"])
    skipped = len(skipped_files)
    oom = len([None for status in status_by_path.values() if status == "oom"])
    total = passed + failed
    try:
        limit = int(os.environ.get("MOLT_DIFF_RSS_TOP", "5"))
    except ValueError:
        limit = 5
    rss_top = [
        {
            "file": entry.get("file"),
            "status": entry.get("status"),
            "run_max_rss_kb": (entry.get("run") or {}).get("max_rss")
            if isinstance(entry.get("run"), dict)
            else None,
            "build_max_rss_kb": (entry.get("build") or {}).get("max_rss")
            if isinstance(entry.get("build"), dict)
            else None,
        }
        for entry in _top_rss_entries(run_id, limit if _diff_measure_rss() else 0)
    ]
    summary = {
        "discovered": discovered,
        "total": total,
        "passed": passed,
        "failed": failed,
        "oom": oom,
        "skipped": skipped,
        "failed_files": failed_files,
        "skipped_files": skipped_files,
        "python_exe": python_exe,
        "jobs": jobs,
        "run_id": run_id,
        "config": {
            "measure_rss": _diff_measure_rss(),
            "mem_limit_bytes": _memory_limit_bytes(),
            "mem_per_job_gb": _parse_float_env("MOLT_DIFF_MEM_PER_JOB_GB") or 2.0,
            "order": os.environ.get("MOLT_DIFF_ORDER", "auto"),
            "warm_cache": warm_cache,
            "retry_oom": retry_oom,
        },
        "rss": {
            **_aggregate_rss_metrics(run_id),
            "top": rss_top,
        },
    }
    if failures_output is None:
        env_path = os.environ.get("MOLT_DIFF_FAILURES", "").strip()
        if env_path:
            failures_output = Path(env_path).expanduser()
        else:
            failures_output = _diff_root() / "failures.txt"
    if failures_output is not None and failed_files:
        try:
            failures_output.parent.mkdir(parents=True, exist_ok=True)
            failures_output.write_text("\n".join(failed_files) + "\n")
        except OSError:
            pass
    summary_output = os.environ.get("MOLT_DIFF_SUMMARY", "").strip()
    if summary_output:
        _emit_json(summary, summary_output, stdout=False)
    else:
        summary_path = _diff_root() / "summary.json"
        _emit_json(summary, str(summary_path), stdout=False)
    _print_rss_top(run_id, limit if _diff_measure_rss() else 0)
    return summary


def _emit_json(payload: dict, output_path: str | None, stdout: bool) -> None:
    text = json.dumps(payload, indent=2, sort_keys=True)
    if output_path:
        Path(output_path).write_text(text)
    if stdout:
        print(text)


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Molt Differential Test Harness")
    parser.add_argument(
        "file",
        nargs="*",
        help="Python file(s) or directory(ies) to test",
    )
    parser.add_argument(
        "--python-version", help="Python version to test against (e.g. 3.13)"
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit JSON summary to stdout.",
    )
    parser.add_argument(
        "--json-output",
        help="Write JSON summary to a file.",
    )
    parser.add_argument(
        "--jobs",
        type=int,
        default=None,
        help="Number of parallel workers (default: auto for multi-test runs).",
    )
    parser.add_argument(
        "--log-dir",
        help="Write per-test logs to a directory when running in parallel.",
    )
    parser.add_argument(
        "--log-file",
        help="Append live status lines to a central log file.",
    )
    parser.add_argument(
        "--log-aggregate",
        help="Append per-test stdout/stderr to a single log file.",
    )
    parser.add_argument(
        "--live",
        action="store_true",
        help="Emit per-test status lines as tests complete.",
    )
    parser.add_argument(
        "--fail-fast",
        action="store_true",
        help="Stop after the first failing test.",
    )
    parser.add_argument(
        "--failures-output",
        help="Write failed test paths to a file (default: MOLT_DIFF_ROOT/failures.txt).",
    )
    parser.add_argument(
        "--warm-cache",
        action="store_true",
        help="Warm shared MOLT_CACHE with build-only pass before running tests.",
    )
    parser.add_argument(
        "--retry-oom",
        action="store_true",
        help="Retry OOM failures once with --jobs 1 (enabled by default).",
    )
    parser.add_argument(
        "--no-retry-oom",
        action="store_true",
        help="Disable OOM retries.",
    )

    args = parser.parse_args()

    python_exe = sys.executable
    if args.python_version:
        python_exe = f"python{args.python_version}"

    log_dir = Path(args.log_dir).expanduser() if args.log_dir else None
    log_file = Path(args.log_file).expanduser() if args.log_file else None
    log_aggregate = (
        Path(args.log_aggregate).expanduser() if args.log_aggregate else None
    )
    failures_output = (
        Path(args.failures_output).expanduser() if args.failures_output else None
    )

    if args.file:
        targets = [Path(path) for path in args.file]
        retry_oom = _diff_retry_oom_default()
        if args.retry_oom:
            retry_oom = True
        if args.no_retry_oom:
            retry_oom = False
        summary = run_diff(
            targets,
            python_exe,
            jobs=args.jobs,
            log_dir=log_dir,
            log_file=log_file,
            log_aggregate=log_aggregate,
            live=args.live,
            fail_fast=args.fail_fast,
            failures_output=failures_output,
            warm_cache=args.warm_cache or _diff_warm_cache(),
            retry_oom=retry_oom,
        )
        _emit_json(summary, args.json_output, args.json)
        sys.exit(0 if summary["failed"] == 0 else 1)
    # Default test
    with open("temp_test.py", "w") as f:
        f.write("print(1 + 2)\n")
    summary = run_diff(
        Path("temp_test.py"),
        python_exe,
        jobs=args.jobs,
        log_dir=log_dir,
        log_file=log_file,
        log_aggregate=log_aggregate,
        live=args.live,
        fail_fast=args.fail_fast,
        failures_output=failures_output,
        warm_cache=args.warm_cache or _diff_warm_cache(),
        retry_oom=_diff_retry_oom_default(),
    )
    _emit_json(summary, args.json_output, args.json)
    os.remove("temp_test.py")
    sys.exit(0 if summary["failed"] == 0 else 1)
