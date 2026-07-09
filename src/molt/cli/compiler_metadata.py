from __future__ import annotations

import functools
import hashlib
import json
import os
import shutil
import subprocess
from pathlib import Path
import tomllib
from typing import Any

from molt.cli.command_runtime import _CLI_MEMORY_GUARD_PREFIX, _run_completed_command
from molt.cli.default_paths import _default_molt_cache
from molt.cli.json_cache import _read_cached_json_object, _write_cached_json_object


_CLI_PACKAGE_ROOT = Path(__file__).resolve().parent
_MOLT_PACKAGE_ROOT = _CLI_PACKAGE_ROOT.parent
_SRC_ROOT = _MOLT_PACKAGE_ROOT.parent
_COMPILER_ROOT = _SRC_ROOT.parent
_RUSTC_VERSION_CACHE_SCHEMA_VERSION = 1
_GIT_CLEAN_SOURCE_STATE_SCHEMA_VERSION = 1
_GIT_CLEAN_PATHSPEC_SOURCE_STATE_SCHEMA_VERSION = 1
_GIT_CLEAN_SOURCE_STATUS_TIMEOUT_SEC = 5.0


def _compiler_root() -> Path:
    return _COMPILER_ROOT


def _git_rev(root: Path) -> str | None:
    try:
        result = _run_completed_command(
            ["git", "-C", str(root), "rev-parse", "HEAD"],
            capture_output=True,
            env=None,
            cwd=root,
            memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    value = result.stdout.strip()
    return value or None


def _git_clean_head(root: Path) -> str | None:
    try:
        result = _run_completed_command(
            [
                "git",
                "-C",
                str(root),
                "status",
                "--porcelain=v2",
                "--branch",
                "--untracked-files=all",
            ],
            capture_output=True,
            env=None,
            cwd=root,
            memory_guard_prefix=None,
            timeout=_GIT_CLEAN_SOURCE_STATUS_TIMEOUT_SEC,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if result.returncode != 0:
        return None
    head: str | None = None
    for line in result.stdout.splitlines():
        if line.startswith("# branch.oid "):
            candidate = line.removeprefix("# branch.oid ").strip()
            if candidate and candidate != "(initial)":
                head = candidate
            continue
        if line.startswith("# "):
            continue
        if line.strip():
            return None
    return head


@functools.lru_cache(maxsize=16)
def _compiler_clean_source_state_cached(root_str: str) -> dict[str, str | int] | None:
    root = Path(root_str)
    git_rev = _git_clean_head(root)
    if git_rev is None:
        return None
    return {
        "schema_version": _GIT_CLEAN_SOURCE_STATE_SCHEMA_VERSION,
        "kind": "git-clean-head",
        "head": git_rev,
    }


def _compiler_clean_source_state(root: Path) -> dict[str, str | int] | None:
    try:
        resolved = root.resolve()
    except OSError:
        resolved = root
    return _compiler_clean_source_state_cached(os.fspath(resolved))


def _clean_pathspecs_for_root(
    root: Path,
    path_keys: tuple[str, ...],
) -> tuple[str, ...] | None:
    try:
        root_resolved = root.resolve()
    except OSError:
        root_resolved = root
    pathspecs: list[str] = []
    for path_key in sorted(set(path_keys)):
        path = Path(path_key)
        if not path.is_absolute():
            path = root_resolved / path
        try:
            rel = path.resolve(strict=False).relative_to(root_resolved)
        except (OSError, ValueError):
            return None
        rel_text = rel.as_posix()
        pathspecs.append(rel_text or ".")
    return tuple(pathspecs)


def _git_clean_pathspec_state(
    root: Path,
    pathspecs: tuple[str, ...],
) -> dict[str, str | int] | None:
    if not pathspecs:
        return None
    try:
        status = _run_completed_command(
            [
                "git",
                "-C",
                str(root),
                "status",
                "--porcelain=v2",
                "--untracked-files=all",
                "--",
                *pathspecs,
            ],
            capture_output=True,
            env=None,
            cwd=root,
            memory_guard_prefix=None,
            timeout=_GIT_CLEAN_SOURCE_STATUS_TIMEOUT_SEC,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if status.returncode != 0 or status.stdout.strip():
        return None
    try:
        listing = _run_completed_command(
            ["git", "-C", str(root), "ls-files", "-s", "-z", "--", *pathspecs],
            capture_output=True,
            env=None,
            cwd=root,
            memory_guard_prefix=None,
            timeout=_GIT_CLEAN_SOURCE_STATUS_TIMEOUT_SEC,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if listing.returncode != 0:
        return None
    tracked = listing.stdout
    if not tracked:
        return None
    pathspec_payload = "\0".join(pathspecs).encode("utf-8")
    tracked_payload = tracked.encode("utf-8", errors="surrogateescape")
    return {
        "schema_version": _GIT_CLEAN_PATHSPEC_SOURCE_STATE_SCHEMA_VERSION,
        "kind": "git-clean-pathspec",
        "pathspec_count": len(pathspecs),
        "pathspec_digest": hashlib.sha256(pathspec_payload).hexdigest(),
        "tracked_digest": hashlib.sha256(tracked_payload).hexdigest(),
        "tracked_entry_count": tracked.count("\0"),
    }


@functools.lru_cache(maxsize=128)
def _compiler_clean_pathspec_source_state_cached(
    root_str: str,
    pathspecs: tuple[str, ...],
) -> dict[str, str | int] | None:
    return _git_clean_pathspec_state(Path(root_str), pathspecs)


def _compiler_clean_pathspec_source_state(
    root: Path,
    path_keys: tuple[str, ...],
) -> dict[str, str | int] | None:
    try:
        resolved = root.resolve()
    except OSError:
        resolved = root
    pathspecs = _clean_pathspecs_for_root(resolved, path_keys)
    if pathspecs is None:
        return None
    return _compiler_clean_pathspec_source_state_cached(
        os.fspath(resolved),
        pathspecs,
    )


def _compiler_metadata() -> tuple[str | None, str | None]:
    compiler_root = _compiler_root()
    try:
        data = tomllib.loads((compiler_root / "pyproject.toml").read_text())
    except (OSError, tomllib.TOMLDecodeError):
        data = {}
    project = data.get("project")
    version = project.get("version") if isinstance(project, dict) else None
    git_rev = _git_rev(compiler_root)
    return version if isinstance(version, str) else None, git_rev


def _file_identity(path: Path, *, include_digest: bool = False) -> dict[str, Any]:
    try:
        resolved = path.resolve(strict=False)
    except OSError:
        resolved = path
    identity: dict[str, Any] = {"path": os.fspath(resolved)}
    try:
        stat = resolved.stat()
    except OSError:
        identity["exists"] = False
        return identity
    identity.update(
        {
            "exists": True,
            "size": stat.st_size,
            "mtime_ns": stat.st_mtime_ns,
            "ctime_ns": stat.st_ctime_ns,
        }
    )
    if include_digest:
        try:
            identity["sha256"] = hashlib.sha256(resolved.read_bytes()).hexdigest()
        except OSError:
            identity["sha256"] = None
    return identity


def _rust_toolchain_channel(toolchain_file: Path) -> str | None:
    try:
        data = tomllib.loads(toolchain_file.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError):
        return None
    toolchain = data.get("toolchain")
    if not isinstance(toolchain, dict):
        return None
    channel = toolchain.get("channel")
    return channel if isinstance(channel, str) and channel else None


def _rustup_home() -> Path | None:
    raw = os.environ.get("RUSTUP_HOME")
    if raw:
        return Path(raw).expanduser()
    try:
        return Path.home() / ".rustup"
    except RuntimeError:
        return None


def _rustup_toolchain_rustc_identities(channel: str | None) -> list[dict[str, Any]]:
    if not channel:
        return []
    home = _rustup_home()
    if home is None:
        return []
    toolchains = home / "toolchains"
    if not toolchains.is_dir():
        return []
    identities: list[dict[str, Any]] = []
    for toolchain_dir in sorted(toolchains.glob(f"{channel}-*")):
        for exe_name in ("rustc.exe", "rustc"):
            rustc = toolchain_dir / "bin" / exe_name
            if rustc.exists():
                identities.append(_file_identity(rustc))
    return identities


def _rustc_version_cache_identity() -> dict[str, Any]:
    compiler_root = _compiler_root()
    toolchain_file = compiler_root / "rust-toolchain.toml"
    channel = _rust_toolchain_channel(toolchain_file)
    rustc_path = shutil.which("rustc")
    identity: dict[str, Any] = {
        "schema_version": _RUSTC_VERSION_CACHE_SCHEMA_VERSION,
        "command": "rustc -Vv",
        "compiler_root": os.fspath(compiler_root),
        "rustc_path": rustc_path or "rustc",
        "rustup_toolchain_env": os.environ.get("RUSTUP_TOOLCHAIN"),
        "rustup_home_env": os.environ.get("RUSTUP_HOME"),
        "cargo_home_env": os.environ.get("CARGO_HOME"),
        "toolchain_file": _file_identity(toolchain_file, include_digest=True),
        "toolchain_channel": channel,
        "rustup_toolchain_rustc": _rustup_toolchain_rustc_identities(channel),
    }
    if rustc_path:
        identity["rustc_executable"] = _file_identity(Path(rustc_path))
    return identity


def _rustc_version_cache_digest(identity: dict[str, Any]) -> str:
    encoded = json.dumps(
        identity,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _rustc_version_cache_path(identity_digest: str) -> Path:
    return (
        _default_molt_cache()
        / "toolchain_identity"
        / f"rustc_version.{identity_digest}.json"
    )


def _read_cached_rustc_version(identity_digest: str) -> str | None:
    payload = _read_cached_json_object(_rustc_version_cache_path(identity_digest))
    if payload is None:
        return None
    if payload.get("schema_version") != _RUSTC_VERSION_CACHE_SCHEMA_VERSION:
        return None
    if payload.get("identity_digest") != identity_digest:
        return None
    version = payload.get("rustc_version")
    return version if isinstance(version, str) and version else None


def _write_cached_rustc_version(identity_digest: str, rustc_version: str) -> None:
    try:
        _write_cached_json_object(
            _rustc_version_cache_path(identity_digest),
            {
                "schema_version": _RUSTC_VERSION_CACHE_SCHEMA_VERSION,
                "identity_digest": identity_digest,
                "rustc_version": rustc_version,
            },
        )
    except OSError:
        return


@functools.lru_cache(maxsize=1)
def _rustc_version() -> str | None:
    identity = _rustc_version_cache_identity()
    identity_digest = _rustc_version_cache_digest(identity)
    cached = _read_cached_rustc_version(identity_digest)
    if cached is not None:
        return cached
    try:
        result = _run_completed_command(
            ["rustc", "-Vv"],
            capture_output=True,
            env=None,
            cwd=None,
            memory_guard_prefix="MOLT_BUILD",
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    version = result.stdout.strip()
    if version:
        _write_cached_rustc_version(identity_digest, version)
    return version or None
