from __future__ import annotations

import functools
import os
import sys
from pathlib import Path


@functools.lru_cache(maxsize=128)
def _default_molt_cache_cached(
    cache_override: str | None,
    xdg_cache_home: str | None,
    cwd_str: str,
    home_str: str | None,
    platform_name: str,
    ext_root_str: str | None,
) -> Path:
    if cache_override:
        path = Path(cache_override).expanduser()
        if not path.is_absolute():
            path = (Path(cwd_str) / path).absolute()
        return path
    if ext_root_str:
        ext_root = Path(ext_root_str).expanduser()
        if not ext_root.is_absolute():
            ext_root = (Path(cwd_str) / ext_root).absolute()
        if ext_root.is_dir():
            return ext_root / ".molt_cache"
    if platform_name == "win32":
        local_app_data = os.environ.get("LOCALAPPDATA")
        if local_app_data:
            base = Path(local_app_data)
        elif home_str is None:
            fallback_base = Path(ext_root_str) if ext_root_str else Path(cwd_str)
            if not fallback_base.is_absolute():
                fallback_base = (Path(cwd_str) / fallback_base).absolute()
            return fallback_base / ".molt_cache"
        else:
            base = Path(home_str) / "AppData" / "Local"
        return base / "Molt"
    if xdg_cache_home:
        base = Path(xdg_cache_home).expanduser()
        if not base.is_absolute():
            base = (Path(cwd_str) / base).absolute()
    elif home_str is None:
        fallback_base = Path(ext_root_str) if ext_root_str else Path(cwd_str)
        if not fallback_base.is_absolute():
            fallback_base = (Path(cwd_str) / fallback_base).absolute()
        return fallback_base / ".molt_cache"
    else:
        base = Path(home_str) / ".cache"
    return base / "molt"


def _default_home_str() -> str | None:
    try:
        return os.fspath(Path.home())
    except RuntimeError:
        return None


def _default_molt_cache() -> Path:
    return _default_molt_cache_cached(
        os.environ.get("MOLT_CACHE"),
        os.environ.get("XDG_CACHE_HOME"),
        os.fspath(Path.cwd()),
        _default_home_str(),
        sys.platform,
        os.environ.get("MOLT_EXT_ROOT"),
    )


@functools.lru_cache(maxsize=128)
def _default_molt_home_cached(
    home_override: str | None,
    cache_override: str | None,
    xdg_cache_home: str | None,
    cwd_str: str,
    home_str: str | None,
    platform_name: str,
    ext_root_str: str | None,
) -> Path:
    if home_override:
        path = Path(home_override).expanduser()
        if not path.is_absolute():
            path = (Path(cwd_str) / path).absolute()
        return path
    return (
        _default_molt_cache_cached(
            cache_override,
            xdg_cache_home,
            cwd_str,
            home_str,
            platform_name,
            ext_root_str,
        )
        / "home"
    )


def _default_molt_home() -> Path:
    return _default_molt_home_cached(
        os.environ.get("MOLT_HOME"),
        os.environ.get("MOLT_CACHE"),
        os.environ.get("XDG_CACHE_HOME"),
        os.fspath(Path.cwd()),
        _default_home_str(),
        sys.platform,
        os.environ.get("MOLT_EXT_ROOT"),
    )


@functools.lru_cache(maxsize=128)
def _default_molt_bin_cached(
    bin_override: str | None,
    home_override: str | None,
    cache_override: str | None,
    xdg_cache_home: str | None,
    cwd_str: str,
    home_str: str | None,
    platform_name: str,
    ext_root_str: str | None,
) -> Path:
    if bin_override:
        path = Path(bin_override).expanduser()
        if not path.is_absolute():
            path = (Path(cwd_str) / path).absolute()
        return path
    return (
        _default_molt_home_cached(
            home_override,
            cache_override,
            xdg_cache_home,
            cwd_str,
            home_str,
            platform_name,
            ext_root_str,
        )
        / "bin"
    )


def _default_molt_bin() -> Path:
    return _default_molt_bin_cached(
        os.environ.get("MOLT_BIN"),
        os.environ.get("MOLT_HOME"),
        os.environ.get("MOLT_CACHE"),
        os.environ.get("XDG_CACHE_HOME"),
        os.fspath(Path.cwd()),
        _default_home_str(),
        sys.platform,
        os.environ.get("MOLT_EXT_ROOT"),
    )
