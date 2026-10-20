from __future__ import annotations

import functools
import os
from collections.abc import Mapping
from pathlib import Path

from molt.build_state_layout import build_state_root

from molt.cli.config_resolution import (
    DEFAULT_RUNTIME_STDLIB_PROFILE,
    RUNTIME_STDLIB_PROFILE_TIERS,
    RuntimeStdlibProfile,
)
from molt.cli.build_output_layout import _wasm_runtime_root_from_env
from molt.default_paths import configured_artifact_root_text
from molt.dx import project_cargo_target_dir

_RUNTIME_STDLIB_PROFILE_ALIASES = {
    profile: f"stdlib_{profile}" for profile in RUNTIME_STDLIB_PROFILE_TIERS
}


def _normalize_runtime_stdlib_profile(
    stdlib_profile: str | None,
) -> RuntimeStdlibProfile:
    profile = stdlib_profile or DEFAULT_RUNTIME_STDLIB_PROFILE
    if profile not in RUNTIME_STDLIB_PROFILE_TIERS:
        choices = "', '".join(RUNTIME_STDLIB_PROFILE_TIERS)
        raise ValueError(f"runtime stdlib_profile must be one of '{choices}'")
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
        *(
            _runtime_lib_archive_name(profile, target_triple)
            for profile in RUNTIME_STDLIB_PROFILE_TIERS
        ),
        _runtime_cargo_scratch_lib_name(target_triple),
    ]
    return tuple(dict.fromkeys(names))


def _molt_session_id() -> str | None:
    return os.environ.get("MOLT_SESSION_ID")


@functools.lru_cache(maxsize=64)
def _cargo_profile_dir(cargo_profile: str) -> str:
    return "debug" if cargo_profile == "dev" else cargo_profile


class _RecordingEnvironment(Mapping[str, str]):
    """Read-through view of an environment that records the keys it serves."""

    def __init__(self, environment: Mapping[str, str]) -> None:
        self._environment = environment
        self._read: dict[str, str | None] = {}
        self.iterated = False

    def __getitem__(self, key: str) -> str:
        value = self._environment.get(key)
        self._read[key] = value
        if value is None:
            raise KeyError(key)
        return value

    def __contains__(self, key: object) -> bool:
        if not isinstance(key, str):
            return False
        value = self._environment.get(key)
        self._read[key] = value
        return value is not None

    def __iter__(self):
        self.iterated = True
        return iter(self._environment)

    def __len__(self) -> int:
        self.iterated = True
        return len(self._environment)

    def observed(self) -> tuple[tuple[str, str | None], ...]:
        return tuple(self._read.items())


# (project root, cwd) -> (environment keys read with their values, target).
_CARGO_TARGET_ROOTS: dict[
    tuple[str, str], tuple[tuple[tuple[str, str | None], ...], Path]
] = {}


def _cargo_target_root(project_root: Path) -> Path:
    """The Cargo target this process builds ``project_root`` into (molt.dx).

    Resolution costs about 0.15 ms and a build asks about 11,000 times, once
    per import-scan record. The memo is keyed by the environment keys the
    resolution actually read, so a changed value, or a key molt.dx starts to
    read, recomputes. A resolution that enumerates the environment is not
    memoized.
    """
    key = (os.fspath(project_root), os.getcwd())
    cached = _CARGO_TARGET_ROOTS.get(key)
    if cached is not None and all(
        os.environ.get(name) == value for name, value in cached[0]
    ):
        return cached[1]
    environment = _RecordingEnvironment(os.environ)
    target = project_cargo_target_dir(project_root, environment)
    if not environment.iterated:
        _CARGO_TARGET_ROOTS[key] = (environment.observed(), target)
    return target


@functools.lru_cache(maxsize=256)
def _build_state_root_cached(
    project_root_str: str,
    build_state_override: str | None,
    cargo_target_str: str,
    artifact_root: str | None = None,
) -> Path:
    environment = {}
    if build_state_override:
        environment["MOLT_BUILD_STATE_DIR"] = build_state_override
    if artifact_root:
        environment["MOLT_EXT_ROOT"] = artifact_root
    return build_state_root(
        project_root=Path(project_root_str),
        cargo_target=Path(cargo_target_str),
        environment=environment,
    )


def _build_state_root(project_root: Path) -> Path:
    return _build_state_root_cached(
        os.fspath(project_root),
        os.environ.get("MOLT_BUILD_STATE_DIR"),
        os.fspath(_cargo_target_root(project_root)),
        configured_artifact_root_text(os.environ),
    )


@functools.lru_cache(maxsize=256)
def _runtime_lib_path_cached(
    cargo_target_str: str,
    cargo_profile: str,
    target_triple: str | None,
    stdlib_profile: str | None,
) -> Path:
    profile_dir = _cargo_profile_dir(cargo_profile)
    target_root = Path(cargo_target_str)
    archive_name = _runtime_lib_archive_name(stdlib_profile, target_triple)
    if target_triple:
        return target_root / target_triple / profile_dir / archive_name
    return target_root / profile_dir / archive_name


def _runtime_lib_path(
    project_root: Path,
    cargo_profile: str,
    target_triple: str | None,
    stdlib_profile: str | None = None,
) -> Path:
    return _runtime_lib_path_cached(
        os.fspath(_cargo_target_root(project_root)),
        cargo_profile,
        target_triple,
        stdlib_profile,
    )


def _runtime_wasm_artifact_path(project_root: Path, artifact_name: str) -> Path:
    return _runtime_wasm_artifact_path_from_env(project_root, artifact_name, os.environ)


def _runtime_wasm_artifact_path_from_env(
    project_root: Path,
    artifact_name: str,
    env: Mapping[str, str],
) -> Path:
    return _wasm_runtime_root_from_env(project_root, env) / artifact_name
