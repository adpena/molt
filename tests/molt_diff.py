import atexit
import concurrent.futures
import contextlib
import io
import json
import os
import re
import signal
import socket
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import Sequence
from functools import lru_cache
from pathlib import Path

_ACTIVE_CHILD_PIDS: set[int] = set()
_SIGNAL_HANDLERS_INSTALLED = False
_DYLD_GUARD_MARKER = "dyld_guard.json"
_DIFF_RUN_LOCK_HANDLE: io.TextIOWrapper | None = None
_WORKER_ORPHAN_GUARD_INSTALLED = False

try:
    import fcntl  # type: ignore
except Exception:  # pragma: no cover - non-posix fallback
    fcntl = None


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


@lru_cache(maxsize=1)
def _resolve_molt_cli_python() -> str:
    override = os.environ.get("MOLT_DIFF_MOLT_PYTHON", "").strip()
    if override:
        return _resolve_python_exe(override)

    repo_root = Path(__file__).resolve().parents[1]
    if os.name == "nt":
        candidates = [repo_root / ".venv" / "Scripts" / "python.exe"]
    else:
        candidates = [
            repo_root / ".venv" / "bin" / "python3",
            repo_root / ".venv" / "bin" / "python",
        ]
    candidates.append(Path(sys.executable))

    for candidate in candidates:
        if not candidate.exists():
            continue
        try:
            probe = subprocess.run(
                [
                    str(candidate),
                    "-c",
                    "import packaging.markers, packaging.requirements",
                ],
                capture_output=True,
                text=True,
                check=False,
                timeout=5.0,
            )
        except (OSError, subprocess.TimeoutExpired):
            continue
        if probe.returncode == 0:
            return str(candidate)

    return sys.executable


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


def _diff_build_timeout(run_timeout: float | None) -> float | None:
    raw = os.environ.get("MOLT_DIFF_BUILD_TIMEOUT", "").strip()
    if raw:
        try:
            val = float(raw)
        except ValueError:
            val = 0.0
        if val > 0:
            return val
    if run_timeout is None:
        return None
    # Build can include queued runtime/backend work under shared locks, but
    # defaulting too high can leave deadlocked helpers alive for too long.
    return max(run_timeout * 2.0, 300.0)


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


def _diff_cargo_target_root() -> Path:
    raw = os.environ.get("MOLT_DIFF_CARGO_TARGET_DIR", "").strip()
    if raw:
        root = Path(raw).expanduser()
    else:
        root = _diff_root() / "target"
    root.mkdir(parents=True, exist_ok=True)
    return root


def _diff_backend_daemon_root() -> Path:
    return _diff_cargo_target_root() / ".molt_state" / "backend_daemon"


def _diff_build_lock_root() -> Path:
    return _diff_cargo_target_root() / ".molt_state" / "build_locks"


def _diff_state_root() -> Path:
    return _diff_cargo_target_root() / ".molt_state"


def _diff_run_lock_path() -> Path:
    return _diff_state_root() / "diff_run.lock"


def _diff_run_lock_wait_sec() -> float:
    raw = _parse_float_env("MOLT_DIFF_RUN_LOCK_WAIT_SEC")
    if raw is None:
        return 15 * 60.0
    return max(0.0, raw)


def _diff_run_lock_poll_sec() -> float:
    raw = _parse_float_env("MOLT_DIFF_RUN_LOCK_POLL_SEC")
    if raw is None:
        return 0.5
    return max(0.05, raw)


def _release_diff_run_lock() -> None:
    global _DIFF_RUN_LOCK_HANDLE
    handle = _DIFF_RUN_LOCK_HANDLE
    _DIFF_RUN_LOCK_HANDLE = None
    if handle is None:
        return
    if fcntl is not None:
        with contextlib.suppress(OSError):
            fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
    with contextlib.suppress(OSError):
        handle.close()


def _ensure_diff_run_lock() -> None:
    global _DIFF_RUN_LOCK_HANDLE
    if _DIFF_RUN_LOCK_HANDLE is not None:
        return
    if os.name != "posix" or fcntl is None:
        return
    lock_path = _diff_run_lock_path()
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    handle = open(lock_path, "a+", encoding="utf-8")
    wait_sec = _diff_run_lock_wait_sec()
    poll_sec = _diff_run_lock_poll_sec()
    deadline = time.monotonic() + wait_sec
    announced_wait = False
    while True:
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            break
        except BlockingIOError:
            if not announced_wait:
                print(
                    "[INFO] Waiting for active differential run lock at "
                    f"{lock_path} (timeout={wait_sec:.0f}s)"
                )
                announced_wait = True
            if wait_sec <= 0 or time.monotonic() >= deadline:
                handle.close()
                raise RuntimeError(
                    f"Timed out waiting for differential run lock: {lock_path}"
                )
            time.sleep(poll_sec)
    handle.seek(0)
    handle.truncate(0)
    handle.write(f"pid={os.getpid()} started={int(time.time())}\n")
    handle.flush()
    _DIFF_RUN_LOCK_HANDLE = handle
    atexit.register(_release_diff_run_lock)


def _dyld_guard_marker_path() -> Path:
    return _diff_state_root() / _DYLD_GUARD_MARKER


def _global_dyld_guard_marker_path() -> Path:
    # Keep dyld guard state in the shared diff root, independent of per-run
    # target overrides/quarantine paths.
    return _diff_root() / "target" / ".molt_state" / _DYLD_GUARD_MARKER


def _parse_int_env(name: str, default: int) -> int:
    raw = os.environ.get(name, "").strip()
    if not raw:
        return default
    try:
        value = int(raw)
    except ValueError:
        return default
    return value


@lru_cache(maxsize=8)
def _ps_supports_field(field: str) -> bool:
    if os.name != "posix":
        return False
    try:
        result = subprocess.run(
            ["ps", "-o", f"{field}=", "-p", str(os.getpid())],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return False
    if result.returncode != 0:
        return False
    stderr = (result.stderr or "").lower()
    if "keyword not found" in stderr:
        return False
    return True


def _parse_ps_elapsed(token: str) -> int | None:
    raw = token.strip()
    if not raw:
        return None
    if raw.isdigit():
        return int(raw)
    days = 0
    if "-" in raw:
        day_part, rest = raw.split("-", 1)
        if not day_part.isdigit():
            return None
        days = int(day_part)
        raw = rest
    parts = raw.split(":")
    if not parts or any(not part.isdigit() for part in parts):
        return None
    values = [int(part) for part in parts]
    if len(values) == 3:
        hours, minutes, seconds = values
    elif len(values) == 2:
        hours = 0
        minutes, seconds = values
    elif len(values) == 1:
        hours = 0
        minutes = 0
        seconds = values[0]
    else:
        return None
    return days * 86400 + hours * 3600 + minutes * 60 + seconds


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
    with contextlib.suppress(OSError):
        os.kill(pid, signal.SIGKILL)


def _daemon_ping(socket_path: Path, *, timeout: float = 0.75) -> bool:
    if os.name != "posix":
        return False
    if not socket_path.exists():
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


def _pid_rss_age(pid: int) -> tuple[int | None, int | None]:
    if os.name != "posix" or pid <= 0:
        return None, None
    age_field = "etimes" if _ps_supports_field("etimes") else "etime"
    try:
        result = subprocess.run(
            ["ps", "-o", f"rss=,{age_field}=", "-p", str(pid)],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None, None
    if result.returncode != 0:
        return None, None
    line = result.stdout.strip()
    if not line:
        return None, None
    parts = line.split()
    if len(parts) < 2:
        return None, None
    rss = int(parts[0]) if parts[0].isdigit() else None
    age = _parse_ps_elapsed(parts[1])
    return rss, age


def _list_backend_daemon_processes() -> dict[Path, list[int]]:
    groups: dict[Path, list[int]] = {}
    if os.name != "posix":
        return groups
    try:
        result = subprocess.run(
            ["ps", "-axo", "pid=,command="],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return groups
    pattern = re.compile(r"^\s*(\d+)\s+(.*)$")
    socket_pat = re.compile(r"--socket\s+(\S+)")
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
    return groups


def _list_orphan_diff_workers() -> list[int]:
    if os.name != "posix":
        return []
    repo_python = Path(__file__).resolve().parents[1] / ".venv" / "bin" / "python3"
    markers = (
        "from multiprocessing.spawn import spawn_main",
        "from multiprocessing.resource_tracker import main",
    )
    try:
        result = subprocess.run(
            ["ps", "-axo", "pid=,ppid=,command="],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return []
    orphan_pids: list[int] = []
    for raw_line in result.stdout.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        parts = line.split(None, 2)
        if len(parts) < 3:
            continue
        pid_raw, ppid_raw, cmd = parts
        if not (pid_raw.isdigit() and ppid_raw.isdigit()):
            continue
        if int(ppid_raw) != 1:
            continue
        if str(repo_python) not in cmd:
            continue
        if any(marker in cmd for marker in markers):
            orphan_pids.append(int(pid_raw))
    return sorted(set(orphan_pids))


def _prune_orphan_diff_workers() -> None:
    pids = _list_orphan_diff_workers()
    if not pids:
        return
    for pid in pids:
        _kill_pid(pid)
    print(f"[INFO] Pruned {len(pids)} orphan multiprocessing worker(s)")


def _list_process_rows() -> list[tuple[int, int, int, str]]:
    if os.name != "posix":
        return []
    age_field = "etimes" if _ps_supports_field("etimes") else "etime"
    try:
        result = subprocess.run(
            ["ps", "-axo", f"pid=,ppid=,{age_field}=,command="],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return []

    rows: list[tuple[int, int, int, str]] = []
    for raw_line in result.stdout.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        parts = line.split(None, 3)
        if len(parts) < 4:
            continue
        pid_raw, ppid_raw, elapsed_raw, cmd = parts
        if not (pid_raw.isdigit() and ppid_raw.isdigit()):
            continue
        elapsed = _parse_ps_elapsed(elapsed_raw)
        if elapsed is None:
            continue
        rows.append((int(pid_raw), int(ppid_raw), elapsed, cmd))
    return rows


def _is_diff_build_helper_command(cmd: str) -> bool:
    # Restrict to helper processes tied to diff temp dirs so we don't interfere
    # with unrelated local build activity.
    if "/molt_diff_" not in cmd:
        return False
    if "molt-backend" in cmd and "--output" in cmd and "--daemon" not in cmd:
        return True
    if ("-m molt.cli build " in cmd) or ("src/molt/cli.py build " in cmd):
        return True
    if cmd.rstrip().endswith("_molt"):
        return True
    return False


def _list_orphan_build_helpers() -> list[int]:
    if os.name != "posix":
        return []
    rows = _list_process_rows()
    if not rows:
        return []

    cmd_by_pid = {pid: cmd for pid, _ppid, _etimes, cmd in rows}
    ppid_by_pid = {pid: ppid for pid, ppid, _etimes, _cmd in rows}
    stale_sec = max(60, _parse_int_env("MOLT_DIFF_HELPER_STALE_SEC", 20 * 60))

    def _has_diff_ancestor(pid: int) -> bool:
        seen: set[int] = set()
        current = ppid_by_pid.get(pid, 0)
        while current > 1 and current not in seen:
            seen.add(current)
            cmd = cmd_by_pid.get(current, "")
            if "tests/molt_diff.py" in cmd or "molt_diff.py " in cmd:
                return True
            current = ppid_by_pid.get(current, 0)
        return False

    pids: list[int] = []
    for pid, ppid, etimes, cmd in rows:
        if not _is_diff_build_helper_command(cmd):
            continue
        if ppid == 1:
            pids.append(pid)
            continue
        # A helper that has outlived any diff harness ancestry is stale and can
        # deadlock later runs by holding shared build locks.
        if etimes >= stale_sec and not _has_diff_ancestor(pid):
            pids.append(pid)
    return sorted(set(pids))


def _prune_orphan_build_helpers() -> None:
    pids = _list_orphan_build_helpers()
    if not pids:
        return
    for pid in pids:
        _kill_pid(pid, grace=0.35)
    print(f"[INFO] Pruned {len(pids)} orphan build helper process(es)")


def _prune_backend_daemons() -> None:
    if os.name != "posix":
        return
    max_rss_kb = _parse_int_env("MOLT_DIFF_DAEMON_MAX_RSS_KB", 2_500_000)
    unresponsive_stale_sec = max(
        60, _parse_int_env("MOLT_DIFF_DAEMON_STALE_SEC", 10 * 60)
    )
    groups = _list_backend_daemon_processes()
    for socket_path, pids in groups.items():
        live = sorted({pid for pid in pids if _pid_alive(pid)})
        if not live:
            continue
        if max_rss_kb > 0:
            filtered: list[int] = []
            for pid in live:
                rss_kb, _age_sec = _pid_rss_age(pid)
                if rss_kb is not None and rss_kb > max_rss_kb:
                    _kill_pid(pid)
                    print(
                        "[INFO] Pruned backend daemon pid="
                        f"{pid} rss={rss_kb}KB (> {max_rss_kb}KB)"
                    )
                    continue
                filtered.append(pid)
            live = sorted({pid for pid in filtered if _pid_alive(pid)})
            if not live:
                continue
        if not socket_path.exists():
            for pid in live:
                _kill_pid(pid)
            continue
        if len(live) > 1:
            # Keep the newest pid; terminate duplicate daemons bound to the same socket.
            for pid in live[:-1]:
                _kill_pid(pid)
            live = live[-1:]
        ping_ok = _daemon_ping(socket_path)
        if not ping_ok:
            pid = live[0]
            _rss_kb, age_sec = _pid_rss_age(pid)
            if age_sec is not None and age_sec >= unresponsive_stale_sec:
                _kill_pid(pid)
                print(
                    "[INFO] Pruned stale unresponsive backend daemon pid="
                    f"{pid} age={age_sec}s socket={socket_path}"
                )

    daemon_root = _diff_backend_daemon_root()
    if not daemon_root.exists():
        return
    for pid_path in daemon_root.glob("*.pid"):
        stem = pid_path.stem
        socket_path = pid_path.with_name(f"{stem}.sock")
        try:
            raw = pid_path.read_text().strip()
        except OSError:
            continue
        if not raw.isdigit():
            with contextlib.suppress(OSError):
                pid_path.unlink()
            continue
        pid = int(raw)
        if not _pid_alive(pid):
            with contextlib.suppress(OSError):
                pid_path.unlink()
            continue
        if not socket_path.exists():
            _kill_pid(pid)
            with contextlib.suppress(OSError):
                pid_path.unlink()


def _prune_stale_build_locks() -> None:
    lock_root = _diff_build_lock_root()
    if not lock_root.exists():
        return
    now = time.time()
    max_age = _parse_int_env("MOLT_DIFF_BUILD_LOCK_MAX_AGE_SEC", 12 * 60 * 60)
    max_keep = _parse_int_env("MOLT_DIFF_BUILD_LOCK_MAX_FILES", 4096)
    removed = 0
    lock_entries: list[tuple[float, Path]] = []
    try:
        for lock_path in lock_root.glob("*.lock"):
            try:
                stat = lock_path.stat()
            except OSError:
                continue
            # Build-lock files are coordination sentinels and should remain empty.
            if stat.st_size > 0:
                continue
            lock_entries.append((stat.st_mtime, lock_path))
            if max_age > 0 and (now - stat.st_mtime) > max_age:
                with contextlib.suppress(OSError):
                    lock_path.unlink()
                    removed += 1
    except OSError:
        return

    if max_keep > 0 and len(lock_entries) - removed > max_keep:
        # Keep the newest N lock sentinels to avoid directory growth over long runs.
        stale = sorted(lock_entries, key=lambda item: item[0])[
            : max(0, len(lock_entries) - max_keep)
        ]
        for _mtime, lock_path in stale:
            if not lock_path.exists():
                continue
            with contextlib.suppress(OSError):
                lock_path.unlink()
                removed += 1
    if removed > 0:
        print(f"[INFO] Pruned {removed} stale build lock file(s) from {lock_root}")


def _diff_keep_artifacts() -> bool:
    raw = os.environ.get("MOLT_DIFF_KEEP", "").strip().lower()
    return raw in {"1", "true", "yes", "on"}


def _diff_log_passes() -> bool:
    raw = os.environ.get("MOLT_DIFF_LOG_PASSES", "").strip().lower()
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


def _diff_retry_dyld_default() -> bool:
    raw = os.environ.get("MOLT_DIFF_RETRY_DYLD", "").strip().lower()
    if raw:
        return raw in {"1", "true", "yes", "on"}
    return True


def _diff_dyld_preflight_default() -> bool:
    explicit = _bool_env("MOLT_DIFF_DYLD_PREFLIGHT")
    if explicit is not None:
        return explicit
    return sys.platform == "darwin"


def _bool_env(name: str) -> bool | None:
    raw = os.environ.get(name, "").strip().lower()
    if not raw:
        return None
    if raw in {"1", "true", "yes", "on"}:
        return True
    if raw in {"0", "false", "no", "off"}:
        return False
    return None


def _diff_backend_daemon_default() -> bool:
    explicit = _bool_env("MOLT_DIFF_BACKEND_DAEMON")
    if explicit is not None:
        return explicit
    inherited = _bool_env("MOLT_BACKEND_DAEMON")
    if inherited is not None:
        return inherited
    # dyld "unknown imports format" has been observed repeatedly on macOS
    # daemon lanes; defaulting to off keeps diff runs stable.
    return sys.platform != "darwin"


def _diff_disable_daemon_on_dyld() -> bool:
    raw = os.environ.get("MOLT_DIFF_DISABLE_DAEMON_ON_DYLD", "").strip().lower()
    if raw:
        return raw in {"1", "true", "yes", "on"}
    return True


def _diff_quarantine_on_dyld() -> bool:
    explicit = _bool_env("MOLT_DIFF_QUARANTINE_ON_DYLD")
    if explicit is not None:
        return explicit
    # Keep shared target/state by default to avoid expensive cold rebuilds.
    return False


def _diff_dyld_local_fallback() -> bool:
    explicit = _bool_env("MOLT_DIFF_DYLD_LOCAL_FALLBACK")
    if explicit is not None:
        return explicit
    # macOS is where dyld import-format corruption has been observed.
    return sys.platform == "darwin"


def _diff_dyld_local_root() -> Path:
    raw = os.environ.get("MOLT_DIFF_DYLD_LOCAL_ROOT", "").strip()
    if raw:
        return Path(raw).expanduser()
    return Path(tempfile.gettempdir()) / "molt_diff_dyld"


def _diff_force_no_cache() -> bool:
    explicit = _bool_env("MOLT_DIFF_FORCE_NO_CACHE")
    if explicit is not None:
        return explicit
    # On macOS, forcing fresh builds avoids intermittent dyld
    # "unknown imports format" crashes observed on cached artifacts.
    return sys.platform == "darwin"


def _diff_force_rebuild() -> bool:
    explicit = _bool_env("MOLT_DIFF_FORCE_REBUILD")
    if explicit is not None:
        return explicit
    return False


def _diff_force_rebuild_on_dyld() -> bool:
    explicit = _bool_env("MOLT_DIFF_FORCE_REBUILD_ON_DYLD")
    if explicit is not None:
        return explicit
    return True


def _diff_dyld_guard_ttl_sec() -> int:
    return max(60, _parse_int_env("MOLT_DIFF_DYLD_GUARD_TTL_SEC", 6 * 60 * 60))


def _diff_dyld_guard_runs() -> int:
    # Quarantine only a bounded number of subsequent runs after a dyld incident.
    # This keeps safety hardening while avoiding long streaks of cold rebuilds.
    return max(1, _parse_int_env("MOLT_DIFF_DYLD_GUARD_RUNS", 1))


def _read_dyld_guard_marker() -> dict[str, object] | None:
    marker_path = _global_dyld_guard_marker_path()
    if not marker_path.exists():
        return None
    try:
        raw = marker_path.read_text()
    except OSError:
        return None
    try:
        data = json.loads(raw)
    except json.JSONDecodeError:
        return None
    if not isinstance(data, dict):
        return None
    return data


def _write_dyld_guard_marker(data: dict[str, object]) -> None:
    marker_path = _global_dyld_guard_marker_path()
    marker_path.parent.mkdir(parents=True, exist_ok=True)
    marker_path.write_text(json.dumps(data, sort_keys=True))


def _clear_dyld_guard_marker() -> None:
    with contextlib.suppress(OSError):
        _global_dyld_guard_marker_path().unlink()


def _mark_dyld_guard(file_path: str) -> None:
    payload = {
        "ts": int(time.time()),
        "pid": os.getpid(),
        "run_id": os.environ.get("MOLT_DIFF_RUN_ID", ""),
        "file": file_path,
        "cargo_target_dir": os.environ.get("CARGO_TARGET_DIR", ""),
        "remaining_runs": _diff_dyld_guard_runs(),
    }
    _write_dyld_guard_marker(payload)


def _should_preemptive_dyld_quarantine() -> bool:
    force = os.environ.get("MOLT_DIFF_DYLD_PREEMPTIVE", "").strip().lower()
    if force in {"1", "true", "yes", "on"}:
        return True
    if force in {"0", "false", "no", "off"}:
        return False
    clear = os.environ.get("MOLT_DIFF_CLEAR_DYLD_GUARD", "").strip().lower()
    if clear in {"1", "true", "yes", "on"}:
        _clear_dyld_guard_marker()
        return False
    marker_path = _global_dyld_guard_marker_path()
    marker_data = _read_dyld_guard_marker()
    if marker_data is None:
        if marker_path.exists():
            _clear_dyld_guard_marker()
        return False
    try:
        age_sec = time.time() - marker_path.stat().st_mtime
    except OSError:
        return False
    if age_sec > _diff_dyld_guard_ttl_sec():
        _clear_dyld_guard_marker()
        return False
    remaining_raw = marker_data.get("remaining_runs", _diff_dyld_guard_runs())
    remaining = remaining_raw if isinstance(remaining_raw, int) else 0
    if remaining <= 0:
        _clear_dyld_guard_marker()
        return False
    return True


def _consume_dyld_guard_run() -> int | None:
    marker_data = _read_dyld_guard_marker()
    if marker_data is None:
        return None
    remaining_raw = marker_data.get("remaining_runs", _diff_dyld_guard_runs())
    remaining = remaining_raw if isinstance(remaining_raw, int) else 0
    remaining -= 1
    if remaining <= 0:
        _clear_dyld_guard_marker()
        return 0
    marker_data["remaining_runs"] = remaining
    marker_data["last_consume_ts"] = int(time.time())
    _write_dyld_guard_marker(marker_data)
    return remaining


def _activate_dyld_quarantine_target(
    *, use_local: bool = False
) -> tuple[Path, Path, bool]:
    run_id = os.environ.get("MOLT_DIFF_RUN_ID", "").strip() or "adhoc"
    safe_run_id = re.sub(r"[^A-Za-z0-9_.-]+", "_", run_id)
    if use_local:
        quarantine_root = _diff_dyld_local_root() / safe_run_id
    else:
        quarantine_root = _diff_root() / "dyld_quarantine" / safe_run_id
    target_dir = quarantine_root / "target"
    state_dir = quarantine_root / "state"
    target_dir.mkdir(parents=True, exist_ok=True)
    state_dir.mkdir(parents=True, exist_ok=True)
    activated = (
        os.environ.get("MOLT_DIFF_CARGO_TARGET_DIR", "") != str(target_dir)
        or os.environ.get("MOLT_BUILD_STATE_DIR", "") != str(state_dir)
        or os.environ.get("MOLT_BACKEND_DAEMON", "") != "0"
    )
    os.environ["MOLT_DIFF_CARGO_TARGET_DIR"] = str(target_dir)
    os.environ["MOLT_BUILD_STATE_DIR"] = str(state_dir)
    os.environ["CARGO_TARGET_DIR"] = str(target_dir)
    os.environ["MOLT_BACKEND_DAEMON"] = "0"
    return target_dir, state_dir, activated


def _diff_retry_isolated_default() -> bool:
    raw = os.environ.get("MOLT_DIFF_RETRY_ISOLATED", "").strip().lower()
    if raw:
        return raw in {"1", "true", "yes", "on"}
    return True


def _diff_keep_isolated_retry_dirs() -> bool:
    raw = os.environ.get("MOLT_DIFF_KEEP_ISOLATED_RETRY", "").strip().lower()
    return raw in {"1", "true", "yes", "on"}


def _dyld_preflight_error(binary_path: Path) -> str | None:
    if not _diff_dyld_preflight_default() or sys.platform != "darwin":
        return None
    otool = shutil.which("otool")
    if not otool:
        return None
    try:
        probe = subprocess.run(
            [otool, "-l", str(binary_path)],
            capture_output=True,
            text=True,
            check=False,
            timeout=20,
        )
    except OSError:
        # Preflight is best-effort; host tool failures are not binary corruption.
        return None
    except subprocess.TimeoutExpired:
        return None
    merged = "\n".join([probe.stdout or "", probe.stderr or ""]).lower()
    if probe.returncode != 0:
        # Only gate when otool output indicates the specific dyld corruption we care
        # about; generic preflight failures should not poison the run.
        if "unknown imports format" in merged:
            return "dyld: unknown imports format (preflight)"
        if "malformed load command" in merged:
            return "dyld: unknown imports format (preflight malformed load command)"
        return None
    if "unknown imports format" in merged:
        return "dyld: unknown imports format (preflight)"
    if "malformed load command" in merged:
        return "dyld: unknown imports format (preflight malformed load command)"
    return None


def _diff_allow_rustc_wrapper() -> bool:
    raw = os.environ.get("MOLT_DIFF_ALLOW_RUSTC_WRAPPER", "").strip().lower()
    return raw in {"1", "true", "yes", "on"}


def _diff_build_profile() -> str:
    raw = os.environ.get("MOLT_DIFF_BUILD_PROFILE", "").strip().lower()
    if raw in {"dev", "release"}:
        return raw
    return "dev"


def _diff_prune_every() -> int:
    return max(0, _parse_int_env("MOLT_DIFF_PRUNE_EVERY", 32))


def _diff_max_tasks_per_child() -> int | None:
    raw = _parse_int_env("MOLT_DIFF_MAX_TASKS_PER_CHILD", 0)
    return raw if raw > 0 else None


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


def _diff_fail_rss_kb() -> int | None:
    raw = (
        os.environ.get("MOLT_DIFF_FAIL_RSS_KB", "").strip()
        or os.environ.get("MOLT_DIFF_MAX_RSS_KB", "").strip()
    )
    if not raw:
        return None
    try:
        value = int(raw)
    except ValueError:
        return None
    return value if value > 0 else None


def _rss_exceeded(
    metrics: dict[str, int] | None, threshold_kb: int | None
) -> tuple[bool, str | None]:
    if threshold_kb is None or not metrics:
        return False, None
    candidates = [
        ("max_rss", metrics.get("max_rss")),
        ("peak_footprint", metrics.get("peak_footprint")),
    ]
    for name, value in candidates:
        if isinstance(value, int) and value > threshold_kb:
            return True, f"{name}={value}KB exceeds {threshold_kb}KB"
    return False, None


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
        manifest = target / "TESTS.txt"
        if manifest.is_file():
            files: list[Path] = []
            seen: set[Path] = set()
            for raw in manifest.read_text(encoding="utf-8").splitlines():
                line = raw.strip()
                if not line or line.startswith("#"):
                    continue
                path = Path(line)
                if not path.is_absolute():
                    path = Path.cwd() / path
                if not path.exists():
                    raise FileNotFoundError(
                        f"Manifest entry missing: {line} (from {manifest})"
                    )
                if path.is_dir():
                    pattern = _diff_glob()
                    matches = sorted(path.glob(pattern))
                else:
                    matches = [path]
                for match in matches:
                    if match.suffix != ".py":
                        continue
                    resolved = match.resolve()
                    if resolved in seen:
                        continue
                    seen.add(resolved)
                    files.append(match)
            return files
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


def _diff_worker(file_path: str, python_exe: str, build_profile: str) -> dict[str, str]:
    _install_worker_orphan_guard()
    buffer_out = io.StringIO()
    buffer_err = io.StringIO()
    with contextlib.redirect_stdout(buffer_out), contextlib.redirect_stderr(buffer_err):
        status = diff_test(file_path, python_exe, build_profile=build_profile)
    return {
        "path": file_path,
        "status": status,
        "stdout": buffer_out.getvalue(),
        "stderr": buffer_err.getvalue(),
    }


def _install_worker_orphan_guard() -> None:
    global _WORKER_ORPHAN_GUARD_INSTALLED
    if _WORKER_ORPHAN_GUARD_INSTALLED:
        return
    _WORKER_ORPHAN_GUARD_INSTALLED = True
    if os.name != "posix":
        return

    def _watch_parent() -> None:
        while True:
            time.sleep(1.0)
            # If the harness process dies abruptly (app reset/kill), worker
            # processes become orphaned under init/launchd (ppid=1). Exit
            # proactively so they do not accumulate and consume memory.
            if os.getppid() == 1:
                os._exit(0)

    threading.Thread(
        target=_watch_parent, name="molt-diff-orphan-guard", daemon=True
    ).start()


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


def _diff_run_single(
    file_path: str, python_exe: str, build_profile: str
) -> dict[str, str]:
    buffer_out = io.StringIO()
    buffer_err = io.StringIO()
    out_stream = _TeeStream(sys.stdout, buffer_out)
    err_stream = _TeeStream(sys.stderr, buffer_err)
    with contextlib.redirect_stdout(out_stream), contextlib.redirect_stderr(err_stream):
        status = diff_test(file_path, python_exe, build_profile=build_profile)
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


def _popen_group_kwargs() -> dict[str, object]:
    if os.name == "nt":
        creationflags = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
        return {"creationflags": creationflags}
    return {"start_new_session": True}


def _terminate_pid_tree(pid: int, *, grace: float = 1.0) -> None:
    if pid <= 0:
        return
    if os.name == "nt":
        with contextlib.suppress(Exception):
            subprocess.run(
                ["taskkill", "/PID", str(pid), "/T", "/F"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
                timeout=grace,
            )
        with contextlib.suppress(OSError):
            os.kill(pid, signal.SIGKILL)
        return
    with contextlib.suppress(ProcessLookupError, PermissionError):
        os.killpg(os.getpgid(pid), signal.SIGTERM)
    deadline = time.monotonic() + max(0.05, grace)
    while time.monotonic() < deadline:
        if not _pid_alive(pid):
            return
        time.sleep(0.05)
    with contextlib.suppress(ProcessLookupError, PermissionError):
        os.killpg(os.getpgid(pid), signal.SIGKILL)
    with contextlib.suppress(OSError):
        os.kill(pid, signal.SIGKILL)


def _reap_lingering_process_group(pgid: int, *, grace: float = 0.25) -> None:
    if os.name == "nt" or pgid <= 0:
        return
    # Subprocesses run in a dedicated session/group. If the direct child exits
    # but leaves descendants behind, proactively reap that process group.
    with contextlib.suppress(ProcessLookupError, PermissionError):
        os.killpg(pgid, 0)
    try:
        os.killpg(pgid, 0)
    except (ProcessLookupError, PermissionError):
        return
    with contextlib.suppress(ProcessLookupError, PermissionError):
        os.killpg(pgid, signal.SIGTERM)
    deadline = time.monotonic() + max(0.05, grace)
    while time.monotonic() < deadline:
        try:
            os.killpg(pgid, 0)
        except (ProcessLookupError, PermissionError):
            return
        time.sleep(0.02)
    with contextlib.suppress(ProcessLookupError, PermissionError):
        os.killpg(pgid, signal.SIGKILL)


def _terminate_active_children() -> None:
    for pid in sorted(_ACTIVE_CHILD_PIDS):
        _terminate_pid_tree(pid, grace=0.35)
    _ACTIVE_CHILD_PIDS.clear()


def _install_signal_cleanup_handlers() -> None:
    global _SIGNAL_HANDLERS_INSTALLED
    if _SIGNAL_HANDLERS_INSTALLED:
        return
    if os.name != "posix":
        _SIGNAL_HANDLERS_INSTALLED = True
        return

    def _handler(signum, _frame):
        _terminate_active_children()
        raise SystemExit(128 + int(signum))

    signal.signal(signal.SIGTERM, _handler)
    signal.signal(signal.SIGINT, _handler)
    _SIGNAL_HANDLERS_INSTALLED = True


def _terminate_process_tree(proc: subprocess.Popen[str], *, grace: float = 1.0) -> None:
    if proc.poll() is not None:
        return
    if os.name == "nt":
        with contextlib.suppress(Exception):
            subprocess.run(
                ["taskkill", "/PID", str(proc.pid), "/T", "/F"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
                timeout=grace,
            )
        with contextlib.suppress(Exception):
            proc.kill()
        with contextlib.suppress(Exception):
            proc.wait(timeout=grace)
        return

    with contextlib.suppress(ProcessLookupError, PermissionError):
        pgid = os.getpgid(proc.pid)
        os.killpg(pgid, signal.SIGTERM)
    try:
        proc.wait(timeout=grace)
        return
    except subprocess.TimeoutExpired:
        pass
    with contextlib.suppress(ProcessLookupError, PermissionError):
        pgid = os.getpgid(proc.pid)
        os.killpg(pgid, signal.SIGKILL)
    with contextlib.suppress(Exception):
        proc.kill()
    with contextlib.suppress(Exception):
        proc.wait(timeout=grace)


def _run_subprocess(
    cmd: list[str], *, env: dict[str, str], timeout: float | None
) -> subprocess.CompletedProcess[str]:
    _install_signal_cleanup_handlers()
    proc = subprocess.Popen(
        cmd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        errors="surrogateescape",
        **_popen_group_kwargs(),
    )
    _ACTIVE_CHILD_PIDS.add(proc.pid)
    try:
        stdout, stderr = proc.communicate(timeout=timeout)
    except subprocess.TimeoutExpired as exc:
        _terminate_process_tree(proc)
        raise subprocess.TimeoutExpired(
            cmd=cmd,
            timeout=timeout,
            output=exc.output,
            stderr=exc.stderr,
        ) from exc
    finally:
        _ACTIVE_CHILD_PIDS.discard(proc.pid)
        _reap_lingering_process_group(proc.pid)
    return subprocess.CompletedProcess(cmd, proc.returncode, stdout, stderr)


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
    return _run_subprocess(run_cmd, env=env, timeout=timeout)


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
    # Keep CPython baseline path resolution aligned with the Molt build/run env.
    env["PYTHONPATH"] = "src"
    env["PYTHONHASHSEED"] = "0"
    # Keep CPython tempfile roots aligned with Molt subprocess roots so path
    # semantics (especially macOS /var vs /private/var) are compared fairly.
    cpython_tmp = _diff_tmp_root() / "cpython_tmp"
    cpython_tmp.mkdir(parents=True, exist_ok=True)
    env["TMPDIR"] = str(cpython_tmp)
    env["TEMP"] = str(cpython_tmp)
    env["TMP"] = str(cpython_tmp)
    env.update(_collect_env_overrides(file_path))
    bootstrap = """
import importlib.machinery as _machinery
import os as _os
import runpy
import sys

if "importlib_resources_" in sys.argv[1]:
    # Keep zipfile available even when tests temporarily replace sys.path
    # with synthetic roots that omit the host stdlib path.
    import zipfile as _molt_diff_zipfile_preload  # noqa: F401
    try:
        from importlib.readers import MultiplexedPath as _MoltDiffMultiplexedPath
    except Exception:
        _MoltDiffMultiplexedPath = None
    if _MoltDiffMultiplexedPath is not None and not hasattr(
        _MoltDiffMultiplexedPath, "__fspath__"
    ):
        def _molt_diff_multiplexed_fspath(self):
            paths = getattr(self, "_paths", None)
            if isinstance(paths, (list, tuple)) and paths:
                return str(paths[0])
            return str(self)
        _MoltDiffMultiplexedPath.__fspath__ = _molt_diff_multiplexed_fspath

if "importlib_extension_exec_" in sys.argv[1]:
    _orig_create_module = _machinery.ExtensionFileLoader.create_module
    _orig_exec_module = _machinery.ExtensionFileLoader.exec_module

    def _strip_ext(path):
        for suffix in (".so", ".pyd", ".dll", ".dylib"):
            if path.endswith(suffix):
                return path[: -len(suffix)]
        return None

    def _strip_cpython_tag(stem):
        marker = ".cpython-"
        if marker in stem:
            return stem.split(marker, 1)[0]
        return stem

    def _candidate_shim_paths(module_file):
        out = []
        if not module_file:
            return out

        def _append(candidate):
            if candidate and candidate not in out:
                out.append(candidate)

        _append(f"{module_file}.molt.py")
        _append(f"{module_file}.py")

        stripped = _strip_ext(module_file)
        if stripped:
            _append(f"{stripped}.molt.py")
            _append(f"{stripped}.py")
            stripped_tag = _strip_cpython_tag(stripped)
            if stripped_tag != stripped:
                _append(f"{stripped_tag}.molt.py")
                _append(f"{stripped_tag}.py")

        dirname = _os.path.dirname(module_file)
        basename = _os.path.basename(module_file)
        base_noext = _strip_ext(basename) or basename
        base_tag = _strip_cpython_tag(base_noext)

        if dirname:
            if base_tag.startswith("__init__"):
                _append(_os.path.join(dirname, "__init__.molt.py"))
                _append(_os.path.join(dirname, "__init__.py"))

            if _os.path.basename(dirname) == "__pycache__":
                parent = _os.path.dirname(dirname)
                _append(_os.path.join(parent, f"{base_tag}.molt.py"))
                _append(_os.path.join(parent, f"{base_tag}.py"))

        return out

    def _molt_diff_create_module(self, spec):
        try:
            return _orig_create_module(self, spec)
        except (ImportError, OSError, PermissionError):
            return None

    def _molt_diff_exec_module(self, module):
        module_file = getattr(module, "__file__", None)
        if not module_file:
            spec = getattr(module, "__spec__", None)
            module_file = getattr(spec, "origin", None) if spec is not None else None
        shim_path = None
        for candidate in _candidate_shim_paths(module_file):
            if _os.path.exists(candidate):
                shim_path = candidate
                break
        shim_exists = bool(shim_path)
        exec_failed = False
        try:
            _orig_exec_module(self, module)
        except (ImportError, OSError, PermissionError):
            exec_failed = True
            if not shim_exists:
                raise
        if shim_exists:
            with open(shim_path, "rb") as _shim_file:
                _shim_src = _shim_file.read()
            try:
                _shim_code = compile(_shim_src, shim_path, "exec")
            except SyntaxError:
                _shim_text = _shim_src.decode("utf-8", "surrogateescape")
                _shim_unescaped = bytes(_shim_text, "utf-8").decode("unicode_escape")
                _shim_code = compile(_shim_unescaped, shim_path, "exec")
            exec(_shim_code, module.__dict__, module.__dict__)
        elif not exec_failed:
            # Match Molt runtime behavior where extension execution without a
            # loadable shim is treated as unavailable.
            raise ImportError("extension execution unavailable")
        return None

    _machinery.ExtensionFileLoader.create_module = _molt_diff_create_module
    _machinery.ExtensionFileLoader.exec_module = _molt_diff_exec_module

runpy.run_path(sys.argv[1], run_name="__main__")
"""
    timeout = _diff_timeout()
    try:
        result = _run_subprocess(
            [python_exe, "-c", bootstrap, file_path],
            env=env,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return "", f"Timeout after {timeout}s", 124
    return result.stdout, result.stderr, result.returncode


def run_molt(
    file_path: str,
    build_profile: str,
    *,
    daemon_enabled: bool | None = None,
    no_cache: bool = False,
    rebuild: bool = False,
    extra_env: dict[str, str] | None = None,
):
    return _run_molt(
        file_path,
        build_only=False,
        build_profile=build_profile,
        daemon_enabled=daemon_enabled,
        no_cache=no_cache,
        rebuild=rebuild,
        extra_env=extra_env,
    )


def run_molt_build_only(
    file_path: str,
    build_profile: str,
    *,
    daemon_enabled: bool | None = None,
    no_cache: bool = False,
    rebuild: bool = False,
    extra_env: dict[str, str] | None = None,
) -> tuple[str, str, int]:
    return _run_molt(
        file_path,
        build_only=True,
        build_profile=build_profile,
        daemon_enabled=daemon_enabled,
        no_cache=no_cache,
        rebuild=rebuild,
        extra_env=extra_env,
    )


def _run_molt(
    file_path: str,
    *,
    build_only: bool,
    build_profile: str,
    daemon_enabled: bool | None,
    no_cache: bool,
    rebuild: bool,
    extra_env: dict[str, str] | None,
) -> tuple[str | None, str, int]:
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
    # Keep differential builds hermetic to the configured diff roots so host
    # ~/.molt state and inherited shell paths cannot destabilize runs.
    diff_home = _diff_root() / ".molt_home"
    diff_home.mkdir(parents=True, exist_ok=True)
    env.setdefault("MOLT_HOME", str(diff_home))
    env.setdefault("MOLT_BIN", str(diff_home / "bin"))
    env.setdefault("MOLT_BUILD_STATE_DIR", str(_diff_state_root()))
    shared_cache = env.get("MOLT_CACHE")
    if shared_cache:
        Path(shared_cache).mkdir(parents=True, exist_ok=True)
    else:
        env["MOLT_CACHE"] = str(cache_root)
    env["TMPDIR"] = str(tmp_root)
    env["TEMP"] = str(tmp_root)
    env["TMP"] = str(tmp_root)
    # Keep wrappers disabled by default for reproducibility; opt in explicitly
    # when the host wrapper cache is known-good for this environment.
    if not _diff_allow_rustc_wrapper():
        env.pop("RUSTC_WRAPPER", None)
        env.pop("CARGO_BUILD_RUSTC_WRAPPER", None)
        # Force-disable wrapper/caching helpers in this mode, even when parent
        # shells exported throughput-oriented defaults.
        env["SCCACHE_DISABLE"] = "1"
        env["MOLT_USE_SCCACHE"] = "0"
    # Always route through the diff target root (which itself honors
    # MOLT_DIFF_CARGO_TARGET_DIR) instead of inheriting unrelated shell state.
    env["CARGO_TARGET_DIR"] = str(_diff_cargo_target_root())
    if "MOLT_TRUSTED" not in env and _diff_trusted_default():
        env["MOLT_TRUSTED"] = "1"
    env.update(_collect_env_overrides(file_path))
    if extra_env:
        env.update(extra_env)
    if daemon_enabled is None:
        daemon_enabled = _diff_backend_daemon_default()
    env["MOLT_BACKEND_DAEMON"] = "1" if daemon_enabled else "0"
    if _diff_force_no_cache():
        no_cache = True
    if _diff_force_rebuild():
        rebuild = True
    env.setdefault("MOLT_SYS_EXECUTABLE", _resolve_python_exe(sys.executable))
    ver = sys.version_info
    env.setdefault(
        "MOLT_SYS_VERSION_INFO",
        f"{ver.major},{ver.minor},{ver.micro},{ver.releaselevel},{ver.serial}",
    )
    env.setdefault("MOLT_SYS_VERSION", sys.version)
    timeout = _diff_timeout()
    build_timeout = _diff_build_timeout(timeout)
    rss_limit_kb = _diff_fail_rss_kb()
    try:
        build_cmd = [
            _resolve_molt_cli_python(),
            "-m",
            "molt.cli",
            "build",
            file_path,
            "--profile",
            build_profile,
            "--out-dir",
            str(output_root),
            "--output",
            str(output_binary),
        ]
        if no_cache:
            build_cmd.append("--no-cache")
        if rebuild:
            build_cmd.append("--rebuild")
        codec = env.get("MOLT_CODEC")
        if codec:
            build_cmd.extend(["--codec", codec])
        try:
            build_res = _run_with_optional_time(
                build_cmd,
                env=env,
                timeout=build_timeout,
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
            return None, f"Timeout after {build_timeout}s", 124
        if build_time_path is not None:
            build_metrics = _parse_time_metrics(build_time_path)
        exceeded, detail = _rss_exceeded(build_metrics, rss_limit_kb)
        if exceeded:
            message = f"Build RSS limit exceeded: {detail}"
            _record_rss_metrics(
                file_path,
                build_metrics=build_metrics,
                run_metrics=None,
                build_rc=125,
                run_rc=None,
                status="build_rss_exceeded",
            )
            return None, message, 125
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

        preflight_err = _dyld_preflight_error(output_binary)
        if preflight_err is not None:
            _record_rss_metrics(
                file_path,
                build_metrics=build_metrics,
                run_metrics=None,
                build_rc=126,
                run_rc=None,
                status="build_dyld_preflight_failed",
            )
            return None, preflight_err, 126

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
        exceeded, detail = _rss_exceeded(run_metrics, rss_limit_kb)
        if exceeded:
            message = f"Run RSS limit exceeded: {detail}"
            _record_rss_metrics(
                file_path,
                build_metrics=build_metrics,
                run_metrics=run_metrics,
                build_rc=build_res.returncode,
                run_rc=125,
                status="run_rss_exceeded",
            )
            return "", message, 125
        run_status = "ok" if run_res.returncode == 0 else "run_failed"
        _record_rss_metrics(
            file_path,
            build_metrics=build_metrics,
            run_metrics=run_metrics,
            build_rc=build_res.returncode,
            run_rc=run_res.returncode,
            status=run_status,
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
    # Keep OOM detection strict to avoid false positives like "boom".
    if re.search(r"\boom\b", needle):
        return True
    return any(
        token in needle
        for token in (
            "out of memory",
            "std::bad_alloc",
            "memoryerror",
            "cannot allocate memory",
            "allocation failed",
        )
    )


def _should_retry_oom(code: int | None, stderr: str) -> bool:
    return _is_oom_returncode(code) or _is_oom_error(stderr)


def _is_dyld_unknown_imports(stderr: str) -> bool:
    needle = stderr.lower()
    return "dyld" in needle and "unknown imports format" in needle


def _is_timeout_error(stderr: str) -> bool:
    needle = stderr.lower()
    return "timeout after" in needle


def _is_backend_daemon_build_error(stderr: str) -> bool:
    needle = stderr.lower()
    return any(
        token in needle
        for token in (
            "backend daemon failed to become ready",
            "incompatiblesignature(",
            "backend compilation failed",
        )
    )


@contextlib.contextmanager
def _isolated_retry_env(*, local_tmp: bool = False):
    retry_base = Path(tempfile.gettempdir()) if local_tmp else _diff_tmp_root()
    retry_root = Path(tempfile.mkdtemp(prefix="molt_diff_retry_", dir=retry_base))
    target_dir = retry_root / "target"
    state_dir = retry_root / "state"
    target_dir.mkdir(parents=True, exist_ok=True)
    state_dir.mkdir(parents=True, exist_ok=True)
    env = {
        "CARGO_TARGET_DIR": str(target_dir),
        "MOLT_BUILD_STATE_DIR": str(state_dir),
        "MOLT_BACKEND_DAEMON": "0",
        "MOLT_USE_SCCACHE": "0",
    }
    try:
        yield env
    finally:
        if not _diff_keep_isolated_retry_dirs():
            shutil.rmtree(retry_root, ignore_errors=True)


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


def _top_rss_entries(
    run_id: str, limit: int, *, phase: str = "run"
) -> list[dict[str, object]]:
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
        block = entry.get(phase) or {}
        if isinstance(block, dict):
            value = block.get("max_rss")
            if isinstance(value, int):
                return value
        return 0

    ranked = [entry for entry in entries if metric(entry) > 0]
    ranked.sort(key=metric, reverse=True)
    return ranked[: max(0, limit)]


def _print_rss_top(run_id: str, limit: int) -> None:
    if limit <= 0:
        return
    build_entries = _top_rss_entries(run_id, limit, phase="build")
    run_entries = _top_rss_entries(run_id, limit, phase="run")
    if not build_entries and not run_entries:
        return

    def fmt(value: object) -> str:
        return f"{value} KB" if isinstance(value, int) and value > 0 else "-"

    if build_entries:
        print(f"Top {len(build_entries)} RSS offenders (build phase):")
        for entry in build_entries:
            file_path = entry.get("file", "<unknown>")
            status = entry.get("status", "")
            run_block = entry.get("run") or {}
            build_block = entry.get("build") or {}
            run_rss = run_block.get("max_rss") if isinstance(run_block, dict) else None
            build_rss = (
                build_block.get("max_rss") if isinstance(build_block, dict) else None
            )
            print(
                f"- {file_path} | build={fmt(build_rss)} run={fmt(run_rss)} status={status}"
            )

    if run_entries:
        print(f"Top {len(run_entries)} RSS offenders (run phase):")
        for entry in run_entries:
            file_path = entry.get("file", "<unknown>")
            status = entry.get("status", "")
            run_block = entry.get("run") or {}
            build_block = entry.get("build") or {}
            run_rss = run_block.get("max_rss") if isinstance(run_block, dict) else None
            build_rss = (
                build_block.get("max_rss") if isinstance(build_block, dict) else None
            )
            print(
                f"- {file_path} | run={fmt(run_rss)} build={fmt(build_rss)} status={status}"
            )


def diff_test(file_path, python_exe=sys.executable, build_profile: str = "dev"):
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
    molt_out, molt_err, molt_ret = run_molt(file_path, build_profile)
    saw_dyld_retry = False
    if _diff_retry_dyld_default() and _is_dyld_unknown_imports(molt_err):
        _mark_dyld_guard(file_path)
        saw_dyld_retry = True
        print(
            "[RETRY] "
            f"{file_path} encountered dyld unknown imports format; "
            "retrying with backend daemon disabled (cache preserved)."
        )
        retry_out, retry_err, retry_ret = run_molt(
            file_path,
            build_profile,
            daemon_enabled=False,
            no_cache=False,
        )
        molt_out, molt_err, molt_ret = retry_out, retry_err, retry_ret
        if _is_dyld_unknown_imports(molt_err):
            print(
                "[RETRY] "
                f"{file_path} persistent dyld failure; retrying with "
                "daemon disabled and --no-cache on shared target."
            )
            retry_out, retry_err, retry_ret = run_molt(
                file_path,
                build_profile,
                daemon_enabled=False,
                no_cache=True,
            )
            molt_out, molt_err, molt_ret = retry_out, retry_err, retry_ret
        if _is_dyld_unknown_imports(molt_err) and _diff_force_rebuild_on_dyld():
            print(
                "[RETRY] "
                f"{file_path} persistent dyld failure; retrying with "
                "daemon disabled, --no-cache, and --rebuild."
            )
            retry_out, retry_err, retry_ret = run_molt(
                file_path,
                build_profile,
                daemon_enabled=False,
                no_cache=True,
                rebuild=True,
            )
            molt_out, molt_err, molt_ret = retry_out, retry_err, retry_ret
        if _is_dyld_unknown_imports(molt_err) and _diff_retry_isolated_default():
            use_local_retry = _diff_dyld_local_fallback()
            print(
                "[RETRY] "
                f"{file_path} persistent dyld failure; retrying with isolated "
                f"{'local /tmp ' if use_local_retry else ''}"
                "target/build-state, daemon off, and --rebuild."
            )
            with _isolated_retry_env(local_tmp=use_local_retry) as isolated_env:
                retry_out, retry_err, retry_ret = run_molt(
                    file_path,
                    build_profile,
                    daemon_enabled=False,
                    no_cache=True,
                    rebuild=True,
                    extra_env=isolated_env,
                )
            molt_out, molt_err, molt_ret = retry_out, retry_err, retry_ret
    if saw_dyld_retry and _diff_disable_daemon_on_dyld():
        os.environ["MOLT_BACKEND_DAEMON"] = "0"
        os.environ["MOLT_DIFF_FORCE_NO_CACHE"] = "1"
        if _diff_force_rebuild_on_dyld():
            os.environ["MOLT_DIFF_FORCE_REBUILD"] = "1"
        if _diff_quarantine_on_dyld():
            use_local_quarantine = _diff_dyld_local_fallback()
            target_dir, state_dir, activated = _activate_dyld_quarantine_target(
                use_local=use_local_quarantine
            )
            if activated:
                print(
                    "[WARN] dyld unknown imports format detected; forcing "
                    "MOLT_BACKEND_DAEMON=0 and quarantining remaining tests to "
                    f"{'local ' if use_local_quarantine else ''}"
                    f"target={target_dir} state={state_dir} (rebuild forced)."
                )
        else:
            print(
                "[WARN] dyld unknown imports format detected; forcing "
                "MOLT_BACKEND_DAEMON=0, --no-cache, and --rebuild for "
                "remaining tests in this run (shared target retained)."
            )
    if molt_out is None and _is_backend_daemon_build_error(molt_err):
        print(
            "[RETRY] "
            f"{file_path} backend daemon/cache build failure; retrying with "
            "daemon disabled (cache preserved)."
        )
        retry_out, retry_err, retry_ret = run_molt(
            file_path,
            build_profile,
            daemon_enabled=False,
            no_cache=False,
        )
        molt_out, molt_err, molt_ret = retry_out, retry_err, retry_ret
        if (
            molt_out is None
            and _is_backend_daemon_build_error(molt_err)
            and _diff_retry_isolated_default()
        ):
            print(
                "[RETRY] "
                f"{file_path} persistent backend daemon/cache failure; retrying with "
                "isolated target/build-state and --no-cache."
            )
            with _isolated_retry_env() as isolated_env:
                retry_out, retry_err, retry_ret = run_molt(
                    file_path,
                    build_profile,
                    daemon_enabled=False,
                    no_cache=True,
                    extra_env=isolated_env,
                )
            molt_out, molt_err, molt_ret = retry_out, retry_err, retry_ret
        if molt_out is None and _is_backend_daemon_build_error(molt_err):
            os.environ["MOLT_BACKEND_DAEMON"] = "0"
            print(
                "[WARN] Persistent backend daemon/cache failure detected; "
                "forcing MOLT_BACKEND_DAEMON=0 for remaining tests in this run."
            )
    if _should_retry_oom(molt_ret, molt_err):
        print(f"[OOM] {file_path}")
        return "oom"

    cp_out = _normalize_output(cp_out, normalize)
    cp_err = _normalize_output(cp_err, normalize)
    if molt_out is not None:
        molt_out = _normalize_output(molt_out, normalize)
    molt_err = _normalize_output(molt_err, normalize)
    stderr_match = stderr_mode in {"match", "exact"}

    if molt_out is None:
        if _is_timeout_error(molt_err) and _diff_retry_isolated_default():
            print(
                "[RETRY] "
                f"{file_path} build timeout; retrying with isolated target/build-state."
            )
            with _isolated_retry_env() as isolated_env:
                retry_out, retry_err, retry_ret = run_molt(
                    file_path,
                    build_profile,
                    daemon_enabled=False,
                    no_cache=True,
                    extra_env=isolated_env,
                )
            molt_out, molt_err, molt_ret = retry_out, retry_err, retry_ret
            if molt_out is not None:
                cp_out = _normalize_output(cp_out, normalize)
                cp_err = _normalize_output(cp_err, normalize)
                molt_out = _normalize_output(molt_out, normalize)
                molt_err = _normalize_output(molt_err, normalize)
                stderr_ok = True
                if stderr_match:
                    stderr_ok = cp_err == molt_err
                if cp_out == molt_out and cp_ret == molt_ret and stderr_ok:
                    print(f"[PASS] {file_path}")
                    return "pass"
                print(f"[FAIL] {file_path} mismatch")
                print(f"  CPython stdout: {cp_out!r}")
                print(f"  Molt    stdout: {molt_out!r}")
                print(f"  CPython return: {cp_ret} stderr: {cp_err!r}")
                print(f"  Molt    return: {molt_ret} stderr: {molt_err!r}")
                return "fail"

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
    build_profile: str = "dev",
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
    _ensure_diff_run_lock()
    _prune_orphan_diff_workers()
    _prune_orphan_build_helpers()
    _prune_backend_daemons()
    _prune_stale_build_locks()
    results: list[tuple[str, str]] = []
    if isinstance(target, Path):
        test_files = _collect_test_files(target)
    else:
        test_files = _collect_test_files_multi(target)
    if jobs is None:
        jobs = _default_jobs() if len(test_files) > 1 else 1
    run_id = _diff_run_id()
    os.environ["MOLT_DIFF_RUN_ID"] = run_id
    if _should_preemptive_dyld_quarantine() and _diff_disable_daemon_on_dyld():
        remaining = _consume_dyld_guard_run()
        remaining_suffix = (
            f" remaining_guard_runs={remaining}." if remaining is not None else "."
        )
        os.environ["MOLT_BACKEND_DAEMON"] = "0"
        os.environ["MOLT_DIFF_FORCE_NO_CACHE"] = "1"
        if _diff_force_rebuild_on_dyld():
            os.environ["MOLT_DIFF_FORCE_REBUILD"] = "1"
        if _diff_quarantine_on_dyld():
            use_local_quarantine = _diff_dyld_local_fallback()
            target_dir, state_dir, activated = _activate_dyld_quarantine_target(
                use_local=use_local_quarantine
            )
            if activated:
                print(
                    "[WARN] Active dyld guard marker detected; forcing "
                    "MOLT_BACKEND_DAEMON=0 and quarantining this run to "
                    f"{'local ' if use_local_quarantine else ''}"
                    f"target={target_dir} state={state_dir} with rebuild forced"
                    f"{remaining_suffix}"
                )
        else:
            print(
                "[WARN] Active dyld guard marker detected; forcing "
                "MOLT_BACKEND_DAEMON=0, --no-cache, and --rebuild for this run "
                f"(shared target retained){remaining_suffix}"
            )
    os.environ.setdefault("CARGO_TARGET_DIR", str(_diff_cargo_target_root()))
    test_files = _order_test_files(test_files, jobs)
    if warm_cache:
        shared_cache = os.environ.get("MOLT_CACHE")
        if not shared_cache:
            shared_cache = str(_diff_root() / "molt_cache")
            os.environ["MOLT_CACHE"] = shared_cache
        for file_path in test_files:
            _out, err, rc = run_molt_build_only(str(file_path), build_profile)
            if rc != 0:
                print(f"[WARM-CACHE FAIL] {file_path}: {err.strip()}")
    if jobs <= 1:
        with _open_log_file(log_file) as log_handle:
            with _open_log_file(log_aggregate) as aggregate_handle:
                for file_path in test_files:
                    payload = _diff_run_single(
                        str(file_path), python_exe, build_profile
                    )
                    path = payload["path"]
                    status = payload["status"]
                    results.append((path, status))
                    if log_handle is not None:
                        _emit_line(
                            f"[{status.upper()}] {path}",
                            log_handle,
                            echo=False,
                        )
                    if aggregate_handle is not None and (
                        status != "pass" or _diff_log_passes()
                    ):
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
        requested_live = live
        if not live:
            live = True
        outputs: dict[str, dict[str, str]] = {}
        keep_full_payloads = (not requested_live) and log_dir is None
        keep_retry_payloads = retry_oom
        prune_every = _diff_prune_every()
        completed = 0
        with _open_log_file(log_file) as log_handle:
            with _open_log_file(log_aggregate) as aggregate_handle:
                executor_kwargs: dict[str, int] = {"max_workers": jobs}
                max_tasks_per_child = _diff_max_tasks_per_child()
                if max_tasks_per_child is not None:
                    executor_kwargs["max_tasks_per_child"] = max_tasks_per_child
                executor_params = {
                    "initializer": _install_worker_orphan_guard,
                }
                try:
                    executor_ctx = concurrent.futures.ProcessPoolExecutor(
                        **executor_kwargs, **executor_params
                    )
                except TypeError:
                    executor_kwargs.pop("max_tasks_per_child", None)
                    executor_ctx = concurrent.futures.ProcessPoolExecutor(
                        **executor_kwargs, **executor_params
                    )
                with executor_ctx as executor:
                    futures = {
                        executor.submit(
                            _diff_worker, str(file_path), python_exe, build_profile
                        ): str(file_path)
                        for file_path in test_files
                    }
                    for future in concurrent.futures.as_completed(futures):
                        result = future.result()
                        path = result["path"]
                        status = result["status"]
                        completed += 1
                        if keep_full_payloads or (
                            keep_retry_payloads and status == "oom"
                        ):
                            outputs[path] = result
                        results.append((path, status))
                        log_path = None
                        if log_dir is not None:
                            persist_log = status != "pass" or _diff_log_passes()
                            candidate_log_path = _log_path_for_test(log_dir, path)
                            if persist_log:
                                log_path = _write_test_log(
                                    log_dir, path, result["stdout"], result["stderr"]
                                )
                            else:
                                with contextlib.suppress(OSError):
                                    candidate_log_path.unlink()
                        _emit_line(
                            f"[{status.upper()}] {path}",
                            log_handle,
                            echo=live,
                        )
                        if status == "fail" and log_path is not None:
                            _emit_line(f"  log: {log_path}", log_handle, echo=live)
                        if aggregate_handle is not None and (
                            status != "pass" or _diff_log_passes()
                        ):
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
                        if prune_every > 0 and completed % prune_every == 0:
                            _prune_orphan_diff_workers()
                            _prune_orphan_build_helpers()
                            _prune_backend_daemons()
        if not live and log_dir is None:
            for file_path in test_files:
                payload = outputs.get(str(file_path))
                if payload is None:
                    continue
                if payload["stdout"]:
                    print(payload["stdout"], end="")
                if payload["stderr"]:
                    print(payload["stderr"], end="", file=sys.stderr)
    _prune_orphan_diff_workers()
    _prune_orphan_build_helpers()
    _prune_backend_daemons()
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
            retry_payload = _diff_run_single(path, python_exe, build_profile)
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
    rss_top_run = [
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
        for entry in _top_rss_entries(
            run_id, limit if _diff_measure_rss() else 0, phase="run"
        )
    ]
    rss_top_build = [
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
        for entry in _top_rss_entries(
            run_id, limit if _diff_measure_rss() else 0, phase="build"
        )
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
            "cargo_target_dir": os.environ.get("CARGO_TARGET_DIR", ""),
            "build_profile": build_profile,
            "warm_cache": warm_cache,
            "retry_oom": retry_oom,
        },
        "rss": {
            **_aggregate_rss_metrics(run_id),
            "top": rss_top_run,
            "top_run": rss_top_run,
            "top_build": rss_top_build,
        },
    }
    if failures_output is None:
        env_path = os.environ.get("MOLT_DIFF_FAILURES", "").strip()
        if env_path:
            failures_output = Path(env_path).expanduser()
        else:
            failures_output = _diff_root() / "failures.txt"
    if failures_output is not None:
        try:
            failures_output.parent.mkdir(parents=True, exist_ok=True)
            payload = ("\n".join(failed_files) + "\n") if failed_files else ""
            failures_output.write_text(payload)
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
        "--files-from",
        action="append",
        default=[],
        help=(
            "Read additional test paths from a newline-delimited file. "
            "Can be provided multiple times."
        ),
    )
    parser.add_argument(
        "--python-version", help="Python version to test against (e.g. 3.13)"
    )
    parser.add_argument(
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help=(
            "Build profile forwarded to `molt build` for the Molt side "
            "(default: MOLT_DIFF_BUILD_PROFILE or dev)."
        ),
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
    build_profile = args.build_profile or _diff_build_profile()

    log_dir = Path(args.log_dir).expanduser() if args.log_dir else None
    log_file = Path(args.log_file).expanduser() if args.log_file else None
    log_aggregate = (
        Path(args.log_aggregate).expanduser() if args.log_aggregate else None
    )
    failures_output = (
        Path(args.failures_output).expanduser() if args.failures_output else None
    )

    target_paths: list[str] = list(args.file)
    for list_path in args.files_from:
        try:
            entries = Path(list_path).read_text().splitlines()
        except OSError as exc:
            print(f"Failed to read --files-from {list_path}: {exc}", file=sys.stderr)
            sys.exit(2)
        for entry in entries:
            raw = entry.strip()
            if not raw or raw.startswith("#"):
                continue
            target_paths.append(raw)

    if target_paths:
        targets = [Path(path) for path in target_paths]
        retry_oom = _diff_retry_oom_default()
        if args.retry_oom:
            retry_oom = True
        if args.no_retry_oom:
            retry_oom = False
        try:
            summary = run_diff(
                targets,
                python_exe,
                build_profile=build_profile,
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
        except RuntimeError as exc:
            print(f"[LOCK] {exc}", file=sys.stderr)
            sys.exit(2)
        _emit_json(summary, args.json_output, args.json)
        sys.exit(0 if summary["failed"] == 0 else 1)
    # Default test
    with open("temp_test.py", "w") as f:
        f.write("print(1 + 2)\n")
    try:
        summary = run_diff(
            Path("temp_test.py"),
            python_exe,
            build_profile=build_profile,
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
    except RuntimeError as exc:
        print(f"[LOCK] {exc}", file=sys.stderr)
        os.remove("temp_test.py")
        sys.exit(2)
    _emit_json(summary, args.json_output, args.json)
    os.remove("temp_test.py")
    sys.exit(0 if summary["failed"] == 0 else 1)
