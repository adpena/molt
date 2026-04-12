"""Capability registry for Molt host access."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from molt import intrinsics as _intrinsics

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
    raw = _env_get("MOLT_CAPABILITIES", "")
    return _parse_caps(raw)


def trusted() -> bool:
    fn = _load_intrinsic("molt_capabilities_trusted")
    if fn is not None:
        return bool(fn())
    raw = _env_get("MOLT_TRUSTED", "")
    return raw.strip() not in ("", "0", "false", "no")


def has(capability: str) -> bool:
    if trusted():
        return True
    fn = _load_intrinsic("molt_capabilities_has")
    if fn is not None:
        return bool(fn(capability))
    return capability in capabilities()


def require(capability: str) -> None:
    if trusted():
        return None
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
