"""Capability registry for Molt host access."""

from __future__ import annotations

import os
from collections.abc import Iterable


def _parse_caps(raw: str) -> set[str]:
    return {cap.strip() for cap in raw.split(",") if cap.strip()}


def capabilities() -> set[str]:
    return _parse_caps(os.getenv("MOLT_CAPABILITIES", ""))


def has(capability: str) -> bool:
    return capability in capabilities()


def require(capability: str) -> None:
    if not has(capability):
        raise PermissionError("Missing capability")


def format_caps(caps: Iterable[str]) -> str:
    return ",".join(sorted(set(caps)))
