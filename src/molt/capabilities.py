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


def capabilities() -> set[str]:
    return _parse_caps(_raw_getenv("MOLT_CAPABILITIES", ""))


def has(capability: str) -> bool:
    return capability in capabilities()


def require(capability: str) -> None:
    if not has(capability):
        raise PermissionError("Missing capability")


def format_caps(caps: Iterable[str]) -> str:
    return ",".join(sorted(set(caps)))
