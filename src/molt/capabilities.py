"""Capability registry for Molt host access."""

from __future__ import annotations

import os
from collections.abc import Iterable


def _parse_caps(raw: str) -> set[str]:
    caps: set[str] = set()
    for part in raw.split(","):
        stripped = part.strip()
        if stripped:
            caps.add(stripped)
    return caps


def _raw_getenv(key: str, default: str = "") -> str:
    getter = getattr(os, "_molt_env_get", None)
    if getter is not None:
        try:
            return getter(key, default)
        except Exception:
            return default
    try:
        return os.getenv(key, default)
    except Exception:
        return default


_CAPS_CACHE: set[str] | None = None
_CAPS_RAW: str | None = None
_TRUSTED_CACHE: bool | None = None
_TRUSTED_RAW: str | None = None


def _caps_cache() -> set[str]:
    global _CAPS_CACHE, _CAPS_RAW
    raw = _raw_getenv("MOLT_CAPABILITIES", "")
    if _CAPS_CACHE is None or raw != _CAPS_RAW:
        _CAPS_RAW = raw
        _CAPS_CACHE = _parse_caps(raw)
    return _CAPS_CACHE


def _trusted_cache() -> bool:
    global _TRUSTED_CACHE, _TRUSTED_RAW
    raw = _raw_getenv("MOLT_TRUSTED", "")
    if _TRUSTED_CACHE is None or raw != _TRUSTED_RAW:
        _TRUSTED_RAW = raw
        _TRUSTED_CACHE = raw.strip().lower() in {"1", "true", "yes", "on"}
    return _TRUSTED_CACHE


def capabilities() -> set[str]:
    return set(_caps_cache())


def trusted() -> bool:
    return _trusted_cache()


def has(capability: str) -> bool:
    if _trusted_cache():
        return True
    return capability in _caps_cache()


def require(capability: str) -> None:
    if not has(capability):
        raise PermissionError("Missing capability")


def format_caps(caps: Iterable[str]) -> str:
    items = list(set(caps))
    items.sort()
    return ",".join(items)
