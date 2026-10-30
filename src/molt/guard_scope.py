"""The guard scope authority: registered stems and the names they compose.

``src/molt/environment_registry.toml`` declares every guard scope as a
``[[stem]]`` row and every budget as a ``[[family]]`` suffix;
``molt.guard_stems_generated`` projects both. Guards compose
``<STEM><SUFFIX>`` names only from a registered stem, so no undeclared name
reaches a process and ``tools/check_environment_registry.py`` sees every name.
"""

from __future__ import annotations

from typing import TypeGuard

from .guard_stems_generated import GUARD_STEMS, GuardStem


class UnregisteredGuardStemError(ValueError):
    """A guard scope stem that no ``[[stem]]`` registry row declares."""


def is_guard_stem(value: str) -> TypeGuard[GuardStem]:
    return value in GUARD_STEMS


def require_guard_stem(prefix: str | None) -> str:
    """``prefix`` as a registered stem spelled exactly, or "" for the root scope."""

    if not prefix:
        return ""
    if not is_guard_stem(prefix):
        raise UnregisteredGuardStemError(
            f"guard stem {prefix!r} is not a [[stem]] row of "
            "src/molt/environment_registry.toml; use a registered stem "
            "(molt.guard_stems_generated.GuardStem) or register the scope"
        )
    return prefix
