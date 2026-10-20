"""Shared stdlib-only user-state paths, including verified bundle bootstrap."""

from __future__ import annotations

import functools
import ntpath
import os
import re
import sys
from collections.abc import Mapping
from pathlib import Path

from .process_environment import ProcessEnvironment


def executable_environment_value(
    environment: Mapping[str, str],
    key: str,
    default: str = "",
) -> str:
    """Read executable-selection inputs under the caller's captured name rule."""
    selected = (
        environment
        if isinstance(environment, ProcessEnvironment)
        else ProcessEnvironment(environment)
    )
    return selected.get(key, default)


def expand_user_path(
    path: str | Path, *, environment: Mapping[str, str] | None = None
) -> Path:
    """Expand a user selector using the selected environment, never ambient keys.

    With no explicit mapping, retain pathlib's host behavior. POSIX named users
    (and a missing HOME) use the system account database, as pathlib does;
    Windows uses USERPROFILE or HOMEDRIVE/HOMEPATH and captured USERNAME.
    """
    raw = os.fspath(path)
    if not raw.startswith("~"):
        return Path(raw)
    if environment is None:
        return Path(raw).expanduser()
    environment = ProcessEnvironment(environment)
    separators = "/\\" if os.name == "nt" else "/"
    end = next(
        (index for index, char in enumerate(raw) if char in separators), len(raw)
    )
    user, suffix = raw[1:end], raw[end:]
    if os.name == "nt":
        home = executable_environment_value(environment, "USERPROFILE")
        if not home:
            homepath = executable_environment_value(environment, "HOMEPATH")
            if not homepath:
                raise ValueError("selected environment has no user home for tilde path")
            home = ntpath.join(
                executable_environment_value(environment, "HOMEDRIVE"), homepath
            )
        if user:
            current_user = executable_environment_value(environment, "USERNAME")
            if user != current_user:
                if ntpath.basename(home.rstrip("\\/")) != current_user:
                    raise ValueError(
                        "selected environment cannot resolve named user home"
                    )
                home = ntpath.join(ntpath.dirname(home.rstrip("\\/")), user)
    else:
        home = environment.get("HOME") if not user else None
        if home is None:
            import pwd

            try:
                home = (
                    pwd.getpwnam(user) if user else pwd.getpwuid(os.getuid())
                ).pw_dir
            except KeyError as exc:
                raise ValueError(
                    "cannot resolve system account for tilde path"
                ) from exc
    if "\0" in home:
        raise ValueError("selected user home contains a NUL byte")
    expanded = home + suffix if os.name == "nt" else home.rstrip(separators) + suffix
    if expanded.startswith("~"):
        raise ValueError("selected user home did not resolve tilde path")
    return Path(expanded or os.sep)


def _default_home_str(environ: Mapping[str, str] | None = None) -> str | None:
    try:
        return os.fspath(
            Path.home()
            if environ is None
            else expand_user_path("~", environment=environ)
        )
    except (RuntimeError, ValueError):
        return None


def _path_override(env: Mapping[str, str], key: str) -> str | None:
    raw = executable_environment_value(env, key)
    return os.fspath(expand_user_path(raw, environment=env)) if raw else None


ARTIFACT_ROOT_ENV = "MOLT_EXT_ROOT"


def configured_artifact_root_text(env: Mapping[str, str] | None = None) -> str | None:
    """Return the operator's ``MOLT_EXT_ROOT`` text exactly, or None when unset.

    For cache keys on hot paths: it touches no filesystem. The cached consumer
    anchors and resolves the value once per distinct key.
    """
    view = os.environ if env is None else env
    raw = executable_environment_value(view, ARTIFACT_ROOT_ENV).strip()
    return raw or None


@functools.lru_cache(maxsize=128)
def _default_molt_cache_cached(
    values: tuple[tuple[str, str], ...],
    cwd_str: str,
    platform_name: str,
    home: str | None,
) -> Path:
    # Keep both precedence and the external-root existence check inside this
    # existing cache. Lower-priority selectors are expanded only if selected.
    env = dict(values)
    operation_cwd = Path(cwd_str)
    selected = _path_override(env, "MOLT_CACHE")
    if selected is None:
        external_text = configured_artifact_root_text(env)
        external = (
            os.fspath(expand_user_path(external_text, environment=env))
            if external_text
            else None
        )
        external_path = Path(external) if external else operation_cwd
        if not external_path.is_absolute():
            external_path = operation_cwd / external_path
        if external and external_path.is_dir():
            selected = os.fspath(external_path / ".molt_cache")
        else:
            windows = platform_name == "win32"
            base = _path_override(env, "LOCALAPPDATA" if windows else "XDG_CACHE_HOME")
            if base is not None:
                selected = os.fspath(Path(base) / ("Molt" if windows else "molt"))
            elif home is None:
                selected = os.fspath(external_path / ".molt_cache")
            elif windows:
                selected = os.fspath(Path(home) / "AppData" / "Local" / "Molt")
            else:
                selected = os.fspath(Path(home) / ".cache" / "molt")
    path = Path(selected)
    return path if path.is_absolute() else (operation_cwd / path).absolute()


def _default_molt_cache(
    *, environ: Mapping[str, str] | None = None, cwd: Path | None = None
) -> Path:
    env = ProcessEnvironment(os.environ if environ is None else environ)
    values = tuple(
        (key, executable_environment_value(env, key))
        for key in (
            "MOLT_CACHE",
            "MOLT_EXT_ROOT",
            "XDG_CACHE_HOME",
            "LOCALAPPDATA",
            "HOME",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "USERNAME",
        )
        if key in env
    )
    return _default_molt_cache_cached(
        values,
        os.fspath(Path.cwd() if cwd is None else cwd),
        sys.platform,
        _default_home_str(environ),
    )


DEFAULT_MOLT_CACHE_MAX_GB = "30"
DEFAULT_MOLT_CACHE_MAX_AGE_DAYS = "30"
CACHE_MAINTENANCE_INTERVAL_SECONDS = 300.0
SHARED_SOURCE_RECORDS_DIRECTORY = "toolchain_source_records"
WASM_LINK_CACHE_DIRECTORY = "wasm_link"
WASM_LINK_CACHE_FAMILIES = frozenset(
    {"runtime_tree_shake", "split_app_optimize", "final_link"}
)
NATIVE_STDLIB_OBJECT_CACHE_DIRECTORY = "native-stdlib-objects-v1"
SHARED_STDLIB_CACHE_DIRECTORY = "shared-stdlib"


def backend_cache_artifact_filename(source_key: str, suffix: str) -> str:
    """Filename projection shared by compiler publication and retention."""
    return (
        source_key.replace("|artifact:", ".artifact-").replace("|stdlib:", ".stdlib-")
        + suffix
    )


def is_backend_cache_artifact_name(name: str) -> bool:
    # Canonical cache keys and contract identities are SHA-256. Reserved private
    # publication stages and unknown/control namespaces cannot match this form.
    return (
        re.fullmatch(
            r"[0-9a-f]{64}\.artifact-[0-9a-f]{64}(?:\.stdlib-[0-9a-f]{64})?"
            r"\.(?:[a-z][a-z0-9]*|symbols\.json)",
            name,
        )
        is not None
    )


def cache_control_root(cache_root: Path) -> Path:
    """Persistent lock ownership within one selected cache namespace."""
    return cache_root / "locks"


def file_digest_cache_path(cache_root: Path) -> Path:
    """The host's stat-keyed file digest store within one cache namespace."""
    return cache_root / "file-digests" / "v1.sqlite"


def shared_cache_lock_path(cache_root: Path, name: str) -> Path:
    """Project the shared producer/reader/evictor lock in durable control state."""
    return cache_control_root(cache_root) / f"{name}.lock"


def wasm_link_cache_paths(
    cache_root: Path, family: str, schema: str, key: str
) -> tuple[Path, Path]:
    """Project one WASM entry and its stable bounded publication lock stripe."""
    if family not in WASM_LINK_CACHE_FAMILIES:
        raise ValueError(f"unknown wasm linker cache family: {family}")
    if (
        not isinstance(schema, str)
        or re.fullmatch(r"[a-z0-9][a-z0-9-]*", schema) is None
    ):
        raise ValueError("wasm linker cache schema must be a lowercase identifier")
    if not isinstance(key, str) or re.fullmatch(r"[0-9a-f]{64}", key) is None:
        raise ValueError("wasm linker cache key must be a lowercase SHA-256")
    family_root = cache_root / family
    # These 256 files must survive every eviction: an open waiter may still
    # refer to a lock inode after its current owner releases it.
    return family_root / schema / key, family_root / ".locks" / f"{key[:2]}.lock"


def shared_stdlib_cache_entry(cache_root: Path, key: str) -> tuple[Path, Path]:
    """One archive generation, including sidecars and private publication stages."""
    if re.fullmatch(r"[0-9a-f]{64}", key) is None:
        raise ValueError("shared stdlib entry key must be a lowercase SHA-256")
    return (
        cache_root / SHARED_STDLIB_CACHE_DIRECTORY / key,
        shared_cache_lock_path(cache_root, f"shared-stdlib.{key[:2]}"),
    )


def native_stdlib_object_cache_entry(cache_root: Path, key: str) -> tuple[Path, Path]:
    """One independently reusable constituent object and its publication stages."""
    if re.fullmatch(r"[0-9a-f]{64}", key) is None:
        raise ValueError("native stdlib object key must be a lowercase SHA-256")
    return (
        cache_root / NATIVE_STDLIB_OBJECT_CACHE_DIRECTORY / key,
        shared_cache_lock_path(cache_root, f"native-stdlib-object.{key[:2]}"),
    )


def shared_stdlib_archive_cache_root(archive: Path) -> Path | None:
    if (
        archive.name != "stdlib.a"
        or archive.parent.parent.name != SHARED_STDLIB_CACHE_DIRECTORY
    ):
        return None
    root = archive.parent.parent.parent
    shared_stdlib_cache_entry(root, archive.parent.name)
    return root


def shared_stdlib_publish_lock_path(archive: Path) -> Path:
    """Shared entry stripes; standalone explicit archives retain their own lock."""
    root = shared_stdlib_archive_cache_root(archive)
    if root is not None:
        _entry, lock = shared_stdlib_cache_entry(root, archive.parent.name)
        return lock
    return archive.with_name(f"{archive.name}.publish.lock")


def validate_required_stdlib_output(output: Path, source: Path | None = None) -> None:
    """Required build bytes cannot remain in an evictable shared generation."""
    resolved = output.resolve()
    invalid = source is not None and source.resolve() == resolved
    invalid |= shared_stdlib_publish_lock_path(resolved) != resolved.with_name(
        f"{resolved.name}.publish.lock"
    )
    root = (
        shared_stdlib_archive_cache_root(source.resolve())
        if source is not None
        else None
    )
    if root is None and os.environ.get("MOLT_CACHE"):
        root = Path(os.environ["MOLT_CACHE"]).resolve()
    if root is not None and resolved.is_relative_to(root):
        # Canonical home is protected across every maintenance environment.
        # A selected MOLT_HOME/MOLT_BIN alone cannot promise future retention.
        home = cache_home_root(root).resolve()
        invalid |= not resolved.is_relative_to(home)
    if invalid:
        raise ValueError(
            "Required build-owned stdlib output must be outside cache entries or inside canonical cache/home, and distinct from its source"
        )


def cache_home_root(cache_root: Path) -> Path:
    """The default durable user home, never an evictable cache entry."""
    return cache_root / "home"


@functools.lru_cache(maxsize=128)
def _default_molt_home_cached(path: str, cwd_str: str) -> Path:
    selected = Path(path)
    return selected if selected.is_absolute() else (Path(cwd_str) / selected).absolute()


def _default_molt_home(
    *, environ: Mapping[str, str] | None = None, cwd: Path | None = None
) -> Path:
    env = os.environ if environ is None else environ
    selected = _path_override(env, "MOLT_HOME")
    if selected is None:
        selected = os.fspath(
            cache_home_root(_default_molt_cache(environ=environ, cwd=cwd))
        )
    return _default_molt_home_cached(
        selected, os.fspath(Path.cwd() if cwd is None else cwd)
    )


@functools.lru_cache(maxsize=128)
def _default_molt_bin_cached(path: str, cwd_str: str) -> Path:
    selected = Path(path)
    return selected if selected.is_absolute() else (Path(cwd_str) / selected).absolute()


def _default_molt_bin(
    *, environ: Mapping[str, str] | None = None, cwd: Path | None = None
) -> Path:
    env = os.environ if environ is None else environ
    selected = _path_override(env, "MOLT_BIN")
    if selected is None:
        selected = os.fspath(_default_molt_home(environ=environ, cwd=cwd) / "bin")
    return _default_molt_bin_cached(
        selected, os.fspath(Path.cwd() if cwd is None else cwd)
    )


def protected_cache_roots(
    cache_root: Path, *, environ: Mapping[str, str] | None = None
) -> tuple[Path, ...]:
    """Return the top-level cache children that hold the selected home or bin.

    The default Molt home lives in the cache and owns installed tool state
    (``<MOLT_HOME>/target-root``) and the bin directory; neither is an evictable
    cache entry. Defaults bind to the cache being pruned, even when
    ``--cache-dir`` names another namespace than the process's ``MOLT_CACHE``.
    """
    root = cache_root.resolve(strict=False)
    environment = dict(os.environ if environ is None else environ)
    environment["MOLT_CACHE"] = os.fspath(root)
    protected = {cache_control_root(root), cache_home_root(root)}
    for selected in (
        cache_home_root(root),
        _default_molt_home(environ=environment),
        _default_molt_bin(environ=environment),
    ):
        for candidate in (selected.absolute(), selected.resolve(strict=False)):
            try:
                relative = candidate.relative_to(root)
            except ValueError:
                continue
            protected.add(root / relative.parts[0] if relative.parts else root)
    return tuple(sorted(protected, key=os.fspath))
