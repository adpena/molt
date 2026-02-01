import concurrent.futures
import contextlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
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
        root = Path("logs") / "molt_diff"
    root.mkdir(parents=True, exist_ok=True)
    return root


def _diff_tmp_root() -> Path:
    raw = os.environ.get("MOLT_DIFF_TMPDIR", "").strip()
    if raw:
        root = Path(raw).expanduser()
    else:
        root = _diff_root()
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


def _default_jobs() -> int:
    count = os.cpu_count() or 1
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
        parts = line.split()
        if not parts:
            continue
        try:
            value = int(parts[0])
        except ValueError:
            continue
        if "maximum resident set size" in line:
            metrics["max_rss"] = value
        elif "peak memory footprint" in line:
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
            run_cmd = [time_bin, "-l", "-o", str(time_path), *cmd]
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
    payload = {
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
    env = os.environ.copy()
    paths = [env.get("PYTHONPATH", ""), ".", "src"]
    env["PYTHONPATH"] = os.pathsep.join(p for p in paths if p)
    env["PYTHONHASHSEED"] = "0"
    env.update(_collect_env_overrides(file_path))
    bootstrap = (
        "import runpy, sys; "
        "import molt.shims as shims; "
        "shims.install(); "
        "runpy.run_path(sys.argv[1], run_name='__main__')"
    )
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
    if cp_ret != 0 and (
        "msgpack is required for parse_msgpack fallback" in cp_err
        or "cbor2 is required for parse_cbor fallback" in cp_err
    ):
        print(f"[SKIP] {file_path} (missing msgpack/cbor2 in CPython env)")
        return "skip"
    molt_out, molt_err, molt_ret = run_molt(file_path)

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
) -> dict:
    results: list[tuple[str, str]] = []
    if isinstance(target, Path):
        test_files = _collect_test_files(target)
    else:
        test_files = _collect_test_files_multi(target)
    if jobs is None:
        jobs = _default_jobs() if len(test_files) > 1 else 1
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
    discovered = len(results)
    failed_files = [path for path, status in results if status == "fail"]
    skipped_files = [path for path, status in results if status == "skip"]
    failed = len(failed_files)
    passed = len([None for _, status in results if status == "pass"])
    skipped = len(skipped_files)
    total = passed + failed
    return {
        "discovered": discovered,
        "total": total,
        "passed": passed,
        "failed": failed,
        "skipped": skipped,
        "failed_files": failed_files,
        "skipped_files": skipped_files,
        "python_exe": python_exe,
        "jobs": jobs,
    }


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

    args = parser.parse_args()

    python_exe = sys.executable
    if args.python_version:
        python_exe = f"python{args.python_version}"

    log_dir = Path(args.log_dir).expanduser() if args.log_dir else None
    log_file = Path(args.log_file).expanduser() if args.log_file else None
    log_aggregate = (
        Path(args.log_aggregate).expanduser() if args.log_aggregate else None
    )

    if args.file:
        targets = [Path(path) for path in args.file]
        summary = run_diff(
            targets,
            python_exe,
            jobs=args.jobs,
            log_dir=log_dir,
            log_file=log_file,
            log_aggregate=log_aggregate,
            live=args.live,
            fail_fast=args.fail_fast,
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
    )
    _emit_json(summary, args.json_output, args.json)
    os.remove("temp_test.py")
    sys.exit(0 if summary["failed"] == 0 else 1)
