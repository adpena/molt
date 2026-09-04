"""Capability registry for Molt host access."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from molt import intrinsics as _intrinsics
from molt._host_capabilities_generated import (
    CAPABILITY_TIERS,
    DEFAULT_CAPABILITY_TIER,
    MAXIMUM_BUILTIN_CAPABILITY_TIER,
    capabilities_for_tier,
)

if TYPE_CHECKING:
    from collections.abc import Callable, Iterable
else:

    class _TypingAlias:
        __slots__ = ()

        def __getitem__(self, _item):
            return self

    Callable = _TypingAlias()
    Iterable = _TypingAlias()


def _parse_caps(raw: str) -> set[str]:
    caps: set[str] = set()
    for part in raw.split(","):
        stripped = part.strip()
        if stripped:
            caps.add(stripped)
    return caps


def _load_intrinsic(name: str) -> Callable[..., Any] | None:
    try:
        return _intrinsics.require(name, globals())
    except RuntimeError:
        return None


def _env_get(key: str, default: str = "") -> str:
    import os

    fn = _load_intrinsic("molt_env_get")
    if fn is not None:
        value = fn(key, default)
        return str(value)
    return os.environ.get(key, default)


def capabilities() -> set[str]:
    tier = _env_get("MOLT_CAPABILITY_TIER", DEFAULT_CAPABILITY_TIER)
    tier_capabilities = capabilities_for_tier(tier)
    # Unknown tiers are an invalid configuration and therefore grant nothing;
    # a typo must never select a more permissive fallback.
    return set(tier_capabilities or ()) | _parse_caps(_env_get("MOLT_CAPABILITIES", ""))


def trusted() -> bool:
    """Report whether the explicit finite ``full`` tier is selected.

    This is diagnostic state only.  Capability enforcement always checks the
    exact resolved grant set; selecting ``full`` never bypasses future or
    package-scoped permissions.
    """

    fn = _load_intrinsic("molt_capabilities_trusted")
    if fn is not None:
        return bool(fn())
    tier = _env_get("MOLT_CAPABILITY_TIER", DEFAULT_CAPABILITY_TIER)
    return (
        tier.strip().casefold() == MAXIMUM_BUILTIN_CAPABILITY_TIER
        and MAXIMUM_BUILTIN_CAPABILITY_TIER in CAPABILITY_TIERS
    )


def has(capability: str) -> bool:
    fn = _load_intrinsic("molt_capabilities_has")
    if fn is not None:
        return bool(fn(capability))
    return capability in capabilities()


def require(capability: str) -> None:
    fn = _load_intrinsic("molt_capabilities_require")
    if fn is not None:
        fn(capability)
        return None
    if capability not in capabilities():
        raise PermissionError(
            f"capability '{capability}' is not granted (MOLT_CAPABILITIES={_env_get('MOLT_CAPABILITIES', '')})"
        )
    return None


def format_caps(caps: Iterable[str]) -> str:
    items = list(set(caps))
    items.sort()
    return ",".join(items)
