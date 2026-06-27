from __future__ import annotations

import contextlib
from dataclasses import dataclass
import json
import os
from pathlib import Path
import shlex
import subprocess
import time
from collections.abc import Callable, Mapping, Sequence
from typing import Any

from molt.dx import session_artifact_component

IDENTITY_SCHEMA = "molt.backend_daemon.identity.v1"

HealthProbe = Callable[[Path, float | None], tuple[bool, Mapping[str, Any] | None]]
ProcessCommandProbe = Callable[[int], str | None]
PidAliveProbe = Callable[[int], bool]


@dataclass(frozen=True)
class BackendDaemonIdentity:
    pid: int
    socket_path: Path
    project_root: Path
    cargo_profile: str
    config_digest: str | None
    backend_bin: Path
    created_at: float
    command: str | None = None


@dataclass(frozen=True)
class BackendDaemonIdentityRecord:
    identity: BackendDaemonIdentity
    path: Path


def backend_daemon_identity_payload(
    identity: BackendDaemonIdentity,
) -> dict[str, object]:
    return {
        "schema": IDENTITY_SCHEMA,
        "pid": identity.pid,
        "socket_path": os.fspath(identity.socket_path),
        "project_root": os.fspath(identity.project_root),
        "cargo_profile": identity.cargo_profile,
        "config_digest": identity.config_digest,
        "backend_bin": os.fspath(identity.backend_bin),
        "created_at": identity.created_at,
        "command": identity.command,
    }


def _json_path_field(payload: Mapping[str, object], key: str) -> Path | None:
    raw = payload.get(key)
    if not isinstance(raw, str) or not raw:
        return None
    return Path(raw)


def read_backend_daemon_identity(identity_path: Path) -> BackendDaemonIdentity | None:
    try:
        payload = json.loads(identity_path.read_text(encoding="utf-8"))
    except OSError:
        return None
    except json.JSONDecodeError:
        return None
    if not isinstance(payload, dict):
        return None
    if payload.get("schema") != IDENTITY_SCHEMA:
        return None
    raw_pid = payload.get("pid")
    if not isinstance(raw_pid, int) or raw_pid <= 0:
        return None
    socket_path = _json_path_field(payload, "socket_path")
    project_root = _json_path_field(payload, "project_root")
    backend_bin = _json_path_field(payload, "backend_bin")
    raw_cargo_profile = payload.get("cargo_profile")
    raw_config_digest = payload.get("config_digest")
    raw_created_at = payload.get("created_at")
    raw_command = payload.get("command")
    if (
        socket_path is None
        or project_root is None
        or backend_bin is None
        or not isinstance(raw_cargo_profile, str)
        or not raw_cargo_profile
    ):
        return None
    config_digest = raw_config_digest if isinstance(raw_config_digest, str) else None
    created_at = (
        float(raw_created_at) if isinstance(raw_created_at, (int, float)) else 0.0
    )
    command = raw_command if isinstance(raw_command, str) and raw_command else None
    return BackendDaemonIdentity(
        pid=raw_pid,
        socket_path=socket_path,
        project_root=project_root,
        cargo_profile=raw_cargo_profile,
        config_digest=config_digest,
        backend_bin=backend_bin,
        created_at=created_at,
        command=command,
    )


def write_backend_daemon_identity(
    identity_path: Path,
    identity: BackendDaemonIdentity,
) -> None:
    identity_path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = identity_path.with_name(f".{identity_path.name}.{os.getpid()}.tmp")
    try:
        tmp_path.write_text(
            json.dumps(backend_daemon_identity_payload(identity), sort_keys=True)
            + "\n",
            encoding="utf-8",
        )
        tmp_path.replace(identity_path)
    finally:
        with contextlib.suppress(OSError):
            if tmp_path.exists():
                tmp_path.unlink()


def remove_backend_daemon_identity(identity_path: Path) -> None:
    with contextlib.suppress(OSError):
        identity_path.unlink()


def backend_daemon_build_state_root_from_env(
    env: Mapping[str, str],
    *,
    project_root: Path,
) -> Path:
    explicit = env.get("MOLT_BUILD_STATE_DIR")
    if explicit:
        path = Path(explicit).expanduser()
        return path if path.is_absolute() else (project_root / path).resolve()
    target = Path(
        env.get("CARGO_TARGET_DIR", str(project_root / "target"))
    ).expanduser()
    if not target.is_absolute():
        target = (project_root / target).resolve()
    return target / ".molt_state"


def backend_daemon_root_from_env(
    env: Mapping[str, str],
    *,
    project_root: Path,
) -> Path:
    return (
        backend_daemon_build_state_root_from_env(
            env,
            project_root=project_root,
        )
        / "backend_daemon"
    )


def iter_backend_daemon_identity_records(
    daemon_root: Path,
    *,
    session_id: str | None = None,
) -> tuple[BackendDaemonIdentityRecord, ...]:
    if session_id is not None:
        session_label = session_artifact_component(session_id.strip())
    else:
        session_label = ""
    pattern = (
        f"molt-backend.*.{session_label}.*.identity.json"
        if session_label
        else "*.identity.json"
    )
    try:
        identity_paths = sorted(daemon_root.glob(pattern))
    except OSError:
        return ()
    records: list[BackendDaemonIdentityRecord] = []
    for identity_path in identity_paths:
        identity = read_backend_daemon_identity(identity_path)
        if identity is None:
            continue
        records.append(
            BackendDaemonIdentityRecord(identity=identity, path=identity_path)
        )
    return tuple(records)


def current_session_backend_daemon_identity_records(
    env: Mapping[str, str],
    *,
    project_root: Path,
) -> tuple[BackendDaemonIdentityRecord, ...]:
    session_id = env.get("MOLT_SESSION_ID", "").strip()
    if not session_id:
        return ()
    return iter_backend_daemon_identity_records(
        backend_daemon_root_from_env(env, project_root=project_root),
        session_id=session_id,
    )


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


def _process_command(pid: int) -> str | None:
    if pid <= 0 or os.name == "nt":
        return None
    try:
        result = subprocess.run(
            ["ps", "-p", str(pid), "-o", "command="],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=1.0,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0:
        return None
    command = result.stdout.strip()
    return command or None


def _strip_matching_quotes(token: str) -> str:
    if len(token) >= 2 and token[0] == token[-1] and token[0] in {'"', "'"}:
        return token[1:-1]
    return token


def _split_windows_command_fallback(command: str) -> tuple[str, ...]:
    try:
        return tuple(
            _strip_matching_quotes(token) for token in shlex.split(command, posix=False)
        )
    except ValueError:
        return tuple(command.split())


def _split_windows_command(command: str) -> tuple[str, ...]:
    try:
        import ctypes
        from ctypes import wintypes

        argc = ctypes.c_int()
        command_line_to_argv = ctypes.windll.shell32.CommandLineToArgvW
        command_line_to_argv.argtypes = (
            wintypes.LPCWSTR,
            ctypes.POINTER(ctypes.c_int),
        )
        command_line_to_argv.restype = ctypes.POINTER(wintypes.LPWSTR)
        local_free = ctypes.windll.kernel32.LocalFree
        local_free.argtypes = (wintypes.HLOCAL,)
        local_free.restype = wintypes.HLOCAL
        argv = command_line_to_argv(command, ctypes.byref(argc))
        if not argv:
            return _split_windows_command_fallback(command)
        try:
            return tuple(argv[index] for index in range(argc.value))
        finally:
            local_free(argv)
    except (AttributeError, OSError, ValueError):
        return _split_windows_command_fallback(command)


def _split_command(command: str) -> tuple[str, ...]:
    if os.name == "nt":
        return _split_windows_command(command)
    try:
        return tuple(shlex.split(command))
    except ValueError:
        return tuple(command.split())


def _command_executable_matches_backend(
    executable: str,
    backend_bin: Path | None,
) -> bool:
    exe_path = Path(executable)
    expected_name = backend_bin.name if backend_bin is not None else "molt-backend"
    if exe_path.name != expected_name:
        return False
    if backend_bin is None or not exe_path.is_absolute():
        return True
    try:
        return exe_path.resolve(strict=False) == backend_bin.resolve(strict=False)
    except OSError:
        return str(exe_path) == str(backend_bin)


def _command_has_socket(
    tokens: Sequence[str],
    socket_path: Path | None,
) -> bool:
    for index, token in enumerate(tokens[:-1]):
        if token != "--socket":
            continue
        candidate = tokens[index + 1]
        if socket_path is None:
            return bool(candidate)
        if candidate == str(socket_path):
            return True
        try:
            if Path(candidate) == socket_path:
                return True
        except (OSError, ValueError):
            pass
    return False


def backend_daemon_command_matches_identity(
    command: str,
    *,
    backend_bin: Path | None,
    socket_path: Path | None,
) -> bool:
    tokens = _split_command(command)
    if not tokens:
        return False
    if not _command_executable_matches_backend(tokens[0], backend_bin):
        return False
    return "--daemon" in tokens and _command_has_socket(tokens, socket_path)


def _backend_daemon_identity_process_matches(
    identity: BackendDaemonIdentity,
    *,
    process_command: ProcessCommandProbe | None = None,
) -> bool:
    command_probe = process_command or _process_command
    command = command_probe(identity.pid)
    if command is None:
        return False
    return backend_daemon_command_matches_identity(
        command,
        backend_bin=identity.backend_bin,
        socket_path=identity.socket_path,
    )


def _backend_daemon_health_contradicts_identity(
    identity: BackendDaemonIdentity,
    *,
    health_probe: HealthProbe | None,
    timeout: float = 0.25,
) -> bool:
    if health_probe is None:
        return False
    ready, health = health_probe(identity.socket_path, timeout)
    if not ready or health is None:
        return False
    raw_pid = health.get("pid")
    return isinstance(raw_pid, int) and raw_pid != identity.pid


def backend_daemon_identity_is_verified(
    identity: BackendDaemonIdentity,
    *,
    allow_health_probe: bool,
    health_probe: HealthProbe | None = None,
    process_command: ProcessCommandProbe | None = None,
    pid_alive: PidAliveProbe | None = None,
) -> bool:
    alive_probe = pid_alive or _pid_alive
    if identity.pid <= 0 or not alive_probe(identity.pid):
        return False
    if not _backend_daemon_identity_process_matches(
        identity,
        process_command=process_command,
    ):
        return False
    return not (
        allow_health_probe
        and _backend_daemon_health_contradicts_identity(
            identity,
            health_probe=health_probe,
        )
    )


def backend_daemon_identity_matches_context(
    identity: BackendDaemonIdentity,
    *,
    backend_bin: Path,
    socket_path: Path,
    project_root: Path,
    cargo_profile: str,
    config_digest: str | None,
) -> bool:
    return (
        os.fspath(identity.backend_bin) == os.fspath(backend_bin)
        and os.fspath(identity.socket_path) == os.fspath(socket_path)
        and os.fspath(identity.project_root) == os.fspath(project_root)
        and identity.cargo_profile == cargo_profile
        and identity.config_digest == config_digest
    )


def backend_daemon_identity_for_pid(
    pid: int,
    *,
    socket_path: Path,
    project_root: Path,
    cargo_profile: str,
    config_digest: str | None,
    backend_bin: Path,
    process_command: ProcessCommandProbe | None = None,
) -> BackendDaemonIdentity:
    command_probe = process_command or _process_command
    return BackendDaemonIdentity(
        pid=pid,
        socket_path=socket_path,
        project_root=project_root,
        cargo_profile=cargo_profile,
        config_digest=config_digest,
        backend_bin=backend_bin,
        created_at=time.time(),
        command=command_probe(pid),
    )


def backend_daemon_identity_from_health(
    health: Mapping[str, Any] | None,
    *,
    socket_path: Path,
    project_root: Path,
    cargo_profile: str,
    config_digest: str | None,
    backend_bin: Path,
    process_command: ProcessCommandProbe | None = None,
) -> BackendDaemonIdentity | None:
    if health is None:
        return None
    raw_pid = health.get("pid")
    if not isinstance(raw_pid, int) or raw_pid <= 0:
        return None
    if config_digest is not None:
        raw_spawn_digest = health.get("spawn_config_digest")
        if raw_spawn_digest != config_digest:
            return None
    return backend_daemon_identity_for_pid(
        raw_pid,
        socket_path=socket_path,
        project_root=project_root,
        cargo_profile=cargo_profile,
        config_digest=config_digest,
        backend_bin=backend_bin,
        process_command=process_command,
    )


def _load_memory_guard_module() -> Any | None:
    try:
        from tools import memory_guard
    except ModuleNotFoundError:
        return None
    return memory_guard


def _backend_daemon_snapshot_sample(
    identity: BackendDaemonIdentity,
    *,
    memory_guard: Any,
) -> Any | None:
    samples = memory_guard.sample_processes()
    sample = samples.get(identity.pid)
    if sample is None:
        return None
    if memory_guard.is_host_control_plane_process(sample):
        return None
    if not backend_daemon_command_matches_identity(
        sample.command,
        backend_bin=identity.backend_bin,
        socket_path=identity.socket_path,
    ):
        return None
    return sample


def _termination_actions_succeeded(actions: Sequence[Any]) -> bool:
    if not actions:
        return False
    first_result = getattr(actions[0], "result", "")
    if first_result in {"completed_or_missing", "skipped_missing"}:
        return True
    if first_result != "still_live":
        return False
    if len(actions) == 1:
        return False
    final_result = getattr(actions[-1], "result", "")
    return final_result in {
        "sent",
        "missing",
        "skipped_identity_mismatch",
        "skipped_host_control_lineage",
        "skipped_host_control_plane",
        "skipped_protected_group_member",
    }


def terminate_backend_daemon_identity(
    identity: BackendDaemonIdentity,
    *,
    grace: float = 1.0,
    health_probe: HealthProbe | None = None,
    pid_alive: PidAliveProbe | None = None,
) -> bool:
    alive_probe = pid_alive or _pid_alive
    if identity.pid <= 0 or not alive_probe(identity.pid):
        return False
    if _backend_daemon_health_contradicts_identity(
        identity,
        health_probe=health_probe,
    ):
        return False
    memory_guard = _load_memory_guard_module()
    if memory_guard is None:
        return False
    sample = _backend_daemon_snapshot_sample(
        identity,
        memory_guard=memory_guard,
    )
    if sample is None:
        return False
    actions = memory_guard.terminate_verified_pid(
        identity.pid,
        memory_guard.process_identity(sample),
        sampler=memory_guard.sample_processes,
        grace=grace,
    )
    return _termination_actions_succeeded(actions)


def terminate_backend_daemons_for_session(
    env: Mapping[str, str],
    *,
    project_root: Path,
    grace: float = 1.0,
    health_probe: HealthProbe | None = None,
    pid_alive: PidAliveProbe | None = None,
    remove_identity_files: bool = True,
) -> tuple[BackendDaemonIdentityRecord, ...]:
    terminated: list[BackendDaemonIdentityRecord] = []
    for record in current_session_backend_daemon_identity_records(
        env,
        project_root=project_root,
    ):
        if not terminate_backend_daemon_identity(
            record.identity,
            grace=grace,
            health_probe=health_probe,
            pid_alive=pid_alive,
        ):
            continue
        terminated.append(record)
        if remove_identity_files:
            remove_backend_daemon_identity(record.path)
    return tuple(terminated)


def remove_legacy_pid_files(root: Path, pattern: str = "*.pid") -> int:
    removed = 0
    try:
        pid_files = list(root.glob(pattern))
    except OSError:
        return removed
    for pid_file in pid_files:
        with contextlib.suppress(OSError):
            pid_file.unlink()
            removed += 1
    return removed
