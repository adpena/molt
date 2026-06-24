from __future__ import annotations

import functools
import os
from pathlib import Path

_RUNTIME_STDLIB_PROFILE_ALIASES = {
    "micro": "stdlib_micro",
    "full": "stdlib_full",
}


def _normalize_runtime_stdlib_profile(stdlib_profile: str | None) -> str:
    profile = stdlib_profile or "micro"
    if profile not in _RUNTIME_STDLIB_PROFILE_ALIASES:
        raise ValueError("stdlib_profile must be 'micro' or 'full'")
    return profile


def _runtime_staticlib_target_is_windows(target_triple: str | None) -> bool:
    if target_triple:
        return "windows" in target_triple.lower()
    return os.name == "nt"


def _runtime_lib_archive_name(
    stdlib_profile: str | None,
    target_triple: str | None = None,
) -> str:
    profile = _normalize_runtime_stdlib_profile(stdlib_profile)
    alias = _RUNTIME_STDLIB_PROFILE_ALIASES[profile]
    if _runtime_staticlib_target_is_windows(target_triple):
        return f"molt_runtime.{alias}.lib"
    return f"libmolt_runtime.{alias}.a"


def _runtime_cargo_scratch_lib_name(target_triple: str | None = None) -> str:
    if _runtime_staticlib_target_is_windows(target_triple):
        return "molt_runtime.lib"
    return "libmolt_runtime.a"


def _runtime_cargo_scratch_lib_path(
    runtime_lib: Path,
    target_triple: str | None = None,
) -> Path:
    return runtime_lib.with_name(_runtime_cargo_scratch_lib_name(target_triple))


def _runtime_lib_archive_names(target_triple: str | None = None) -> tuple[str, ...]:
    names = [
        _runtime_lib_archive_name("micro", target_triple),
        _runtime_lib_archive_name("full", target_triple),
        _runtime_cargo_scratch_lib_name(target_triple),
    ]
    return tuple(dict.fromkeys(names))


def _molt_session_id() -> str | None:
    return os.environ.get("MOLT_SESSION_ID")


def _session_artifact_component(session_id: str) -> str:
    return "".join(c if c.isalnum() or c in "-_" else "_" for c in session_id)[:32]


@functools.lru_cache(maxsize=64)
def _cargo_profile_dir(cargo_profile: str) -> str:
    return "debug" if cargo_profile == "dev" else cargo_profile


@functools.lru_cache(maxsize=256)
def _cargo_target_root_cached(
    project_root_str: str,
    cargo_target_override: str | None,
    cwd_str: str,
    session_id: str | None = None,
) -> Path:
    project_root = Path(project_root_str)
    if not cargo_target_override:
        if session_id is not None:
            return (
                project_root
                / "target"
                / "sessions"
                / _session_artifact_component(session_id)
            )
        return project_root / "target"
    path = Path(cargo_target_override).expanduser()
    if not path.is_absolute():
        path = (Path(cwd_str) / path).absolute()
    return path


def _cargo_target_root(project_root: Path) -> Path:
    return _cargo_target_root_cached(
        os.fspath(project_root),
        os.environ.get("CARGO_TARGET_DIR"),
        os.fspath(Path.cwd()),
        _molt_session_id(),
    )


@functools.lru_cache(maxsize=256)
def _build_state_root_cached(
    project_root_str: str,
    build_state_override: str | None,
    cargo_target_override: str | None,
    cwd_str: str,
    session_id: str | None = None,
) -> Path:
    project_root = Path(project_root_str)
    if build_state_override:
        path = Path(build_state_override).expanduser()
        if not path.is_absolute():
            path = (project_root / path).absolute()
        return path
    return (
        _cargo_target_root_cached(
            project_root_str,
            cargo_target_override,
            cwd_str,
            session_id,
        )
        / ".molt_state"
    )


def _build_state_root(project_root: Path) -> Path:
    return _build_state_root_cached(
        os.fspath(project_root),
        os.environ.get("MOLT_BUILD_STATE_DIR"),
        os.environ.get("CARGO_TARGET_DIR"),
        os.fspath(Path.cwd()),
        _molt_session_id(),
    )


@functools.lru_cache(maxsize=256)
def _runtime_lib_path_cached(
    project_root_str: str,
    cargo_profile: str,
    target_triple: str | None,
    stdlib_profile: str | None,
    cargo_target_override: str | None,
    cwd_str: str,
) -> Path:
    profile_dir = _cargo_profile_dir(cargo_profile)
    target_root = _cargo_target_root_cached(
        project_root_str,
        cargo_target_override,
        cwd_str,
        _molt_session_id(),
    )
    archive_name = _runtime_lib_archive_name(stdlib_profile, target_triple)
    if target_triple:
        return target_root / target_triple / profile_dir / archive_name
    return target_root / profile_dir / archive_name


def _runtime_lib_path(
    project_root: Path,
    cargo_profile: str,
    target_triple: str | None,
    stdlib_profile: str | None = "micro",
) -> Path:
    return _runtime_lib_path_cached(
        os.fspath(project_root),
        cargo_profile,
        target_triple,
        stdlib_profile,
        os.environ.get("CARGO_TARGET_DIR"),
        os.fspath(Path.cwd()),
    )


@functools.lru_cache(maxsize=256)
def _runtime_wasm_artifact_path_cached(
    project_root_str: str,
    artifact_name: str,
    wasm_runtime_dir_override: str | None,
    ext_root_override: str | None,
    cwd_str: str,
) -> Path:
    project_root = Path(project_root_str)
    if wasm_runtime_dir_override:
        base = Path(wasm_runtime_dir_override).expanduser()
    else:
        configured = ext_root_override
        external_root = Path(configured).expanduser() if configured else Path(cwd_str)
        if external_root.is_dir():
            base = external_root / "wasm"
        else:
            base = project_root / "wasm"
    return base / artifact_name


def _runtime_wasm_artifact_path(project_root: Path, artifact_name: str) -> Path:
    return _runtime_wasm_artifact_path_cached(
        os.fspath(project_root),
        artifact_name,
        os.environ.get("MOLT_WASM_RUNTIME_DIR"),
        os.environ.get("MOLT_EXT_ROOT"),
        os.fspath(Path.cwd()),
    )
