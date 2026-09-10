from __future__ import annotations

import datetime as dt
import functools
import json
import os
import shutil
from pathlib import Path
from typing import Any

from molt.cargo_workspace import workspace_manifest_facts
from molt.file_hashing import _sha256_file_with_size
from molt.cli.atomic_io import _atomic_write_text
from molt.cli.command_runtime import _run_completed_command
from molt.cli.output import fail as _fail
from molt.cli.runtime_paths import _cargo_target_root_cached, _molt_session_id


_LOCK_CHECK_CACHE_VERSION = 3


def _check_lockfiles(
    project_root: Path,
    json_output: bool,
    warnings: list[str],
    deterministic: bool,
    deterministic_warn: bool,
    command: str,
) -> int | None:
    pyproject = project_root / "pyproject.toml"
    if not pyproject.exists():
        return None
    lock_path = project_root / "uv.lock"
    cargo_lock = project_root / "Cargo.lock"
    missing = []
    if not lock_path.exists():
        missing.append("uv.lock")
    if not cargo_lock.exists():
        missing.append("Cargo.lock")
    if missing and deterministic:
        missing_text = ", ".join(missing)
        message = (
            f"Missing lockfiles ({missing_text}); run `uv lock` and ensure Cargo.lock."
        )
        if deterministic_warn:
            warnings.append(message)
        else:
            return _fail(message, json_output, command=command)
    if missing:
        warnings.append(f"Missing lockfiles: {', '.join(missing)}")
        return None
    if deterministic:
        skip_uv_lock = os.environ.get("UV_NO_SYNC") == "1"
        if skip_uv_lock:
            warnings.append("Skipping uv.lock check because UV_NO_SYNC=1.")
        else:
            uv_error = _verify_uv_lock(project_root)
            if uv_error is not None:
                if deterministic_warn:
                    warnings.append(uv_error)
                else:
                    return _fail(uv_error, json_output, command=command)
        skip_cargo_lock = os.environ.get("MOLT_SKIP_CARGO_LOCK") == "1"
        if skip_cargo_lock:
            warnings.append("Skipping Cargo.lock check because MOLT_SKIP_CARGO_LOCK=1.")
        else:
            cargo_error = _verify_cargo_lock(project_root)
            if cargo_error is not None:
                if deterministic_warn:
                    warnings.append(cargo_error)
                else:
                    return _fail(cargo_error, json_output, command=command)
    return None


@functools.lru_cache(maxsize=256)
def _lock_check_cache_path_cached(
    project_root_str: str,
    name: str,
    cargo_target_override: str | None,
    cwd_str: str,
    session_id: str | None = None,
) -> Path:
    # The lock-check cache can grow, especially for Cargo metadata inputs.
    # Keep it colocated with Cargo build outputs when CARGO_TARGET_DIR is set.
    target_dir = _cargo_target_root_cached(
        project_root_str,
        cargo_target_override,
        cwd_str,
        session_id,
    )
    return target_dir / "lock_checks" / f"{name}.json"


def _lock_check_cache_path(project_root: Path, name: str) -> Path:
    return _lock_check_cache_path_cached(
        os.fspath(project_root),
        name,
        os.environ.get("CARGO_TARGET_DIR"),
        os.fspath(Path.cwd()),
        _molt_session_id(),
    )


def _lock_check_inputs(
    project_root: Path, paths: list[Path]
) -> dict[str, dict[str, str | int]] | None:
    project_root_resolved = project_root.resolve()
    payload: dict[str, dict[str, str | int]] = {}
    for path in paths:
        try:
            digest, size = _sha256_file_with_size(path)
            resolved = path.resolve()
        except OSError:
            return None
        try:
            key = str(resolved.relative_to(project_root_resolved))
        except ValueError:
            key = str(resolved)
        payload[key] = {"sha256": digest, "size": size}
    return {name: payload[name] for name in sorted(payload)}


def _load_lock_check_cache(path: Path) -> dict[str, Any] | None:
    try:
        data = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(data, dict):
        return None
    return data


def _is_lock_check_cache_valid(
    project_root: Path, name: str, inputs: dict[str, dict[str, str | int]] | None
) -> bool:
    if not inputs:
        return False
    payload = _load_lock_check_cache(_lock_check_cache_path(project_root, name))
    if payload is None:
        return False
    if payload.get("version") != _LOCK_CHECK_CACHE_VERSION:
        return False
    if payload.get("ok") is not True:
        return False
    return payload.get("inputs") == inputs


def _write_lock_check_cache(
    project_root: Path, name: str, inputs: dict[str, dict[str, str | int]] | None
) -> None:
    if not inputs:
        return
    path = _lock_check_cache_path(project_root, name)
    payload = {
        "version": _LOCK_CHECK_CACHE_VERSION,
        "ok": True,
        "checked_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "inputs": inputs,
    }
    _atomic_write_text(path, json.dumps(payload, sort_keys=True) + "\n")


def _verify_uv_lock(project_root: Path) -> str | None:
    if shutil.which("uv") is None:
        return "Deterministic builds require uv; install uv to validate uv.lock."
    inputs = _lock_check_inputs(
        project_root,
        [project_root / "pyproject.toml", project_root / "uv.lock"],
    )
    if _is_lock_check_cache_valid(project_root, "uv", inputs):
        return None
    try:
        result = _run_completed_command(
            ["uv", "lock", "--check"],
            cwd=project_root,
            capture_output=True,
            env=None,
            memory_guard_prefix="MOLT_BUILD",
        )
    except OSError as exc:
        return f"Failed to run `uv lock --check`: {exc}"
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip() or "uv lock check failed"
        return f"uv.lock is out of date or invalid: {detail}"
    _write_lock_check_cache(project_root, "uv", inputs)
    return None


def _verify_cargo_lock(project_root: Path) -> str | None:
    if shutil.which("cargo") is None:
        return "Deterministic builds require cargo; install Rust toolchain to validate Cargo.lock."
    try:
        cargo_inputs = [
            *workspace_manifest_facts(project_root).input_manifests,
            project_root / "Cargo.lock",
        ]
    except ValueError as exc:
        return f"Cannot validate Cargo.lock workspace inputs: {exc}"
    inputs = _lock_check_inputs(project_root, cargo_inputs)
    if _is_lock_check_cache_valid(project_root, "cargo", inputs):
        return None
    try:
        result = _run_completed_command(
            ["cargo", "metadata", "--locked", "--format-version", "1"],
            cwd=project_root,
            capture_output=True,
            env=None,
            memory_guard_prefix="MOLT_BUILD",
        )
    except OSError as exc:
        return f"Failed to run `cargo metadata --locked`: {exc}"
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip() or "cargo metadata failed"
        return f"Cargo.lock is out of date or invalid: {detail}"
    _write_lock_check_cache(project_root, "cargo", inputs)
    return None
