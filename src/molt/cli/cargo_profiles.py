from __future__ import annotations

import functools
import os
import re
from typing import cast

from molt.cli.models import BuildProfile
from molt.cli.runtime_paths import _cargo_profile_dir

_CARGO_PROFILE_NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]*$")


@functools.lru_cache(maxsize=32)
def _resolve_backend_profile_cached(
    default_profile: BuildProfile,
    raw: str | None,
) -> tuple[BuildProfile, str | None]:
    if not raw:
        return default_profile, None
    value = raw.strip().lower()
    if value not in {"dev", "release"}:
        return default_profile, f"Invalid MOLT_BACKEND_PROFILE value: {raw}"
    return cast(BuildProfile, value), None


def _resolve_backend_profile(
    default_profile: BuildProfile,
) -> tuple[BuildProfile, str | None]:
    return _resolve_backend_profile_cached(
        default_profile,
        os.environ.get("MOLT_BACKEND_PROFILE"),
    )


@functools.lru_cache(maxsize=32)
def _resolve_cargo_profile_name_cached(
    build_profile: BuildProfile,
    raw: str,
) -> tuple[str, str | None]:
    env_var = (
        "MOLT_DEV_CARGO_PROFILE"
        if build_profile == "dev"
        else "MOLT_RELEASE_CARGO_PROFILE"
    )
    normalized_raw = raw.strip()
    # dev-fast is the correct default for development: it has debug info plus
    # incremental compilation. The plain "dev" profile is unoptimized and
    # produces a much larger binary.
    # release-output is the release default for the runtime staticlib: it uses
    # panic=abort and opt-level "z" for minimal binary size. The backend daemon
    # uses release-fast via _resolve_backend_cargo_profile_name for fast
    # optimized rebuilds.
    default_profile = "dev-fast" if build_profile == "dev" else "release-output"
    profile_name = normalized_raw or default_profile
    if not _CARGO_PROFILE_NAME_RE.match(profile_name):
        return default_profile, f"Invalid {env_var} value: {raw}"
    return profile_name, None


def _resolve_cargo_profile_name(
    build_profile: BuildProfile,
) -> tuple[str, str | None]:
    env_var = (
        "MOLT_DEV_CARGO_PROFILE"
        if build_profile == "dev"
        else "MOLT_RELEASE_CARGO_PROFILE"
    )
    return _resolve_cargo_profile_name_cached(
        build_profile,
        os.environ.get(env_var, ""),
    )


@functools.lru_cache(maxsize=32)
def _resolve_backend_cargo_profile_name_cached(
    build_profile: BuildProfile,
    backend_raw: str,
    generic_raw: str,
) -> tuple[str, str | None]:
    backend_env_var = (
        "MOLT_DEV_BACKEND_CARGO_PROFILE"
        if build_profile == "dev"
        else "MOLT_RELEASE_BACKEND_CARGO_PROFILE"
    )
    generic_env_var = (
        "MOLT_DEV_CARGO_PROFILE"
        if build_profile == "dev"
        else "MOLT_RELEASE_CARGO_PROFILE"
    )
    normalized_backend = backend_raw.strip()
    normalized_generic = generic_raw.strip()
    default_profile = "dev-fast" if build_profile == "dev" else "release-fast"
    profile_name = normalized_backend or normalized_generic
    if not profile_name:
        profile_name = default_profile
    if not _CARGO_PROFILE_NAME_RE.match(profile_name):
        if normalized_backend:
            return default_profile, f"Invalid {backend_env_var} value: {backend_raw}"
        return default_profile, f"Invalid {generic_env_var} value: {generic_raw}"
    return profile_name, None


def _resolve_backend_cargo_profile_name(
    build_profile: BuildProfile,
) -> tuple[str, str | None]:
    backend_env_var = (
        "MOLT_DEV_BACKEND_CARGO_PROFILE"
        if build_profile == "dev"
        else "MOLT_RELEASE_BACKEND_CARGO_PROFILE"
    )
    generic_env_var = (
        "MOLT_DEV_CARGO_PROFILE"
        if build_profile == "dev"
        else "MOLT_RELEASE_CARGO_PROFILE"
    )
    return _resolve_backend_cargo_profile_name_cached(
        build_profile,
        os.environ.get(backend_env_var, ""),
        os.environ.get(generic_env_var, ""),
    )


def _active_artifact_profile_dirs() -> tuple[str, ...]:
    """Cargo profile directories that may hold live Molt build artifacts."""
    profiles = [
        "release",
        "release-fast",
        "debug",
        _resolve_cargo_profile_name("dev")[0],
        _resolve_cargo_profile_name("release")[0],
        _resolve_backend_cargo_profile_name("dev")[0],
        _resolve_backend_cargo_profile_name("release")[0],
    ]
    seen: set[str] = set()
    profile_dirs: list[str] = []
    for profile in profiles:
        profile_dir = _cargo_profile_dir(profile)
        if profile_dir in seen:
            continue
        seen.add(profile_dir)
        profile_dirs.append(profile_dir)
    return tuple(profile_dirs)
