from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
import json
from pathlib import Path
from typing import Protocol


class ProcessSampleLike(Protocol):
    pid: int
    ppid: int
    rss_kb: int
    command: str
    pgid: int | None
    elapsed_sec: int | None


REPRO_ENV_KEYS = {
    "CARGO_TARGET_DIR",
    "CI",
    "CODEX_SESSION_ID",
    "CODEX_WORKSPACE",
    "MOLT_CACHE",
    "MOLT_DIFF_CARGO_TARGET_DIR",
    "MOLT_DIFF_ROOT",
    "MOLT_EXT_ROOT",
    "MOLT_GUARD_PROFILE",
    "MOLT_GUARD_PROFILE_LOG",
    "MOLT_MEMORY_GUARD_TERMINATION_WAIT_SEC",
    "MOLT_PREFER_EXTERNAL_ARTIFACTS",
    "MOLT_SESSION_ID",
    "PYTEST_CURRENT_TEST",
    "PYTEST_XDIST_WORKER",
    "PYTHONHASHSEED",
    "PYTHONPATH",
    "RUSTFLAGS",
    "RUSTC_WRAPPER",
    "TMPDIR",
    "UV_CACHE_DIR",
    "VIRTUAL_ENV",
}
REPRO_ENV_PREFIXES = ("CODEX_", "GITHUB_", "MOLT_", "PYTEST_")
SECRET_ENV_TOKENS = (
    "AUTH",
    "COOKIE",
    "CREDENTIAL",
    "KEY",
    "PASS",
    "SECRET",
    "TOKEN",
)
PYTEST_CURRENT_TEST_FILE_MAX_BYTES = 16 * 1024
PYTEST_CURRENT_TEST_WORKER_MAX_FILES = 128
PYTEST_COMMAND_NAMES = frozenset({"pytest", "py.test", "pytest.exe", "py.test.exe"})


def _safe_repro_env_key(key: str) -> bool:
    upper = key.upper()
    if any(token in upper for token in SECRET_ENV_TOKENS):
        return False
    return key in REPRO_ENV_KEYS or any(
        key.startswith(prefix) for prefix in REPRO_ENV_PREFIXES
    )


def _safe_repro_env_value(value: object) -> str:
    text = str(value)
    return text if len(text) <= 512 else f"{text[:512]}...<truncated>"


def _safe_repro_env(environ: Mapping[str, str]) -> dict[str, str]:
    payload: dict[str, str] = {}
    for key in sorted(environ):
        if not _safe_repro_env_key(key):
            continue
        payload[key] = _safe_repro_env_value(environ.get(key, ""))
    return payload


def _safe_repro_env_delta(
    environ: Mapping[str, str],
    *,
    baseline: Mapping[str, str],
) -> dict[str, object]:
    added: dict[str, str] = {}
    changed: dict[str, dict[str, str]] = {}
    removed: list[str] = []
    for key in sorted(set(baseline) | set(environ)):
        if not _safe_repro_env_key(key):
            continue
        in_base = key in baseline
        in_env = key in environ
        if in_env and not in_base:
            added[key] = _safe_repro_env_value(environ[key])
        elif in_base and not in_env:
            removed.append(key)
        elif in_base and in_env and baseline[key] != environ[key]:
            changed[key] = {
                "from": _safe_repro_env_value(baseline[key]),
                "to": _safe_repro_env_value(environ[key]),
            }
    return {
        "baseline": "guard_parent_environment",
        "added": added,
        "changed": changed,
        "removed": removed,
    }


def _process_sample_payload(sample: ProcessSampleLike) -> dict[str, object]:
    return {
        "pid": sample.pid,
        "ppid": sample.ppid,
        "pgid": sample.pgid,
        "rss_kb": sample.rss_kb,
        "elapsed_sec": sample.elapsed_sec,
        "command": sample.command,
    }


def _bounded_process_sample_payload(
    sample: ProcessSampleLike,
    *,
    max_command_chars: int = 512,
) -> dict[str, object]:
    payload = _process_sample_payload(sample)
    command = str(payload["command"])
    if len(command) > max_command_chars:
        payload["command"] = f"{command[:max_command_chars]}...<truncated>"
    return payload


def _host_control_plane_payload(
    samples: Mapping[int, ProcessSampleLike],
    *,
    sample_pgid: Callable[[ProcessSampleLike], int],
    is_host_control_plane_process: Callable[[ProcessSampleLike], bool],
    protected_process_group_ids: Callable[
        [Mapping[int, ProcessSampleLike]],
        set[int],
    ],
    max_samples: int = 32,
) -> dict[str, object] | None:
    host_pgids = {
        sample_pgid(sample)
        for sample in samples.values()
        if is_host_control_plane_process(sample)
    }
    protected_pgids = protected_process_group_ids(samples)
    if not host_pgids and not protected_pgids:
        return None
    host_samples = [
        sample
        for sample in sorted(samples.values(), key=lambda item: item.pid)
        if sample_pgid(sample) in host_pgids
    ]
    payload: dict[str, object] = {
        "protected_pgids": sorted(protected_pgids),
        "host_pgids": sorted(host_pgids),
        "samples": [
            _bounded_process_sample_payload(sample)
            for sample in host_samples[:max_samples]
        ],
    }
    if len(host_samples) > max_samples:
        payload["truncated_samples"] = len(host_samples) - max_samples
    return payload


def _process_lineage_payload(
    samples: Mapping[int, ProcessSampleLike],
    *,
    pid: int,
    max_depth: int = 8,
) -> list[dict[str, object]]:
    lineage: list[dict[str, object]] = []
    seen: set[int] = set()
    current = pid
    for _ in range(max_depth):
        if current <= 0 or current in seen:
            break
        seen.add(current)
        sample = samples.get(current)
        if sample is None:
            lineage.append({"pid": current, "sample_missing": True})
            break
        lineage.append(_process_sample_payload(sample))
        if sample.ppid <= 0 or sample.ppid == current:
            break
        current = sample.ppid
    return lineage


def _path_is_under(path: Path, root: Path) -> bool:
    try:
        path.resolve(strict=False).relative_to(root.resolve(strict=False))
    except ValueError:
        return False
    return True


def _pytest_custody_artifact_path(
    kind: str,
    suffix: str,
    *,
    summary_dir: Path,
    pid: int,
) -> Path:
    safe_kind = "".join(ch if ch.isalnum() else "-" for ch in kind.lower()).strip("-")
    safe_suffix = "".join(ch if ch.isalnum() else "-" for ch in suffix.lower()).strip(
        "-"
    )
    return summary_dir / f"{safe_kind or 'pytest'}-{pid}_{safe_suffix}.json"


def _canonical_pytest_current_test_file_path(
    raw_path: str | None,
    *,
    root: Path,
    summary_dir: Path,
    fallback_pid: int,
) -> Path:
    path = Path(raw_path).expanduser() if raw_path else None
    if path is None:
        return _pytest_custody_artifact_path(
            "test-custody",
            "current-test",
            summary_dir=summary_dir,
            pid=fallback_pid,
        )
    if not path.is_absolute():
        path = root / path
    path = path.resolve(strict=False)
    if not _path_is_under(path, summary_dir):
        return _pytest_custody_artifact_path(
            "test-custody",
            "current-test",
            summary_dir=summary_dir,
            pid=fallback_pid,
        )
    return path


def _looks_like_repo_test_path(raw: str, cwd: str | Path | None, *, root: Path) -> bool:
    if not raw or raw == "-" or raw.startswith("-") or not raw.endswith(".py"):
        return False
    path = Path(raw).expanduser()
    if not path.is_absolute():
        cwd_root = Path.cwd() if cwd is None else Path(cwd).expanduser()
        path = cwd_root / path
    try:
        path.resolve(strict=False).relative_to((root / "tests").resolve(strict=False))
    except ValueError:
        return False
    return True


def _command_requests_test_custody(
    command: Sequence[str],
    *,
    cwd: str | Path | None = None,
    root: Path,
) -> bool:
    args = tuple(str(arg) for arg in command)
    for idx, arg in enumerate(args):
        if Path(arg).name in PYTEST_COMMAND_NAMES:
            return True
        if _looks_like_repo_test_path(arg, cwd, root=root):
            return True
        if arg == "-m" and idx + 1 < len(args):
            module = args[idx + 1]
            if module == "pytest" or module == "tests" or module.startswith("tests."):
                return True
    return False


def test_custody_launch_env(
    command: Sequence[str],
    *,
    environ: Mapping[str, str],
    cwd: str | Path | None,
    root: Path,
    summary_dir: Path,
    fallback_pid: int,
    current_test_file_env: str,
) -> dict[str, str]:
    env = dict(environ)
    if not _command_requests_test_custody(command, cwd=cwd, root=root):
        return env
    summary_dir.mkdir(parents=True, exist_ok=True)
    env[current_test_file_env] = str(
        _canonical_pytest_current_test_file_path(
            env.get(current_test_file_env),
            root=root,
            summary_dir=summary_dir,
            fallback_pid=fallback_pid,
        )
    )
    return env


def _read_pytest_current_test_json(path: Path) -> dict[str, object]:
    payload: dict[str, object] = {"path": str(path)}
    try:
        data = path.read_bytes()
    except FileNotFoundError:
        payload["missing"] = True
        return payload
    except OSError as exc:
        payload["read_error"] = str(exc)
        return payload
    if len(data) > PYTEST_CURRENT_TEST_FILE_MAX_BYTES:
        payload["truncated"] = True
        data = data[:PYTEST_CURRENT_TEST_FILE_MAX_BYTES]
    try:
        text = data.decode("utf-8", errors="replace")
    except Exception as exc:
        payload["decode_error"] = str(exc)
        return payload
    try:
        decoded = json.loads(text)
    except json.JSONDecodeError:
        payload["raw"] = text[:PYTEST_CURRENT_TEST_FILE_MAX_BYTES]
    else:
        payload["payload"] = decoded
    return payload


def _lineage_pid_set(
    samples: Mapping[int, ProcessSampleLike],
    *,
    pid: int,
    max_depth: int = 16,
) -> set[int]:
    lineage: set[int] = set()
    seen: set[int] = set()
    current = pid
    for _ in range(max_depth):
        if current <= 0 or current in seen:
            break
        seen.add(current)
        lineage.add(current)
        sample = samples.get(current)
        if sample is None or sample.ppid <= 0 or sample.ppid == current:
            break
        current = sample.ppid
    return lineage


def _pytest_worker_record_payloads(
    aggregate_path: Path,
    *,
    samples: Mapping[int, ProcessSampleLike],
    incident_pid: int | None,
) -> list[dict[str, object]]:
    worker_dir = aggregate_path.with_name(f"{aggregate_path.name}.d")
    try:
        paths = sorted(
            (path for path in worker_dir.glob("*.json") if path.is_file()),
            key=lambda path: path.stat().st_mtime,
            reverse=True,
        )
    except OSError:
        return []
    incident_lineage = (
        _lineage_pid_set(samples, pid=incident_pid)
        if incident_pid is not None
        else set()
    )
    records: list[dict[str, object]] = []
    for path in paths[:PYTEST_CURRENT_TEST_WORKER_MAX_FILES]:
        record = _read_pytest_current_test_json(path)
        decoded = record.get("payload")
        if isinstance(decoded, dict) and incident_lineage:
            try:
                record_pid = int(decoded.get("pid", 0) or 0)
            except (TypeError, ValueError):
                record_pid = 0
            if record_pid in incident_lineage:
                record["incident_match"] = "pid_lineage"
        records.append(record)
    if len(paths) > PYTEST_CURRENT_TEST_WORKER_MAX_FILES:
        records.append(
            {
                "truncated_worker_records": len(paths)
                - PYTEST_CURRENT_TEST_WORKER_MAX_FILES
            }
        )
    return records


def _pytest_current_test_file_payload(
    environ: Mapping[str, str],
    *,
    samples: Mapping[int, ProcessSampleLike],
    incident_pid: int | None = None,
    root: Path,
    summary_dir: Path,
    current_test_file_env: str,
) -> dict[str, object] | None:
    raw_path = environ.get(current_test_file_env, "").strip()
    if not raw_path:
        return None
    path = Path(raw_path).expanduser()
    if not path.is_absolute():
        path = root / path
    path = path.resolve(strict=False)
    if not _path_is_under(path, summary_dir):
        return {
            "path": str(path),
            "rejected": "noncanonical",
            "canonical_root": str(summary_dir),
        }
    payload = _read_pytest_current_test_json(path)
    worker_records = _pytest_worker_record_payloads(
        path,
        samples=samples,
        incident_pid=incident_pid,
    )
    if worker_records:
        payload["worker_records"] = worker_records
    return payload


def repro_context_payload(
    *,
    command: Sequence[str],
    cwd: str | Path | None,
    source_environ: Mapping[str, str],
    baseline_environ: Mapping[str, str],
    root: Path,
    summary_dir: Path,
    current_test_file_env: str,
    samples: Mapping[int, ProcessSampleLike],
    pid: int,
    parent_pid: int,
    current_process_group_id: int | None,
    current_session_id: int | None,
    parent_process_group_id: int | None,
    argv: Sequence[str],
    python_executable: str,
    python_version: str,
    platform_name: str,
    platform_detail: str,
    machine: str,
    sample_pgid: Callable[[ProcessSampleLike], int],
    is_host_control_plane_process: Callable[[ProcessSampleLike], bool],
    protected_process_group_ids: Callable[
        [Mapping[int, ProcessSampleLike]],
        set[int],
    ],
    max_process_rss_kb: int | None = None,
    max_total_rss_kb: int | None = None,
    max_global_rss_kb: int | None = None,
    child_rlimit_kb: int | None = None,
    timeout_s: float | None = None,
    poll_interval_s: float | None = None,
    summary_json: str | None = None,
    incident_pid: int | None = None,
) -> dict[str, object]:
    cwd_path = Path.cwd() if cwd is None else Path(cwd).expanduser()
    pytest_payload: dict[str, object] = {
        "current_test": source_environ.get("PYTEST_CURRENT_TEST", ""),
        "xdist_worker": source_environ.get("PYTEST_XDIST_WORKER", ""),
    }
    current_test_file = _pytest_current_test_file_payload(
        source_environ,
        samples=samples,
        incident_pid=incident_pid,
        root=root,
        summary_dir=summary_dir,
        current_test_file_env=current_test_file_env,
    )
    if current_test_file is not None:
        pytest_payload["current_test_file"] = current_test_file
    payload: dict[str, object] = {
        "command": list(command),
        "cwd": str(cwd_path.resolve(strict=False)),
        "env": _safe_repro_env(source_environ),
        "env_delta": _safe_repro_env_delta(
            source_environ,
            baseline=baseline_environ,
        ),
        "guard_process": {
            "pid": pid,
            "ppid": parent_pid,
            "pgid": current_process_group_id,
            "sid": current_session_id,
            "argv": list(argv),
        },
        "host": {
            "python_executable": python_executable,
            "python_version": python_version,
            "platform": platform_name,
            "platform_detail": platform_detail,
            "machine": machine,
        },
        "limits": {
            "max_process_rss_kb": max_process_rss_kb,
            "max_process_rss_gb": (
                None
                if max_process_rss_kb is None
                else max_process_rss_kb / (1024 * 1024)
            ),
            "max_total_rss_kb": max_total_rss_kb,
            "max_total_rss_gb": (
                None if max_total_rss_kb is None else max_total_rss_kb / (1024 * 1024)
            ),
            "max_global_rss_kb": max_global_rss_kb,
            "max_global_rss_gb": (
                None if max_global_rss_kb is None else max_global_rss_kb / (1024 * 1024)
            ),
            "child_rlimit_kb": child_rlimit_kb,
            "child_rlimit_gb": (
                None if child_rlimit_kb is None else child_rlimit_kb / (1024 * 1024)
            ),
            "timeout_s": timeout_s,
            "poll_interval_s": poll_interval_s,
        },
        "parent_lineage": _process_lineage_payload(samples, pid=pid),
        "pytest": pytest_payload,
    }
    host_control_plane = _host_control_plane_payload(
        samples,
        sample_pgid=sample_pgid,
        is_host_control_plane_process=is_host_control_plane_process,
        protected_process_group_ids=protected_process_group_ids,
    )
    if host_control_plane is not None:
        payload["host_control_plane"] = host_control_plane
    if summary_json:
        payload["summary_json"] = str(Path(summary_json).expanduser())
    parent_sample = samples.get(parent_pid)
    if parent_sample is not None:
        payload["parent_process"] = _process_sample_payload(parent_sample)
    else:
        payload["parent_process"] = {
            "pid": parent_pid,
            "pgid": parent_process_group_id,
            "sample_missing": True,
        }
    return payload


def repro_context_line(payload: Mapping[str, object]) -> str:
    return json.dumps(payload, sort_keys=True, separators=(",", ":"))
