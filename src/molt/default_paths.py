"""Shared stdlib-only user-state paths, including verified bundle bootstrap."""

from __future__ import annotations

from collections.abc import Mapping
import functools
import ntpath
import os
import sys
from pathlib import Path


def executable_environment_value(
    environment: Mapping[str, str],
    key: str,
    default: str = "",
    *,
    windows: bool | None = None,
) -> str:
    """Read captured executable-selection inputs with host key semantics."""
    if windows is None:
        windows = os.name == "nt"
    if not windows:
        return environment.get(key, default)
    values = {
        value
        for name, value in environment.items()
        if name.casefold() == key.casefold()
    }
    if len(values) > 1:
        raise ValueError(f"conflicting captured environment spellings for {key}")
    return next(iter(values), default)


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
    env = os.environ if environ is None else environ
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
        or (os.name == "nt" and any(name.casefold() == key.casefold() for name in env))
    )
    return _default_molt_cache_cached(
        values,
        os.fspath(Path.cwd() if cwd is None else cwd),
        sys.platform,
        _default_home_str(environ),
    )


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
        selected = os.fspath(_default_molt_cache(environ=environ, cwd=cwd) / "home")
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
