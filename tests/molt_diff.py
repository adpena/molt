import atexit
import concurrent.futures
import contextlib
import io
import json
import os
import re
import runpy
import signal
import socket
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path

# Make the repository root importable so the `tools` / `tests` packages resolve
# regardless of the current working directory or how this harness is launched.
# Running `python3 tests/molt_diff.py` puts `tests/` (the script directory) on
# sys.path, NOT the repo root, so the `from tools...` imports below would fail
# with ModuleNotFoundError unless PYTHONPATH happened to include the root. This
# self-bootstrap makes the documented invocation work from any directory.
_REPO_ROOT = Path(__file__).resolve().parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))
_SRC_ROOT = _REPO_ROOT / "src"
if str(_SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(_SRC_ROOT))

from tools.batch_compile_client import BatchCompileServerClient  # noqa: E402  (must follow the sys.path self-bootstrap above)
from tools import (  # noqa: E402  (must follow the sys.path self-bootstrap above)
    harness_memory_guard,
    memory_guard,
    process_sentinel,
    resource_pressure,
)
from molt import backend_daemon_custody as daemon_custody  # noqa: E402

_DYLD_GUARD_MARKER = "dyld_guard.json"
_DIFF_RUN_LOCK_HANDLE: io.TextIOWrapper | None = None
_WORKER_ORPHAN_GUARD_INSTALLED = False
_BATCH_COMPILE_SERVER_CLIENT: "_BatchCompileServerClient | None" = None
_BATCH_COMPILE_SERVER_CLIENT_PID = 0
_BATCH_COMPILE_SERVER_DISABLED_UNTIL = 0.0
_BATCH_COMPILE_SERVER_DISABLE_REASON = ""
_BATCH_COMPILE_SERVER_FAILURE_COUNT = 0
_DIFF_MEMORY_GUARD_TRIP_FILE_ENV = "MOLT_DIFF_MEMORY_GUARD_TRIP_FILE"
_DIFF_MEMORY_GUARD_EVENTS_JSONL_ENV = "MOLT_DIFF_MEMORY_GUARD_EVENTS_JSONL"
_DIFF_MEMORY_GUARD_GLOBAL_SAMPLES_JSONL_ENV = (
    "MOLT_DIFF_MEMORY_GUARD_GLOBAL_SAMPLES_JSONL"
)
_DIFF_MEMORY_GUARD_DEFAULT_SAMPLE_INTERVAL_SEC = 1.0
_DIFF_MEMORY_GUARD_DEFAULT_EVENT_MAX_MB = 1.0
_DIFF_MEMORY_GUARD_DEFAULT_SAMPLE_MAX_MB = 2.0
_DIFF_MEMORY_GUARD_RETURN_CODE = memory_guard.GUARD_RETURN_CODE
_DIFF_MEMORY_GUARD_HARD_GLOBAL_GB = harness_memory_guard.HARD_RSS_LIMIT_GB
_DIFF_MEMORY_GUARD_HARD_GLOBAL_KB = memory_guard.max_rss_kb_from_gb(
    _DIFF_MEMORY_GUARD_HARD_GLOBAL_GB
)

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


def _metadata_probe_timeout_sec() -> float:
    raw = os.environ.get("MOLT_DIFF_METADATA_PROBE_TIMEOUT_SEC", "5").strip()
    try:
        value = float(raw)
    except ValueError:
        return 5.0
    return value if value > 0 else 5.0


def _run_metadata_probe(cmd: Sequence[str]) -> subprocess.CompletedProcess[str] | None:
    try:
        return harness_memory_guard.guarded_completed_process(
            cmd,
            prefix="MOLT_DIFF",
            capture_output=True,
            text=True,
            timeout=_metadata_probe_timeout_sec(),
        )
    except (OSError, subprocess.TimeoutExpired):
        return None


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
        probe = _run_metadata_probe(
            [
                str(candidate),
                "-c",
                "import packaging.markers, packaging.requirements",
            ]
        )
        if probe is None:
            continue
        if probe.returncode == 0:
            return str(candidate)

    return sys.executable


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def _stdlib_full_coverage_manifest_path() -> Path:
    return _repo_root() / "tools" / "stdlib_full_coverage_manifest.py"


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


def _diff_capabilities(env: dict[str, str]) -> str:
    if "MOLT_DIFF_CAPABILITIES" in env:
        return env["MOLT_DIFF_CAPABILITIES"]
    if "MOLT_CAPABILITIES" in env:
        return env["MOLT_CAPABILITIES"]
    return "fs,env,time,random"


def _metadata_stdlib_profile(file_path: str) -> tuple[str | None, str | None]:
    values = _collect_meta(file_path).get("stdlib_profile", [])
    normalized = {value.strip().lower() for value in values if value.strip()}
    if not normalized:
        return None, None
    if len(normalized) != 1:
        return None, "MOLT_META stdlib_profile must select exactly one profile"
    profile = next(iter(normalized))
    if profile not in {"micro", "full"}:
        return None, "MOLT_META stdlib_profile must be 'micro' or 'full'"
    return profile, None


def _apply_metadata_env_overrides(file_path: str, env: dict[str, str]) -> str | None:
    profile, error = _metadata_stdlib_profile(file_path)
    if error is not None:
        return error
    if profile is None:
        return None
    explicit = env.get("MOLT_DIFF_STDLIB_PROFILE", "").strip().lower()
    if explicit and explicit != profile:
        return (
            f"{file_path} requires MOLT_DIFF_STDLIB_PROFILE={profile} "
            f"but selected {explicit}"
        )
    env["MOLT_DIFF_STDLIB_PROFILE"] = profile
    return None


def _normalize_repo_relative(path: str | Path) -> str:
    candidate = Path(path)
    if not candidate.is_absolute():
        candidate = (_repo_root() / candidate).resolve()
    else:
        candidate = candidate.resolve()
    try:
        rel = candidate.relative_to(_repo_root())
    except ValueError:
        return candidate.as_posix()
    return rel.as_posix()


@lru_cache(maxsize=1)
def _too_dynamic_expected_failure_tests() -> frozenset[str]:
    manifest = _stdlib_full_coverage_manifest_path()
    if not manifest.exists():
        return frozenset()
    try:
        namespace = runpy.run_path(str(manifest))
    except Exception:
        return frozenset()
    raw = namespace.get("TOO_DYNAMIC_EXPECTED_FAILURE_TESTS", ())
    if not isinstance(raw, tuple):
        return frozenset()
    out: set[str] = set()
    for item in raw:
        if not isinstance(item, str):
            continue
        out.add(_normalize_repo_relative(item))
    return frozenset(out)


def _manifest_marks_expected_failure(file_path: str) -> bool:
    return _normalize_repo_relative(file_path) in _too_dynamic_expected_failure_tests()


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
    result = _run_metadata_probe(
        [python_exe, "-c", "import sys; print(sys.version_info[:2])"]
    )
    if result is None:
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


@lru_cache(maxsize=None)
def _molt_sys_env_for_python_exe(python_exe: str) -> dict[str, str]:
    """Derive Molt sys/version environment from the CPython under test.

    Differential runs compare Molt output against a chosen CPython version
    (3.12/3.13/3.14). When semantics differ across those versions, Molt must be
    configured to match the CPython baseline version for that run.
    """

    if not python_exe:
        return {}
    code = (
        "import json,sys;"
        "vi=sys.version_info;"
        "print(json.dumps({"
        "'executable':sys.executable,"
        "'version':sys.version,"
        "'version_info':[vi.major,vi.minor,vi.micro,vi.releaselevel,vi.serial]"
        "}))"
    )
    result = _run_metadata_probe([python_exe, "-c", code])
    if result is None:
        return {}
    if result.returncode != 0:
        return {}
    raw = (result.stdout or "").strip().splitlines()
    if not raw:
        return {}
    try:
        payload = json.loads(raw[-1])
    except json.JSONDecodeError:
        return {}
    if not isinstance(payload, dict):
        return {}
    version_info = payload.get("version_info")
    if (
        not isinstance(version_info, list)
        or len(version_info) != 5
        or not isinstance(version_info[0], int)
        or not isinstance(version_info[1], int)
        or not isinstance(version_info[2], int)
        or not isinstance(version_info[3], str)
        or not isinstance(version_info[4], int)
    ):
        return {}
    major, minor, micro, releaselevel, serial = version_info
    executable = payload.get("executable")
    version = payload.get("version")
    env: dict[str, str] = {
        "MOLT_PYTHON_VERSION": f"{major}.{minor}",
        "MOLT_SYS_VERSION_INFO": f"{major},{minor},{micro},{releaselevel},{serial}",
    }
    if isinstance(executable, str) and executable:
        env["MOLT_SYS_EXECUTABLE"] = executable
    if isinstance(version, str) and version:
        env["MOLT_SYS_VERSION"] = version
    return env


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


_STDOUT_NUMERIC_TOKEN_RE = re.compile(
    r"(?<![A-Za-z0-9_])[-+]?(?:\d+(?:\.\d+)?|\.\d+)(?:[eE][-+]?\d+)?(?![A-Za-z0-9_])"
)
_STDOUT_SPACING_RE = re.compile(r"\s+")


def _canonicalize_stdout(text: str, mode: str) -> str:
    normalized = mode.strip().lower()
    if normalized in {"", "exact"}:
        return text
    if normalized == "pyperformance":
        lines: list[str] = []
        for raw_line in text.splitlines():
            line = raw_line.strip()
            if not line:
                continue
            line = _STDOUT_NUMERIC_TOKEN_RE.sub("<num>", line)
            line = _STDOUT_SPACING_RE.sub(" ", line)
            lines.append(line)
        return "\n".join(lines)
    return text


_EXCEPTION_SIGNATURE_RE = re.compile(
    r"^(?P<etype>[A-Za-z_][A-Za-z0-9_.]*)(?:: (?P<message>.*))?$"
)


def _extract_exception_signature(stderr: str) -> tuple[str, str] | None:
    lines = [line.strip() for line in stderr.splitlines() if line.strip()]
    for line in reversed(lines):
        match = _EXCEPTION_SIGNATURE_RE.match(line)
        if match is None:
            continue
        etype = match.group("etype")
        message = match.group("message") or ""
        return etype, message
    return None


def _stderr_matches(cpython_stderr: str, molt_stderr: str, mode: str) -> bool:
    normalized = mode.strip().lower()
    if normalized in {"", "ignore"}:
        return True
    if normalized in {"match", "exact"}:
        return cpython_stderr == molt_stderr
    if normalized in {"traceback", "exception", "exception_signature"}:
        cpython_sig = _extract_exception_signature(cpython_stderr)
        molt_sig = _extract_exception_signature(molt_stderr)
        if cpython_sig is None or molt_sig is None:
            return cpython_stderr == molt_stderr
        # Frame/path formatting may differ across engines (especially wasm),
        # but exception type/message must remain exact.
        return cpython_sig == molt_sig
    return cpython_stderr == molt_stderr


def _truthy_flag(values: list[str]) -> bool:
    for value in values:
        if value.strip().lower() in {"1", "true", "yes", "on"}:
            return True
    return False


def _meta_expect_molt_fail(meta: dict[str, list[str]]) -> bool:
    values = [v.lower() for v in meta.get("expect_fail", []) + meta.get("xfail", [])]
    return "molt" in values


def _meta_expect_fail_reason(meta: dict[str, list[str]]) -> str:
    values = meta.get("expect_fail_reason", []) + meta.get("xfail_reason", [])
    if not values:
        return ""
    return values[0].strip()


def _resolve_expected_failure_status(
    *,
    expect_molt_fail: bool,
    raw_status: str,
    cpython_returncode: int,
) -> tuple[str, str | None]:
    if not expect_molt_fail:
        return raw_status, None
    if cpython_returncode != 0:
        return raw_status, None
    if raw_status == "fail":
        return "pass", "xfail"
    if raw_status == "pass":
        return "fail", "xpass"
    return raw_status, None


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
        artifact_root = os.environ.get("MOLT_EXT_ROOT", "").strip()
        if artifact_root:
            root = Path(artifact_root).expanduser() / "tmp" / "diff"
        else:
            root = _repo_root() / "tmp" / "diff"
    root.mkdir(parents=True, exist_ok=True)
    return root


def _diff_results_jsonl_path() -> Path | None:
    """Optional per-test RAW-status sink for the suite-honesty ratchet (#46).

    When MOLT_DIFF_RESULTS_JSONL is set, every diff_test() call appends one JSON
    line recording the test's repo-relative path and its RAW status (before the
    xfail/xpass overlay), plus the resolved status and reason tag for audit. This
    is the single authoritative record of what Molt actually did vs CPython;
    tools/check_suite_honesty.py consumes it so a tracked-but-failing test, or a
    silently-fixed one, can never read as green. Off by default (returns None).
    Workers are separate processes, so each appends to the shared file — the same
    cross-process JSONL idiom used by the memory guard above.
    """
    raw = os.environ.get("MOLT_DIFF_RESULTS_JSONL", "").strip()
    if not raw:
        return None
    return Path(raw).expanduser()


def _record_diff_result(record: dict[str, object]) -> None:
    path = _diff_results_jsonl_path()
    if path is None:
        return
    # A raw_status of None means an exception escaped before any status was
    # assigned; record it explicitly as "error" so the honesty ratchet treats an
    # unexpected crash as a hard, visible outcome rather than a missing line.
    if record.get("raw_status") is None:
        record = {**record, "raw_status": "error", "resolved_status": "error"}
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        line = json.dumps(record, sort_keys=True) + "\n"
        with path.open("a", encoding="utf-8") as handle:
            handle.write(line)
    except OSError:
        pass


def _diff_tmp_root() -> Path:
    raw = os.environ.get("MOLT_DIFF_TMPDIR", "").strip()
    if raw:
        root = Path(raw).expanduser()
    else:
        artifact_root = os.environ.get("MOLT_EXT_ROOT", "").strip()
        if artifact_root:
            root = Path(artifact_root).expanduser() / "tmp"
        else:
            root = _repo_root() / "tmp"
    root.mkdir(parents=True, exist_ok=True)
    return root


def _diff_cargo_target_root() -> Path:
    raw = os.environ.get("MOLT_DIFF_CARGO_TARGET_DIR", "").strip()
    if raw:
        root = Path(raw).expanduser()
    else:
        cargo_target = os.environ.get("CARGO_TARGET_DIR", "").strip()
        if cargo_target:
            root = Path(cargo_target).expanduser()
        else:
            artifact_root = os.environ.get("MOLT_EXT_ROOT", "").strip()
            if artifact_root:
                root = Path(artifact_root).expanduser() / "target"
            else:
                root = _repo_root() / "target"
    root.mkdir(parents=True, exist_ok=True)
    return root


def _diff_cache_root() -> Path:
    artifact_root = os.environ.get("MOLT_EXT_ROOT", "").strip()
    if artifact_root:
        root = Path(artifact_root).expanduser() / ".molt_cache"
    else:
        root = _repo_root() / ".molt_cache"
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
            timeout=2.0,
        )
    except (OSError, subprocess.TimeoutExpired):
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
            timeout=2.0,
        )
    except (OSError, subprocess.TimeoutExpired):
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


@dataclass(frozen=True)
class _BackendDaemonProcess:
    pid: int
    socket_path: Path
    command: str


def _list_backend_daemon_processes() -> list[_BackendDaemonProcess]:
    processes: list[_BackendDaemonProcess] = []
    if os.name != "posix":
        return processes
    try:
        result = subprocess.run(
            ["ps", "-axo", "pid=,command="],
            capture_output=True,
            text=True,
            check=False,
            timeout=2.0,
        )
    except (OSError, subprocess.TimeoutExpired):
        return processes
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
        processes.append(
            _BackendDaemonProcess(pid=pid, socket_path=socket_path, command=cmd)
        )
    return sorted(processes, key=lambda process: process.pid)


def _current_session_backend_daemon_records_by_pid() -> dict[
    int, daemon_custody.BackendDaemonIdentityRecord
]:
    session_id = os.environ.get("MOLT_SESSION_ID", "").strip()
    if not session_id:
        return {}
    return {
        record.identity.pid: record
        for record in daemon_custody.iter_backend_daemon_identity_records(
            _diff_backend_daemon_root(),
            session_id=session_id,
        )
    }


def _verified_backend_daemon_record(
    process: _BackendDaemonProcess,
    records_by_pid: Mapping[int, daemon_custody.BackendDaemonIdentityRecord],
) -> daemon_custody.BackendDaemonIdentityRecord | None:
    record = records_by_pid.get(process.pid)
    if record is None:
        return None
    identity = record.identity
    if identity.socket_path != process.socket_path:
        return None
    if not daemon_custody.backend_daemon_command_matches_identity(
        process.command,
        backend_bin=identity.backend_bin,
        socket_path=identity.socket_path,
    ):
        return None
    return record


def _terminate_verified_backend_daemon(
    process: _BackendDaemonProcess,
    records_by_pid: Mapping[int, daemon_custody.BackendDaemonIdentityRecord],
    *,
    grace: float = 0.75,
) -> bool:
    record = _verified_backend_daemon_record(process, records_by_pid)
    if record is None:
        return False

    def process_command(pid: int) -> str | None:
        return process.command if pid == process.pid else None

    if not daemon_custody.terminate_backend_daemon_identity(
        record.identity,
        grace=grace,
        process_command=process_command,
        pid_alive=_pid_alive,
    ):
        return False
    daemon_custody.remove_backend_daemon_identity(record.path)
    return True


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
            timeout=2.0,
        )
    except (OSError, subprocess.TimeoutExpired):
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
            timeout=2.0,
        )
    except (OSError, subprocess.TimeoutExpired):
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
    if "internal-batch-build-server" in cmd and (
        "-m molt.cli" in cmd or "src/molt/cli/__init__.py" in cmd
    ):
        return True
    # Restrict to helper processes tied to diff temp dirs so we don't interfere
    # with unrelated local build activity.
    if "/molt_diff_" not in cmd:
        return False
    if "molt-backend" in cmd and "--output" in cmd and "--daemon" not in cmd:
        return True
    if ("-m molt.cli build " in cmd) or ("src/molt/cli/__init__.py build " in cmd):
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
    processes_by_socket: dict[Path, list[_BackendDaemonProcess]] = {}
    for process in _list_backend_daemon_processes():
        if not _pid_alive(process.pid):
            continue
        processes_by_socket.setdefault(process.socket_path, []).append(process)
    records_by_pid = _current_session_backend_daemon_records_by_pid()
    for socket_path, processes in processes_by_socket.items():
        verified = [
            process
            for process in processes
            if _verified_backend_daemon_record(process, records_by_pid) is not None
        ]
        if not verified:
            continue
        if max_rss_kb > 0:
            filtered: list[_BackendDaemonProcess] = []
            for process in verified:
                rss_kb, _age_sec = _pid_rss_age(process.pid)
                if rss_kb is not None and rss_kb > max_rss_kb:
                    if _terminate_verified_backend_daemon(process, records_by_pid):
                        print(
                            "[INFO] Pruned backend daemon pid="
                            f"{process.pid} rss={rss_kb}KB (> {max_rss_kb}KB)"
                        )
                    continue
                filtered.append(process)
            verified = [process for process in filtered if _pid_alive(process.pid)]
            if not verified:
                continue
        if not socket_path.exists():
            for process in verified:
                _terminate_verified_backend_daemon(process, records_by_pid)
            continue
        if len(verified) > 1:
            # Keep the newest pid; terminate duplicate daemons bound to the same socket.
            for process in verified[:-1]:
                _terminate_verified_backend_daemon(process, records_by_pid)
            verified = verified[-1:]
        ping_ok = _daemon_ping(socket_path)
        if not ping_ok:
            process = verified[0]
            _rss_kb, age_sec = _pid_rss_age(process.pid)
            if age_sec is not None and age_sec >= unresponsive_stale_sec:
                if _terminate_verified_backend_daemon(process, records_by_pid):
                    print(
                        "[INFO] Pruned stale unresponsive backend daemon pid="
                        f"{process.pid} age={age_sec}s socket={socket_path}"
                    )

    daemon_root = _diff_backend_daemon_root()
    if not daemon_root.exists():
        return
    daemon_custody.remove_legacy_pid_files(daemon_root)


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
    if not raw:
        return True
    return raw not in {"0", "false", "no", "off"}


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


def _diff_batch_compile_server_enabled() -> bool:
    explicit = _bool_env("MOLT_DIFF_BATCH_COMPILE_SERVER")
    if explicit is not None:
        return explicit
    return False


def _diff_batch_compile_server_strict() -> bool:
    explicit = _bool_env("MOLT_DIFF_BATCH_COMPILE_SERVER_STRICT")
    if explicit is not None:
        return explicit
    return False


def _diff_batch_compile_server_request_timeout(
    build_timeout: float | None = None,
) -> float:
    raw = os.environ.get("MOLT_DIFF_BATCH_COMPILE_SERVER_TIMEOUT_SEC", "").strip()
    if raw:
        try:
            parsed = float(raw)
            if parsed > 0:
                return parsed
        except ValueError:
            pass
    if build_timeout is not None and build_timeout > 0:
        return build_timeout
    return 60.0


def _batch_compile_server_disable_cooldown_sec() -> float:
    raw = _parse_float_env("MOLT_DIFF_BATCH_COMPILE_SERVER_DISABLE_COOLDOWN_SEC")
    if raw is None:
        return 30.0
    return max(0.0, raw)


def _batch_compile_server_disable_after_failures() -> int:
    return max(
        1,
        _parse_int_env(
            "MOLT_DIFF_BATCH_COMPILE_SERVER_DISABLE_AFTER_FAILURES",
            2,
        ),
    )


def _batch_compile_server_mark_disabled(reason: str) -> None:
    global _BATCH_COMPILE_SERVER_DISABLED_UNTIL
    global _BATCH_COMPILE_SERVER_DISABLE_REASON
    global _BATCH_COMPILE_SERVER_FAILURE_COUNT
    _BATCH_COMPILE_SERVER_FAILURE_COUNT += 1
    cooldown = _batch_compile_server_disable_cooldown_sec()
    _BATCH_COMPILE_SERVER_DISABLE_REASON = reason.strip()
    if (
        _BATCH_COMPILE_SERVER_FAILURE_COUNT
        < _batch_compile_server_disable_after_failures()
    ):
        _BATCH_COMPILE_SERVER_DISABLED_UNTIL = 0.0
        return
    if cooldown <= 0:
        _BATCH_COMPILE_SERVER_DISABLED_UNTIL = 0.0
    else:
        _BATCH_COMPILE_SERVER_DISABLED_UNTIL = time.monotonic() + cooldown


def _batch_compile_server_reset_disabled() -> None:
    global _BATCH_COMPILE_SERVER_DISABLED_UNTIL
    global _BATCH_COMPILE_SERVER_DISABLE_REASON
    global _BATCH_COMPILE_SERVER_FAILURE_COUNT
    _BATCH_COMPILE_SERVER_DISABLED_UNTIL = 0.0
    _BATCH_COMPILE_SERVER_DISABLE_REASON = ""
    _BATCH_COMPILE_SERVER_FAILURE_COUNT = 0


def _batch_compile_server_disabled_message() -> str | None:
    remaining = _BATCH_COMPILE_SERVER_DISABLED_UNTIL - time.monotonic()
    if remaining <= 0:
        if _BATCH_COMPILE_SERVER_DISABLED_UNTIL > 0:
            _batch_compile_server_reset_disabled()
        return None
    if _BATCH_COMPILE_SERVER_DISABLE_REASON:
        return (
            "batch compile server temporarily disabled for this worker "
            f"({remaining:.1f}s remaining): {_BATCH_COMPILE_SERVER_DISABLE_REASON}"
        )
    return (
        "batch compile server temporarily disabled for this worker "
        f"({remaining:.1f}s remaining)"
    )


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
    # Throughput-first default: keep cache enabled unless explicitly overridden.
    # Dyld corruption handling is enforced by the retry/quarantine pipeline
    # (daemon-off + --no-cache + rebuild + isolated target fallback) only
    # when a real incident is detected.
    return False


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


def _diff_stdlib_profile(env: dict[str, str]) -> tuple[str | None, str | None]:
    raw = env.get("MOLT_DIFF_STDLIB_PROFILE", "").strip().lower()
    if not raw:
        return None, None
    if raw not in {"micro", "full"}:
        return None, "MOLT_DIFF_STDLIB_PROFILE must be 'micro' or 'full'"
    return raw, None


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


@dataclass(frozen=True, slots=True)
class _DiffMemoryGuardConfig:
    max_process_kb: int
    max_tree_kb: int
    global_kb: int
    poll_interval: float
    child_rlimit_kb: int | None = None
    dynamic_process_rss: bool = False
    dynamic_tree_rss: bool = False
    dynamic_global_rss: bool = False
    dynamic_child_rlimit: bool = False

    @property
    def max_process_gb(self) -> float:
        return self.max_process_kb / (1024 * 1024)

    @property
    def max_tree_gb(self) -> float:
        return self.max_tree_kb / (1024 * 1024)

    @property
    def global_gb(self) -> float:
        return self.global_kb / (1024 * 1024)

    @property
    def child_rlimit_gb(self) -> float | None:
        if self.child_rlimit_kb is None:
            return None
        return self.child_rlimit_kb / (1024 * 1024)


def _bounded_positive_float_env(
    name: str, *, default: float, upper: float | None = None
) -> float:
    value = _parse_float_env(name)
    if value is None or value <= 0:
        value = default
    if upper is not None:
        value = min(value, upper)
    return max(0.001, value)


def _diff_memory_guard_limits(
    env: Mapping[str, str] | None = None,
) -> harness_memory_guard.HarnessMemoryLimits:
    limits = harness_memory_guard.limits_from_env("MOLT_DIFF", env)
    if limits.enabled:
        return limits
    return harness_memory_guard.HarnessMemoryLimits(
        enabled=True,
        max_process_rss_gb=limits.max_process_rss_gb,
        max_total_rss_gb=limits.max_total_rss_gb,
        max_global_rss_gb=limits.max_global_rss_gb,
        poll_interval=limits.poll_interval,
        child_rlimit_gb=limits.child_rlimit_gb,
        adaptive_prefix=limits.adaptive_prefix,
        dynamic_process_rss=limits.dynamic_process_rss,
        dynamic_total_rss=limits.dynamic_total_rss,
        dynamic_global_rss=limits.dynamic_global_rss,
        dynamic_child_rlimit=limits.dynamic_child_rlimit,
    )


def _diff_memory_guard_config(*, accounted_rss_kb: int = 0) -> _DiffMemoryGuardConfig:
    limits = _diff_memory_guard_limits()
    accounted_rss_kb = max(0, accounted_rss_kb)
    current_limits = limits.current_memory_limits(accounted_rss_kb=accounted_rss_kb)
    global_limit_kb = (
        limits.max_global_rss_kb
        if current_limits.max_global_rss_kb is None
        else current_limits.max_global_rss_kb
    )
    tree_limit_kb = (
        limits.max_total_rss_kb
        if current_limits.max_total_rss_kb is None
        else current_limits.max_total_rss_kb
    )
    global_kb = min(global_limit_kb, _DIFF_MEMORY_GUARD_HARD_GLOBAL_KB)
    tree_kb = min(tree_limit_kb, global_kb)
    process_kb = min(current_limits.max_process_rss_kb, tree_kb)
    return _DiffMemoryGuardConfig(
        max_process_kb=process_kb,
        max_tree_kb=tree_kb,
        global_kb=global_kb,
        child_rlimit_kb=limits.current_child_rlimit_kb(
            accounted_rss_kb=accounted_rss_kb
        ),
        poll_interval=min(2.0, max(0.01, limits.poll_interval)),
        dynamic_process_rss=limits.dynamic_process_rss,
        dynamic_tree_rss=limits.dynamic_total_rss,
        dynamic_global_rss=limits.dynamic_global_rss,
        dynamic_child_rlimit=limits.dynamic_child_rlimit,
    )


def _diff_memory_guard_enabled() -> bool:
    return True


def _diff_memory_guard_sample_interval_sec() -> float:
    return _bounded_positive_float_env(
        "MOLT_DIFF_MEMORY_GUARD_SAMPLE_INTERVAL_SEC",
        default=_DIFF_MEMORY_GUARD_DEFAULT_SAMPLE_INTERVAL_SEC,
        upper=60.0,
    )


def _diff_memory_guard_write_samples() -> bool:
    explicit = _bool_env("MOLT_DIFF_MEMORY_GUARD_WRITE_SAMPLES")
    return True if explicit is None else explicit


def _diff_memory_guard_stream_mode() -> str:
    raw = os.environ.get("MOLT_DIFF_MEMORY_GUARD_STREAM", "").strip().lower()
    if raw in {"", "0", "false", "no", "off", "none"}:
        return ""
    if raw in {"1", "true", "yes", "on", "stderr"}:
        return "stderr"
    if raw in {"stdout", "json", "json-stdout", "stderr-json", "json-stderr"}:
        return raw
    return ""


def _diff_memory_guard_stream_target(mode: str) -> io.TextIOBase | None:
    if not mode:
        return None
    return sys.stdout if "stdout" in mode else sys.stderr


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
    # Default to the adaptive child resource budget. RSS enforcement remains
    # process/tree/global sampling; this layer bounds inherited virtual memory.
    child_rlimit_kb = _diff_memory_guard_config().child_rlimit_kb
    return None if child_rlimit_kb is None else child_rlimit_kb * 1024


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
    memory_guard._apply_child_resource_limit(max(1, limit // 1024))
    _MEM_LIMIT_APPLIED = True


def _default_jobs() -> int:
    count = os.cpu_count() or 1
    guard = _diff_memory_guard_config()
    count = min(count, _memory_guard_max_jobs(guard))
    max_jobs = os.environ.get("MOLT_DIFF_MAX_JOBS", "").strip()
    if max_jobs.isdigit():
        count = min(count, max(1, int(max_jobs)))
    return max(1, count)


def _memory_guard_scheduler_per_job_gb(config: _DiffMemoryGuardConfig) -> float:
    return _diff_resource_pressure_plan(config).diff_scheduler_per_job_gb


def _diff_resource_pressure_plan(
    config: _DiffMemoryGuardConfig,
) -> resource_pressure.ResourcePressurePlan:
    budget = memory_guard.AdaptiveMemoryBudget(
        max_process_rss_gb=config.max_process_gb,
        max_total_rss_gb=config.max_tree_gb,
        max_global_rss_gb=config.global_gb,
        reserve_gb=0.0,
        physical_gb=None,
        available_gb=config.global_gb,
        source="guard_limits",
    )
    return resource_pressure.plan_resource_pressure(
        prefix="MOLT_DIFF",
        environ=os.environ,
        cpu_count=max(1, os.cpu_count() or 1),
        budget=budget,
        diff_tree_gb=config.max_tree_gb,
        diff_global_gb=config.global_gb,
        diff_mem_per_job_gb=_parse_float_env("MOLT_DIFF_MEM_PER_JOB_GB"),
    )


def _memory_guard_max_jobs(config: _DiffMemoryGuardConfig) -> int:
    return _diff_resource_pressure_plan(config).diff_max_jobs


def _config_payload(config: _DiffMemoryGuardConfig) -> dict[str, object]:
    pressure_plan = _diff_resource_pressure_plan(config)
    return {
        "max_process_gb": config.max_process_gb,
        "max_tree_gb": config.max_tree_gb,
        "global_gb": config.global_gb,
        "child_rlimit_gb": config.child_rlimit_gb,
        "poll_interval": config.poll_interval,
        "scheduler_per_job_gb": pressure_plan.diff_scheduler_per_job_gb,
        "resource_pressure": pressure_plan.to_json_dict(),
        "dynamic_process_rss": config.dynamic_process_rss,
        "dynamic_tree_rss": config.dynamic_tree_rss,
        "dynamic_global_rss": config.dynamic_global_rss,
        "dynamic_child_rlimit": config.dynamic_child_rlimit,
    }


def _refresh_memory_guard_config(
    config: _DiffMemoryGuardConfig, *, accounted_rss_kb: int
) -> _DiffMemoryGuardConfig:
    if not (
        config.dynamic_process_rss
        or config.dynamic_tree_rss
        or config.dynamic_global_rss
        or config.dynamic_child_rlimit
    ):
        return config
    return _diff_memory_guard_config(accounted_rss_kb=accounted_rss_kb)


def _constrain_jobs_for_memory_guard(
    jobs: int, *, config: _DiffMemoryGuardConfig, log: bool = True
) -> int:
    safe_jobs = _memory_guard_max_jobs(config)
    if jobs <= safe_jobs:
        return max(1, jobs)
    if log:
        print(
            "[MEMORY-GUARD] "
            f"Clamping molt_diff jobs from {jobs} to {safe_jobs} "
            f"(global={config.global_gb:.2f}GB "
            f"per_job={_memory_guard_scheduler_per_job_gb(config):.2f}GB)."
        )
    return safe_jobs


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
    return harness_memory_guard.batch_process_group_kwargs(_diff_memory_guard_limits())


def _diff_memory_guard_root() -> Path:
    root = _diff_root() / "memory_guard"
    root.mkdir(parents=True, exist_ok=True)
    return root


def _diff_memory_guard_trip_file() -> Path:
    raw = os.environ.get(_DIFF_MEMORY_GUARD_TRIP_FILE_ENV, "").strip()
    path = Path(raw).expanduser() if raw else _diff_memory_guard_root() / "tripped.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    return path


def _diff_memory_guard_events_jsonl() -> Path:
    raw = os.environ.get(_DIFF_MEMORY_GUARD_EVENTS_JSONL_ENV, "").strip()
    path = Path(raw).expanduser() if raw else _diff_memory_guard_root() / "events.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    return path


def _diff_memory_guard_global_samples_jsonl() -> Path:
    raw = os.environ.get(_DIFF_MEMORY_GUARD_GLOBAL_SAMPLES_JSONL_ENV, "").strip()
    path = (
        Path(raw).expanduser()
        if raw
        else _diff_memory_guard_root() / "global_samples.jsonl"
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    return path


def _memory_guard_jsonl_max_bytes(path: Path) -> int | None:
    if path.name.startswith("global_samples"):
        env_name = "MOLT_DIFF_MEMORY_GUARD_MAX_SAMPLE_MB"
        default_mb = _DIFF_MEMORY_GUARD_DEFAULT_SAMPLE_MAX_MB
    else:
        env_name = "MOLT_DIFF_MEMORY_GUARD_MAX_EVENT_MB"
        default_mb = _DIFF_MEMORY_GUARD_DEFAULT_EVENT_MAX_MB
    value = _parse_float_env(env_name)
    if value is None:
        value = default_mb
    if value <= 0:
        return None
    return max(1024, int(value * 1024 * 1024))


def _rotate_memory_guard_jsonl(path: Path, incoming_bytes: int) -> None:
    max_bytes = _memory_guard_jsonl_max_bytes(path)
    if max_bytes is None:
        return
    try:
        current_size = path.stat().st_size
    except FileNotFoundError:
        return
    except OSError:
        return
    if current_size + incoming_bytes <= max_bytes:
        return
    rotated = path.with_name(f"{path.name}.1")
    with contextlib.suppress(OSError):
        rotated.unlink()
    with contextlib.suppress(OSError):
        path.replace(rotated)


def _append_memory_guard_jsonl(path: Path, payload: dict[str, object]) -> None:
    payload = {"ts": time.time(), **payload}
    try:
        line = json.dumps(payload, sort_keys=True) + "\n"
        _rotate_memory_guard_jsonl(path, len(line.encode("utf-8")))
        with path.open("a", encoding="utf-8") as handle:
            handle.write(line)
    except OSError:
        return


def _memory_guard_payload_gb(payload: dict[str, object], key: str) -> str:
    value = payload.get(key)
    if isinstance(value, (int, float)):
        return f"{value:.2f}GB"
    return "-"


def _memory_guard_record_gb(record: object) -> str:
    if not isinstance(record, dict):
        return "-"
    return _memory_guard_payload_gb(record, "rss_gb")


def _memory_guard_peak_tree_gb(payload: dict[str, object]) -> str:
    trees = payload.get("trees")
    if not isinstance(trees, list):
        return "-"
    values: list[float] = []
    for tree in trees:
        if not isinstance(tree, dict):
            continue
        total = tree.get("total")
        if not isinstance(total, dict):
            continue
        rss_gb = total.get("rss_gb")
        if isinstance(rss_gb, (int, float)):
            values.append(float(rss_gb))
    if not values:
        return "-"
    return f"{max(values):.2f}GB"


def _format_memory_guard_stream(payload: dict[str, object]) -> str:
    event = str(payload.get("event", "sample"))
    if event == "sample":
        roots = payload.get("active_roots")
        root_count = len(roots) if isinstance(roots, list) else 0
        return (
            "[MEMORY-GUARD] "
            f"sample total={_memory_guard_payload_gb(payload, 'total_gb')} "
            f"roots={root_count} peak_tree={_memory_guard_peak_tree_gb(payload)}"
        )
    if event in {"guard_tripped", "subprocess_guard_tripped", "memory_guard_trip"}:
        message = payload.get("message")
        if isinstance(message, str) and message:
            return f"[MEMORY-GUARD] TRIP {message}"
        return (
            "[MEMORY-GUARD] TRIP "
            f"violation={_memory_guard_record_gb(payload.get('violation'))}"
        )
    if event == "run_started":
        return (
            "[MEMORY-GUARD] "
            f"run_started global={_memory_guard_payload_gb(payload, 'global_gb')} "
            f"tree={_memory_guard_payload_gb(payload, 'max_tree_gb')} "
            f"process={_memory_guard_payload_gb(payload, 'max_process_gb')} "
            f"child_rlimit={_memory_guard_payload_gb(payload, 'child_rlimit_gb')} "
            f"poll={payload.get('poll_interval')}s "
            f"samples={'on' if _diff_memory_guard_write_samples() else 'off'}"
        )
    if event == "monitor_error":
        return f"[MEMORY-GUARD] monitor_error {payload.get('error', '')}"
    return f"[MEMORY-GUARD] {event}"


def _stream_memory_guard_payload(payload: dict[str, object]) -> None:
    mode = _diff_memory_guard_stream_mode()
    target = _diff_memory_guard_stream_target(mode)
    if target is None:
        return
    try:
        if "json" in mode:
            line = json.dumps({"ts": time.time(), **payload}, sort_keys=True)
        else:
            line = _format_memory_guard_stream(payload)
        print(line, file=target, flush=True)
    except Exception:
        return


def _record_memory_guard_event(payload: dict[str, object]) -> None:
    _append_memory_guard_jsonl(_diff_memory_guard_events_jsonl(), payload)
    _stream_memory_guard_payload(payload)


def _record_memory_guard_sample(payload: dict[str, object]) -> None:
    if _diff_memory_guard_write_samples():
        _append_memory_guard_jsonl(_diff_memory_guard_global_samples_jsonl(), payload)
    _stream_memory_guard_payload(payload)


def _memory_guard_record(
    record: memory_guard.RssViolation | None,
) -> dict[str, object] | None:
    if record is None:
        return None
    return {
        "pid": record.pid,
        "rss_kb": record.rss_kb,
        "rss_gb": record.rss_gb,
        "command": record.command,
        "scope": record.scope,
    }


def _memory_guard_message(
    violation: memory_guard.RssViolation,
    *,
    limit_gb: float,
    phase: str,
) -> str:
    return (
        "molt_diff memory guard: RSS limit exceeded "
        f"phase={phase} scope={violation.scope} pid={violation.pid} "
        f"rss={violation.rss_gb:.2f}GB limit={limit_gb:.2f}GB "
        f"command={violation.command}"
    )


def _mark_memory_guard_tripped(payload: dict[str, object]) -> None:
    trip_file = _diff_memory_guard_trip_file()
    data = {"ts": time.time(), **payload}
    tmp_path = trip_file.with_name(f"{trip_file.name}.{os.getpid()}.tmp")
    try:
        tmp_path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n")
        tmp_path.replace(trip_file)
    except OSError:
        with contextlib.suppress(OSError):
            tmp_path.unlink()
    _record_memory_guard_event(data)


def _memory_guard_trip_message() -> str | None:
    trip_file = _diff_memory_guard_trip_file()
    if not trip_file.exists():
        return None
    try:
        payload = json.loads(trip_file.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return "molt_diff memory guard: global guard tripped"
    message = payload.get("message")
    if isinstance(message, str) and message:
        return message
    reason = payload.get("reason")
    if isinstance(reason, str) and reason:
        return f"molt_diff memory guard: {reason}"
    return "molt_diff memory guard: global guard tripped"


def _prepare_memory_guard_run(config: _DiffMemoryGuardConfig) -> None:
    global _LAST_SENTINEL_SAMPLE_WRITE
    _LAST_SENTINEL_SAMPLE_WRITE = 0.0
    guard_root = _diff_memory_guard_root()
    os.environ[_DIFF_MEMORY_GUARD_TRIP_FILE_ENV] = str(guard_root / "tripped.json")
    os.environ[_DIFF_MEMORY_GUARD_EVENTS_JSONL_ENV] = str(guard_root / "events.jsonl")
    os.environ[_DIFF_MEMORY_GUARD_GLOBAL_SAMPLES_JSONL_ENV] = str(
        guard_root / "global_samples.jsonl"
    )
    with contextlib.suppress(OSError):
        Path(os.environ[_DIFF_MEMORY_GUARD_TRIP_FILE_ENV]).unlink()
    _record_memory_guard_event(
        {
            "event": "run_started",
            **_config_payload(config),
            "sample_interval": _diff_memory_guard_sample_interval_sec(),
            "write_samples": _diff_memory_guard_write_samples(),
            "stream_mode": _diff_memory_guard_stream_mode(),
            "event_max_bytes": _memory_guard_jsonl_max_bytes(
                Path(os.environ[_DIFF_MEMORY_GUARD_EVENTS_JSONL_ENV])
            ),
            "sample_max_bytes": _memory_guard_jsonl_max_bytes(
                Path(os.environ[_DIFF_MEMORY_GUARD_GLOBAL_SAMPLES_JSONL_ENV])
            ),
        }
    )


_LAST_SENTINEL_SAMPLE_WRITE = 0.0


def _process_sample_record(
    sample: memory_guard.ProcessSample | None,
) -> dict[str, object] | None:
    if sample is None:
        return None
    return _memory_guard_record(
        memory_guard.RssViolation(
            pid=sample.pid,
            rss_kb=sample.rss_kb,
            command=sample.command,
            scope="process",
        )
    )


def _process_group_record(
    group: process_sentinel.ProcessGroup,
) -> dict[str, object]:
    peak = group.peak
    command = group.command_text if peak is None else peak.command
    return {
        "pgid": group.pgid,
        "pids": group.pids,
        "peak": _process_sample_record(peak),
        "total": _memory_guard_record(
            memory_guard.RssViolation(
                pid=group.pgid,
                rss_kb=group.total_rss_kb,
                command=command,
                scope="process_group",
            )
        ),
    }


def _record_memory_guard_sentinel_sample(
    groups: Sequence[process_sentinel.ProcessGroup],
    limits: memory_guard.ResolvedMemoryLimits,
    _elapsed_s: float,
) -> None:
    global _LAST_SENTINEL_SAMPLE_WRITE
    if not groups:
        return
    now = time.monotonic()
    if now - _LAST_SENTINEL_SAMPLE_WRITE < _diff_memory_guard_sample_interval_sec():
        return
    _LAST_SENTINEL_SAMPLE_WRITE = now
    total_kb = sum(group.total_rss_kb for group in groups)
    _record_memory_guard_sample(
        {
            "event": "sample",
            "active_roots": [group.pgid for group in groups],
            "total_kb": total_kb,
            "total_gb": total_kb / (1024 * 1024),
            "limits": memory_guard.memory_limits_payload(limits),
            "trees": [_process_group_record(group) for group in groups],
        }
    )


def _sentinel_violation_as_rss(
    violation: process_sentinel.SentinelViolation,
    payload: Mapping[str, object],
) -> memory_guard.RssViolation:
    if violation.reason == "process_rss":
        return memory_guard.RssViolation(
            pid=0 if violation.peak_pid is None else violation.peak_pid,
            rss_kb=violation.total_rss_kb
            if violation.peak_rss_kb is None
            else violation.peak_rss_kb,
            command=violation.command,
            scope="process",
        )
    if violation.reason == "global_rss":
        global_total = payload.get("global_total_kb")
        rss_kb = violation.total_rss_kb
        if isinstance(global_total, int):
            rss_kb = global_total
        return memory_guard.RssViolation(
            pid=0,
            rss_kb=rss_kb,
            command="all repo-scoped molt_diff process groups",
            scope="diff_global_process_groups",
        )
    return memory_guard.RssViolation(
        pid=violation.pgid,
        rss_kb=violation.total_rss_kb,
        command=violation.command,
        scope="process_tree",
    )


def _record_memory_guard_sentinel_violation(
    violation: process_sentinel.SentinelViolation,
    limits: memory_guard.ResolvedMemoryLimits,
    payload: Mapping[str, object],
) -> None:
    rss_violation = _sentinel_violation_as_rss(violation, payload)
    if violation.reason == "global_rss":
        reason = "global memory guard tripped"
        phase = "global"
        limit_gb = limits.max_global_rss_gb
    elif violation.reason == "process_rss":
        reason = "per-process memory guard tripped"
        phase = f"pgid={violation.pgid}"
        limit_gb = limits.max_process_rss_gb
    else:
        reason = "per-tree memory guard tripped"
        phase = f"pgid={violation.pgid}"
        limit_gb = limits.max_total_rss_gb
    active_roots = payload.get("active_pgids")
    roots = active_roots if isinstance(active_roots, list) else [violation.pgid]
    global_total_kb = payload.get("global_total_kb", violation.total_rss_kb)
    if not isinstance(global_total_kb, int):
        global_total_kb = violation.total_rss_kb
    _mark_memory_guard_tripped(
        {
            "event": "guard_tripped",
            "reason": reason,
            "message": _memory_guard_message(
                rss_violation,
                limit_gb=limits.max_process_rss_gb if limit_gb is None else limit_gb,
                phase=phase,
            ),
            "violation": _memory_guard_record(rss_violation),
            "global_total_kb": global_total_kb,
            "global_total_gb": global_total_kb / (1024 * 1024),
            "active_roots": roots,
            "shared_sentinel_event": dict(payload),
        }
    )


def _force_close_diff_process_group(proc: subprocess.Popen[str]) -> None:
    harness_memory_guard.force_close_process_group(proc)


def _run_subprocess(
    cmd: list[str], *, env: dict[str, str], timeout: float | None
) -> subprocess.CompletedProcess[str]:
    trip_message = _memory_guard_trip_message()
    if trip_message is not None:
        return subprocess.CompletedProcess(
            cmd,
            _DIFF_MEMORY_GUARD_RETURN_CODE,
            "",
            trip_message + "\n",
        )

    guard_context = harness_memory_guard.HarnessExecutionContext.from_env(
        "MOLT_DIFF",
        env,
        repo_root=Path(_repo_root()),
        artifact_root=_diff_root(),
        limits=_diff_memory_guard_limits(env),
    )
    result = guard_context.run(
        cmd,
        cwd=Path(_repo_root()),
        timeout=timeout,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="surrogateescape",
    )
    stdout = "" if result.stdout is None else result.stdout
    stderr = "" if result.stderr is None else result.stderr
    if getattr(result, "timed_out", False):
        raise subprocess.TimeoutExpired(
            cmd=cmd,
            timeout=timeout,
            output=stdout,
            stderr=stderr,
        )
    returncode = result.returncode
    trip_message = _memory_guard_trip_message()
    if trip_message is not None:
        returncode = _DIFF_MEMORY_GUARD_RETURN_CODE
        if trip_message not in stderr:
            stderr = f"{stderr}{trip_message}\n"
    elif returncode == _DIFF_MEMORY_GUARD_RETURN_CODE:
        guard_message = (
            "molt_diff memory guard: RSS limit exceeded under shared harness "
            "subprocess guard; inspect preceding memory_guard diagnostic for "
            "pid/rss/limit details"
        )
        _record_memory_guard_event(
            {
                "event": "subprocess_guard_tripped",
                "message": guard_message,
                "command": cmd,
                "violation": _memory_guard_record(getattr(result, "violation", None)),
            }
        )
        stderr = f"{stderr}{guard_message}\n"
    return subprocess.CompletedProcess(cmd, returncode, stdout, stderr)


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


class _BatchCompileServerClient(BatchCompileServerClient):
    def __init__(self, env: dict[str, str]) -> None:
        cmd = [
            _resolve_molt_cli_python(),
            "-m",
            "molt.cli",
            "internal-batch-build-server",
        ]
        guard_context = harness_memory_guard.HarnessExecutionContext.from_env(
            "MOLT_DIFF",
            env,
            repo_root=Path(_repo_root()),
            limits=_diff_memory_guard_limits(env),
        )
        super().__init__(
            cmd,
            cwd=Path(_repo_root()),
            env=env,
            reader_name="molt-diff-batch-server-reader",
            guard_context=guard_context,
            force_close=_force_close_diff_process_group,
        )

    def close(self, *, force: bool = False, timeout: float | None = None) -> None:
        request_timeout = timeout
        if request_timeout is None:
            request_timeout = _diff_batch_compile_server_request_timeout()
        if not force and self._proc.poll() is None:
            with contextlib.suppress(Exception):
                self.request("shutdown", timeout=request_timeout)
        self.force_close()


def _shutdown_batch_compile_server(*, force: bool = True) -> None:
    global _BATCH_COMPILE_SERVER_CLIENT
    global _BATCH_COMPILE_SERVER_CLIENT_PID
    client = _BATCH_COMPILE_SERVER_CLIENT
    if client is None:
        return
    _BATCH_COMPILE_SERVER_CLIENT = None
    _BATCH_COMPILE_SERVER_CLIENT_PID = 0
    client.close(force=force)


def _batch_compile_server_client(
    env: dict[str, str],
    *,
    request_timeout: float,
) -> tuple[_BatchCompileServerClient | None, str | None]:
    global _BATCH_COMPILE_SERVER_CLIENT
    global _BATCH_COMPILE_SERVER_CLIENT_PID
    disabled_message = _batch_compile_server_disabled_message()
    if disabled_message is not None:
        return None, disabled_message
    pid = os.getpid()
    if _BATCH_COMPILE_SERVER_CLIENT_PID != pid:
        _shutdown_batch_compile_server(force=True)
    if _BATCH_COMPILE_SERVER_CLIENT is not None:
        return _BATCH_COMPILE_SERVER_CLIENT, None
    client: _BatchCompileServerClient | None = None
    try:
        client = _BatchCompileServerClient(env)
        response = client.request(
            "ping",
            timeout=request_timeout,
        )
        if not bool(response.get("ok")) or not bool(response.get("pong")):
            raise RuntimeError("batch compile server ping failed")
    except Exception as exc:
        if client is not None:
            with contextlib.suppress(Exception):
                client.close(force=True)
        _batch_compile_server_mark_disabled(str(exc))
        return None, str(exc)
    _batch_compile_server_reset_disabled()
    _BATCH_COMPILE_SERVER_CLIENT = client
    _BATCH_COMPILE_SERVER_CLIENT_PID = pid
    atexit.register(_shutdown_batch_compile_server)
    return client, None


def _run_batch_compile_build(
    *,
    env: dict[str, str],
    file_path: str,
    output_root: Path,
    output_binary: Path,
    build_profile: str,
    no_cache: bool,
    rebuild: bool,
    request_timeout: float,
    strict_mode: bool,
    extra_params: dict[str, object] | None = None,
) -> tuple[int, str, str, str | None]:
    diff_caps = _diff_capabilities(env)
    stdlib_profile, stdlib_profile_error = _diff_stdlib_profile(env)
    if stdlib_profile_error is not None:
        return 2, "", stdlib_profile_error, None
    params: dict[str, object] = {
        "file_path": file_path,
        "profile": build_profile,
        "output": str(output_binary),
        "out_dir": str(output_root),
        "target": "native",
        "emit": "bin",
        "cache": not no_cache and not rebuild,
        "respect_pythonpath": True,
        "env_overrides": env,
        "codec": env.get("MOLT_CODEC", "msgpack"),
    }
    if stdlib_profile is not None:
        params["stdlib_profile"] = stdlib_profile
    if diff_caps:
        params["capabilities"] = diff_caps
    if extra_params:
        params.update(extra_params)
    attempts = 2 if strict_mode else 1
    last_error = "batch compile server request failed"
    for attempt in range(attempts):
        if strict_mode and attempt > 0:
            _batch_compile_server_reset_disabled()
        client, start_error = _batch_compile_server_client(
            env,
            request_timeout=request_timeout,
        )
        if client is None:
            if start_error:
                last_error = start_error
            if strict_mode and attempt + 1 < attempts:
                continue
            return 0, "", "", last_error
        try:
            response = client.request(
                "build",
                params=params,
                timeout=request_timeout,
            )
        except Exception as exc:
            last_error = str(exc)
            _batch_compile_server_mark_disabled(last_error)
            _shutdown_batch_compile_server(force=True)
            if strict_mode and attempt + 1 < attempts:
                continue
            return 0, "", "", last_error
        stdout = response.get("stdout")
        stderr = response.get("stderr")
        error = response.get("error")
        returncode = response.get("returncode")
        if not isinstance(returncode, int):
            returncode = 1 if not bool(response.get("ok")) else 0
        out_text = stdout if isinstance(stdout, str) else ""
        err_text = stderr if isinstance(stderr, str) else ""
        if isinstance(error, str) and error:
            if err_text:
                err_text = f"{err_text}\n{error}"
            else:
                err_text = error
        _batch_compile_server_reset_disabled()
        return returncode, out_text, err_text, None
    return 0, "", "", last_error


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
    tmp_root = output_root / "tmp"
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
        cache_root = _diff_cache_root()
        cache_root.mkdir(parents=True, exist_ok=True)
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
    metadata_error = _apply_metadata_env_overrides(file_path, env)
    if metadata_error is not None:
        _record_rss_metrics(
            file_path,
            build_metrics=None,
            run_metrics=None,
            build_rc=2,
            run_rc=None,
            status="build_invalid_stdlib_profile",
        )
        return None, metadata_error, 2
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
    stdlib_profile, stdlib_profile_error = _diff_stdlib_profile(env)
    if stdlib_profile_error is not None:
        _record_rss_metrics(
            file_path,
            build_metrics=None,
            run_metrics=None,
            build_rc=2,
            run_rc=None,
            status="build_invalid_stdlib_profile",
        )
        return None, stdlib_profile_error, 2
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
        build_stdout = ""
        build_stderr = ""
        build_rc = 0
        build_via_batch_server = False
        batch_requested = _diff_batch_compile_server_enabled()
        batch_strict = _diff_batch_compile_server_strict()
        batch_request_timeout = _diff_batch_compile_server_request_timeout(
            build_timeout
        )
        if batch_requested:
            (
                build_rc,
                build_stdout,
                build_stderr,
                batch_error,
            ) = _run_batch_compile_build(
                env=env,
                file_path=file_path,
                output_root=output_root,
                output_binary=output_binary,
                build_profile=build_profile,
                no_cache=no_cache,
                rebuild=rebuild,
                request_timeout=batch_request_timeout,
                strict_mode=batch_strict,
            )
            if batch_error is None:
                build_via_batch_server = True
            elif batch_strict:
                message = f"Batch compile server strict mode failed: {batch_error}"
                _record_rss_metrics(
                    file_path,
                    build_metrics=None,
                    run_metrics=None,
                    build_rc=127,
                    run_rc=None,
                    status="build_batch_server_error",
                )
                return None, message, 127
            else:
                print(
                    "[WARN] Batch compile server unavailable; falling back to subprocess build: "
                    f"{batch_error}"
                )

        build_cmd = [
            _resolve_molt_cli_python(),
            "-m",
            "molt.cli",
            "build",
            file_path,
            "--build-profile",
            build_profile,
            "--respect-pythonpath",
            "--out-dir",
            str(output_root),
            "--output",
            str(output_binary),
        ]
        # Grant standard capabilities for CPython parity in differential tests.
        # Without these, compiled binaries cannot access the filesystem (tempfile,
        # pathlib, os.path), environment variables, or time functions — causing
        # spurious failures unrelated to the tested semantics.
        diff_caps = _diff_capabilities(env)
        if diff_caps:
            build_cmd.extend(["--capabilities", diff_caps])
        if no_cache:
            build_cmd.append("--no-cache")
        if rebuild:
            build_cmd.append("--rebuild")
        if stdlib_profile is not None:
            build_cmd.extend(["--stdlib-profile", stdlib_profile])
        codec = env.get("MOLT_CODEC")
        if codec:
            build_cmd.extend(["--codec", codec])
        if not build_via_batch_server:
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
            build_rc = build_res.returncode
            build_stdout = build_res.stdout
            build_stderr = build_res.stderr
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
        if build_rc != 0:
            _record_rss_metrics(
                file_path,
                build_metrics=build_metrics,
                run_metrics=None,
                build_rc=build_rc,
                run_rc=None,
                status="build_failed",
            )
            return None, build_stderr or build_stdout, build_rc

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
                build_rc=build_rc,
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
                build_rc=build_rc,
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
                build_rc=build_rc,
                run_rc=125,
                status="run_rss_exceeded",
            )
            return "", message, 125
        run_status = "ok" if run_res.returncode == 0 else "run_failed"
        _record_rss_metrics(
            file_path,
            build_metrics=build_metrics,
            run_metrics=run_metrics,
            build_rc=build_rc,
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


def _aggregate_rss_entries_by_file(
    entries: list[dict[str, object]],
) -> list[dict[str, object]]:
    """Collapse per-attempt RSS entries into one record per test file.

    The diff harness can emit multiple records for a single file when retry lanes
    run (dyld/cache/isolated fallback). Top-RSS reporting should keep max RSS
    per file but use the final status for that file, so pass/fail reporting
    matches the actual differential outcome.
    """

    by_file: dict[str, dict[str, object]] = {}
    for payload in entries:
        file_path = payload.get("file")
        if not isinstance(file_path, str) or not file_path:
            continue

        current = by_file.get(file_path)
        if current is None:
            current = {
                "file": file_path,
                "status": payload.get("status"),
                "build": {},
                "run": {},
                "_last_ts": payload.get("timestamp", 0.0),
            }
            by_file[file_path] = current

        # Final status should come from the latest record for this file.
        ts = payload.get("timestamp", 0.0)
        prev_ts = current.get("_last_ts", 0.0)
        if isinstance(ts, (int, float)) and isinstance(prev_ts, (int, float)):
            if ts >= prev_ts:
                current["status"] = payload.get("status")
                current["_last_ts"] = ts

        def _merge_block(name: str) -> None:
            src = payload.get(name)
            if not isinstance(src, dict):
                return
            dst = current.get(name)
            if not isinstance(dst, dict):
                dst = {}
                current[name] = dst
            for key in ("max_rss", "peak_footprint"):
                value = src.get(key)
                if not isinstance(value, int):
                    continue
                old = dst.get(key)
                if not isinstance(old, int) or value > old:
                    dst[key] = value

        _merge_block("build")
        _merge_block("run")

    out: list[dict[str, object]] = []
    for payload in by_file.values():
        payload.pop("_last_ts", None)
        out.append(payload)
    return out


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

    aggregated = _aggregate_rss_entries_by_file(entries)
    ranked = [entry for entry in aggregated if metric(entry) > 0]
    ranked.sort(key=metric, reverse=True)
    return ranked[: max(0, limit)]


@lru_cache(maxsize=8192)
def _status_lookup_keys(path: str) -> tuple[str, ...]:
    keys: list[str] = []

    def _add(value: str) -> None:
        if value and value not in keys:
            keys.append(value)

    _add(path)
    parsed = Path(path)
    _add(parsed.as_posix())

    with contextlib.suppress(OSError, RuntimeError, ValueError):
        resolved = parsed.resolve()
        _add(str(resolved))
        _add(resolved.as_posix())
        with contextlib.suppress(ValueError):
            rel = resolved.relative_to(_repo_root())
            _add(rel.as_posix())
            _add(str(rel))

    if not parsed.is_absolute():
        with contextlib.suppress(OSError, RuntimeError, ValueError):
            from_cwd = (Path.cwd() / parsed).resolve()
            _add(str(from_cwd))
            _add(from_cwd.as_posix())
            with contextlib.suppress(ValueError):
                rel = from_cwd.relative_to(_repo_root())
                _add(rel.as_posix())
                _add(str(rel))

    return tuple(keys)


def _rss_display_status(
    entry: dict[str, object], status_by_path: dict[str, str] | None
) -> str:
    file_path = entry.get("file")
    if status_by_path and isinstance(file_path, str):
        for key in _status_lookup_keys(file_path):
            resolved = status_by_path.get(key)
            if isinstance(resolved, str) and resolved:
                return resolved
    raw = entry.get("status")
    if isinstance(raw, str):
        normalized = raw.strip().lower()
        if normalized in {"ok", "build_only_ok", "pass"}:
            return "pass"
        if normalized in {"skip", "skipped"}:
            return "skip"
        if normalized == "oom":
            return "oom"
        # RSS event statuses are runtime/build telemetry details. If we cannot
        # resolve a final diff status from status_by_path, present any
        # non-pass/non-skip raw status as fail to avoid misleading artifacts
        # like `run_failed` in otherwise green summaries.
        if normalized:
            return "fail"
    return ""


def _print_rss_top(
    run_id: str, limit: int, *, status_by_path: dict[str, str] | None = None
) -> None:
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
            status = _rss_display_status(entry, status_by_path)
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
            status = _rss_display_status(entry, status_by_path)
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
    """Run one differential test and return its RESOLVED status string.

    The resolved status applies the expected-failure (xfail/xpass) overlay so the
    nightly suite stays green for by-design exclusions. The SUITE-HONESTY ratchet
    (tools/check_suite_honesty.py, task #46) needs the RAW status — whether Molt
    actually matched CPython, independent of any expectation overlay — so a
    tracked-but-failing test can never read as green. To avoid a second runner
    (which would drift from these exact comparison semantics), the raw outcome is
    surfaced here, at the single chokepoint that computes it, and appended to the
    optional results sink keyed by MOLT_DIFF_RESULTS_JSONL. The sink is off by
    default and never alters the returned (resolved) status.
    """
    # Mutable record the inner finalizer fills in; emitted once on every exit
    # path (skip / oom / pass / fail). raw_status is authoritative for honesty.
    record: dict[str, object] = {
        "file": _normalize_repo_relative(file_path),
        "raw_status": None,
        "resolved_status": None,
        "reason_tag": None,
        "expect_molt_fail": False,
    }

    def _impl() -> str:
        meta = _collect_meta(file_path)
        manifest_expect_fail = _manifest_marks_expected_failure(file_path)
        explicit_expect_fail = _meta_expect_molt_fail(meta)
        expect_molt_fail = manifest_expect_fail or explicit_expect_fail
        record["expect_molt_fail"] = expect_molt_fail
        expect_fail_reason = _meta_expect_fail_reason(meta)
        if not expect_fail_reason and manifest_expect_fail:
            expect_fail_reason = "too_dynamic_policy"
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
            # skip/oom never reach _finalize_status; record raw==resolved so the
            # honesty sink sees every test, not just the compared ones.
            record["raw_status"] = "skip"
            record["resolved_status"] = "skip"
            return "skip"

        normalize = {v.lower() for v in meta.get("normalize", [])}
        stdout_mode = (meta.get("stdout", ["exact"])[0]).lower()
        stderr_mode = (meta.get("stderr", ["ignore"])[0]).lower()

        print(f"Testing {file_path} against {python_exe}...")
        cp_out, cp_err, cp_ret = run_cpython(file_path, python_exe)

        def _finalize_status(raw_status: str) -> str:
            resolved_status, reason_tag = _resolve_expected_failure_status(
                expect_molt_fail=expect_molt_fail,
                raw_status=raw_status,
                cpython_returncode=cp_ret,
            )
            record["raw_status"] = raw_status
            record["resolved_status"] = resolved_status
            record["reason_tag"] = reason_tag
            if reason_tag == "xfail":
                reason_text = expect_fail_reason or "expected dynamic-semantics gap"
                print(f"[XFAIL] {file_path} ({reason_text})")
            elif reason_tag == "xpass":
                reason_text = expect_fail_reason or "expected dynamic-semantics gap"
                print(f"[XPASS] {file_path} ({reason_text})")
            return resolved_status

        if _should_retry_oom(cp_ret, cp_err):
            print(f"[OOM] {file_path} (cpython)")
            record["raw_status"] = "oom"
            record["resolved_status"] = "oom"
            return "oom"
        if cp_ret != 0 and (
            "msgpack is required for parse_msgpack fallback" in cp_err
            or "cbor2 is required for parse_cbor fallback" in cp_err
        ):
            print(f"[SKIP] {file_path} (missing msgpack/cbor2 in CPython env)")
            record["raw_status"] = "skip"
            record["resolved_status"] = "skip"
            return "skip"
        molt_extra_env = _molt_sys_env_for_python_exe(python_exe)
        molt_out, molt_err, molt_ret = run_molt(
            file_path, build_profile, extra_env=molt_extra_env
        )
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
                extra_env=molt_extra_env,
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
            record["raw_status"] = "oom"
            record["resolved_status"] = "oom"
            return "oom"

        cp_out = _normalize_output(cp_out, normalize)
        cp_err = _normalize_output(cp_err, normalize)
        if molt_out is not None:
            molt_out = _normalize_output(molt_out, normalize)
        molt_err = _normalize_output(molt_err, normalize)

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
                    stderr_ok = _stderr_matches(cp_err, molt_err, stderr_mode)
                    cp_cmp = _canonicalize_stdout(cp_out, stdout_mode)
                    molt_cmp = _canonicalize_stdout(molt_out, stdout_mode)
                    if cp_cmp == molt_cmp and cp_ret == molt_ret and stderr_ok:
                        print(f"[PASS] {file_path}")
                        return _finalize_status("pass")
                    print(f"[FAIL] {file_path} mismatch")
                    print(f"  CPython stdout: {cp_out!r}")
                    print(f"  Molt    stdout: {molt_out!r}")
                    print(f"  CPython return: {cp_ret} stderr: {cp_err!r}")
                    print(f"  Molt    return: {molt_ret} stderr: {molt_err!r}")
                    return _finalize_status("fail")

            def is_compile_error(err: str) -> bool:
                return any(
                    tag in err
                    for tag in ("SyntaxError", "IndentationError", "TabError")
                )

            if cp_ret != 0 and is_compile_error(cp_err) and is_compile_error(molt_err):
                print(f"[PASS] {file_path}")
                return _finalize_status("pass")

            print(f"[FAIL] Molt failed to build {file_path}")
            print(molt_err)
            return _finalize_status("fail")

        stderr_ok = _stderr_matches(cp_err, molt_err, stderr_mode)
        cp_cmp = _canonicalize_stdout(cp_out, stdout_mode)
        molt_cmp = _canonicalize_stdout(molt_out, stdout_mode)

        if cp_cmp == molt_cmp and cp_ret == molt_ret and stderr_ok:
            print(f"[PASS] {file_path}")
            return _finalize_status("pass")
        else:
            print(f"[FAIL] {file_path} mismatch")
            print(f"  CPython stdout: {cp_out!r}")
            print(f"  Molt    stdout: {molt_out!r}")
            print(f"  CPython return: {cp_ret} stderr: {cp_err!r}")
            print(f"  Molt    return: {molt_ret} stderr: {molt_err!r}")
            return _finalize_status("fail")

    try:
        return _impl()
    finally:
        _record_diff_result(record)


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
    guard_config = _diff_memory_guard_config()
    _prepare_memory_guard_run(guard_config)
    jobs = _constrain_jobs_for_memory_guard(jobs, config=guard_config)
    suite_guard_context = harness_memory_guard.HarnessExecutionContext.from_env(
        "MOLT_DIFF",
        os.environ,
        repo_root=Path(_repo_root()),
        artifact_root=_diff_root(),
        limits=_diff_memory_guard_limits(os.environ),
    )
    suite_sentinel = suite_guard_context.start_repo_sentinel(
        label="molt_diff_suite",
        drain_on_exit=True,
        drain_grace_sec=0.35,
        drain_until_clean_sec=0.5,
        drain_max_runtime_sec=5.0,
        on_scan=_record_memory_guard_sentinel_sample,
        on_violation=_record_memory_guard_sentinel_violation,
    )
    sentinel_env_key = harness_memory_guard.repo_sentinel_active_env_key("MOLT_DIFF")
    sentinel_env_previous = os.environ.get(sentinel_env_key)
    if suite_sentinel is not None:
        os.environ[sentinel_env_key] = "1"
        atexit.register(lambda: suite_sentinel.__exit__(None, None, None))
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
            shared_cache = str(_diff_cache_root())
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
                    if fail_fast and status == "fail":
                        break
                    if _memory_guard_trip_message() is not None:
                        break
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
                        if _memory_guard_trip_message() is not None:
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
    if suite_sentinel is not None:
        suite_sentinel.__exit__(None, None, None)
    if sentinel_env_previous is None:
        os.environ.pop(sentinel_env_key, None)
    else:
        os.environ[sentinel_env_key] = sentinel_env_previous
    guard_trip_message = _memory_guard_trip_message()
    status_by_path = {path: status for path, status in results}
    if jobs > 1 and retry_oom and guard_trip_message is None:
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
    if guard_trip_message is not None:
        failed_files.append("<memory_guard>")
        failed += 1
        oom += 1
    total = passed + failed
    try:
        limit = int(os.environ.get("MOLT_DIFF_RSS_TOP", "5"))
    except ValueError:
        limit = 5
    rss_top_run = [
        {
            "file": entry.get("file"),
            "status": _rss_display_status(entry, status_by_path),
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
            "status": _rss_display_status(entry, status_by_path),
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
            "mem_per_job_gb": _memory_guard_scheduler_per_job_gb(guard_config),
            "order": os.environ.get("MOLT_DIFF_ORDER", "auto"),
            "cargo_target_dir": os.environ.get("CARGO_TARGET_DIR", ""),
            "build_profile": build_profile,
            "stdlib_profile": _diff_stdlib_profile(os.environ)[0] or "",
            "warm_cache": warm_cache,
            "retry_oom": retry_oom,
            "batch_compile_server": _diff_batch_compile_server_enabled(),
            "batch_compile_server_strict": _diff_batch_compile_server_strict(),
            "memory_guard": {
                "enabled": True,
                **_config_payload(guard_config),
                "sample_interval": _diff_memory_guard_sample_interval_sec(),
                "write_samples": _diff_memory_guard_write_samples(),
                "stream_mode": _diff_memory_guard_stream_mode(),
                "event_max_bytes": _memory_guard_jsonl_max_bytes(
                    _diff_memory_guard_events_jsonl()
                ),
                "sample_max_bytes": _memory_guard_jsonl_max_bytes(
                    _diff_memory_guard_global_samples_jsonl()
                ),
                "tripped": guard_trip_message is not None,
                "trip_message": guard_trip_message,
                "trip_file": str(_diff_memory_guard_trip_file()),
            },
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
    _print_rss_top(
        run_id,
        limit if _diff_measure_rss() else 0,
        status_by_path=status_by_path,
    )
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
        "--stdlib-profile",
        choices=["micro", "full"],
        default=None,
        help=(
            "Stdlib profile forwarded to `molt build` for the Molt side "
            "(default: MOLT_DIFF_STDLIB_PROFILE or build default)."
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
    if args.stdlib_profile is not None:
        os.environ["MOLT_DIFF_STDLIB_PROFILE"] = args.stdlib_profile
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
