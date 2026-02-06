"""Capability registry for Molt host access."""

from __future__ import annotations

from collections.abc import Callable, Iterable
from typing import Any

import builtins as _builtins


def _parse_caps(raw: str) -> set[str]:
    caps: set[str] = set()
    for part in raw.split(","):
        stripped = part.strip()
        if stripped:
            caps.add(stripped)
    return caps


def _load_intrinsic(name: str) -> Callable[..., Any]:
    value = globals().get(name)
    if callable(value):
        return value
    direct = getattr(_builtins, name, None)
    if callable(direct):
        return direct
    reg = getattr(_builtins, "_molt_intrinsics", None)
    if isinstance(reg, dict):
        value = reg.get(name)
        if callable(value):
            return value
    raise RuntimeError(f"{name} intrinsic unavailable")


def _env_get(key: str, default: str = "") -> str:
    getter = _load_intrinsic("molt_env_get")
    value = getter(key, default)
    return str(value)


def capabilities() -> set[str]:
    raw = _env_get("MOLT_CAPABILITIES", "")
    return _parse_caps(raw)


def trusted() -> bool:
    fn = _load_intrinsic("molt_capabilities_trusted")
    return bool(fn())


def has(capability: str) -> bool:
    fn = _load_intrinsic("molt_capabilities_has")
    return bool(fn(capability))


def require(capability: str) -> None:
    fn = _load_intrinsic("molt_capabilities_require")
    fn(capability)
    return None


def format_caps(caps: Iterable[str]) -> str:
    items = list(set(caps))
    items.sort()
    return ",".join(items)
