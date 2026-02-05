"""Capability registry for Molt host access."""

from __future__ import annotations

from collections.abc import Iterable

from _intrinsics import require_intrinsic as _intrinsics_require


def _parse_caps(raw: str) -> set[str]:
    caps: set[str] = set()
    for part in raw.split(","):
        stripped = part.strip()
        if stripped:
            caps.add(stripped)
    return caps


_MOLT_ENV_GET = _intrinsics_require("molt_env_get", globals())
_MOLT_CAP_TRUSTED = _intrinsics_require("molt_capabilities_trusted", globals())
_MOLT_CAP_HAS = _intrinsics_require("molt_capabilities_has", globals())
_MOLT_CAP_REQUIRE = _intrinsics_require("molt_capabilities_require", globals())


def _require_intrinsic(name: str, value: object) -> object:
    if not callable(value):
        raise RuntimeError(f"{name} intrinsic unavailable")
    return value


def _env_get(key: str, default: str = "") -> str:
    getter = _require_intrinsic("molt_env_get", _MOLT_ENV_GET)
    value = getter(key, default)
    return str(value)


def capabilities() -> set[str]:
    raw = _env_get("MOLT_CAPABILITIES", "")
    return _parse_caps(raw)


def trusted() -> bool:
    fn = _require_intrinsic("molt_capabilities_trusted", _MOLT_CAP_TRUSTED)
    return bool(fn())


def has(capability: str) -> bool:
    fn = _require_intrinsic("molt_capabilities_has", _MOLT_CAP_HAS)
    return bool(fn(capability))


def require(capability: str) -> None:
    fn = _require_intrinsic("molt_capabilities_require", _MOLT_CAP_REQUIRE)
    fn(capability)
    return None


def format_caps(caps: Iterable[str]) -> str:
    items = list(set(caps))
    items.sort()
    return ",".join(items)
