from __future__ import annotations

from collections.abc import Callable
import hashlib
import os
from pathlib import Path
import re


_BACKEND_DAEMON_SOCKET_BASENAME = "moltbd.ffffffffffffffff.sock"
_UNIX_SOCKET_PATH_MAX_BYTES = 104
_SIDECAR_LABEL_SAFE_RE = re.compile(r"[^A-Za-z0-9._-]+")


def _unix_socket_path_exceeds_limit(path: Path, *, os_name: str | None = None) -> bool:
    if (os.name if os_name is None else os_name) == "nt":
        return False
    return len(os.fsencode(os.fspath(path))) >= _UNIX_SOCKET_PATH_MAX_BYTES


def _short_backend_daemon_socket_dir(
    default_dir: Path,
    *,
    os_name: str | None = None,
    path_exceeds_limit: Callable[[Path], bool] | None = None,
) -> Path:
    if (os.name if os_name is None else os_name) == "nt":
        return default_dir
    exceeds_limit = path_exceeds_limit or _unix_socket_path_exceeds_limit
    probe = default_dir / _BACKEND_DAEMON_SOCKET_BASENAME
    if not exceeds_limit(probe):
        return default_dir
    for root in (Path("/tmp"), Path("/private/tmp")):
        candidate = root / "molt-backend-daemon"
        if not exceeds_limit(candidate / _BACKEND_DAEMON_SOCKET_BASENAME):
            return candidate
    return default_dir


def _backend_daemon_socket_path_error(socket_path: Path) -> str:
    path_len = len(os.fsencode(os.fspath(socket_path)))
    return (
        "Backend daemon unix socket path is too long "
        f"({path_len} bytes; limit {_UNIX_SOCKET_PATH_MAX_BYTES - 1}). "
        "Use a shorter path or set MOLT_BACKEND_DAEMON_SOCKET_DIR to a short local "
        "directory such as /tmp/molt-backend-daemon."
    )


def _backend_daemon_sidecar_label(raw: str) -> str:
    return _SIDECAR_LABEL_SAFE_RE.sub("-", raw).strip("._-")[:32]


def _backend_daemon_paths(
    *,
    project_root_str: str,
    cargo_profile: str,
    config_digest: str,
    explicit_socket: str,
    socket_dir_override: str | None,
    build_state_root_str: str,
    tempdir_str: str,
    session_id: str | None,
    cwd: Path | None = None,
    path_exceeds_limit: Callable[[Path], bool] | None = None,
) -> tuple[Path, Path, Path]:
    project_root = Path(project_root_str)
    build_state_root = Path(build_state_root_str)
    session_id = (session_id or "").strip()

    if explicit_socket:
        socket_path = Path(explicit_socket).expanduser()
        if not socket_path.is_absolute():
            socket_path = (project_root / socket_path).absolute()
        sidecar_suffix = hashlib.sha256(
            os.fspath(socket_path).encode("utf-8")
        ).hexdigest()[:16]
        sidecar_label = ""
    else:
        default_dir = _short_backend_daemon_socket_dir(
            Path(tempdir_str) / "molt-backend-daemon",
            path_exceeds_limit=path_exceeds_limit,
        )
        if socket_dir_override:
            socket_dir = Path(socket_dir_override).expanduser()
            if not socket_dir.is_absolute():
                socket_dir = ((cwd or Path.cwd()) / socket_dir).absolute()
        else:
            socket_dir = default_dir
        key = (
            f"{project_root.resolve()}|{build_state_root}|"
            f"{cargo_profile}|{config_digest}|{session_id}"
        )
        sidecar_suffix = hashlib.sha256(key.encode("utf-8")).hexdigest()[:16]
        socket_path = socket_dir / f"moltbd.{sidecar_suffix}.sock"
        sidecar_label = _backend_daemon_sidecar_label(session_id)
    daemon_root = build_state_root / "backend_daemon"
    sidecar_stem = f"molt-backend.{cargo_profile}"
    if sidecar_label:
        sidecar_stem = f"{sidecar_stem}.{sidecar_label}"
    sidecar_stem = f"{sidecar_stem}.{sidecar_suffix}"
    return (
        socket_path,
        daemon_root / f"{sidecar_stem}.log",
        daemon_root / f"{sidecar_stem}.identity.json",
    )
