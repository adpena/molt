"""Capability registry for Molt host access."""

from __future__ import annotations

from typing import TYPE_CHECKING

from molt import intrinsics as _intrinsics
from molt._host_capabilities_generated import (
    MAXIMUM_BUILTIN_CAPABILITY_TIER,
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


def _query(name: str, *args):
    # Compiled diagnostics expose the immutable runtime policy. Host tooling
    # resolves the same canonical manifest authority at its own boundary.
    if _intrinsics.runtime_active():
        return _intrinsics.require(name, globals())(*args)
    import os
    from molt.capability_manifest import resolve_runtime_policy_from_env

    policy = resolve_runtime_policy_from_env(os.environ)
    if name == "molt_capabilities_list":
        return list(policy.grants.capabilities)
    if name == "molt_capabilities_trusted":
        return policy.tier == MAXIMUM_BUILTIN_CAPABILITY_TIER
    if name == "molt_capabilities_has":
        return args[0] in policy.grants.capabilities
    if args[0] not in policy.grants.capabilities:
        raise PermissionError(f"missing '{args[0]}' capability")
    return None


def capabilities() -> set[str]:
    return set(_query("molt_capabilities_list"))


def trusted() -> bool:
    return bool(_query("molt_capabilities_trusted"))


def has(capability: str) -> bool:
    return bool(_query("molt_capabilities_has", capability))


def require(capability: str) -> None:
    _query("molt_capabilities_require", capability)


def format_caps(caps: Iterable[str]) -> str:
    return ",".join(sorted(set(caps)))
