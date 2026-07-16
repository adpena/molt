"""Canonical intrinsic feature and target-availability classification.

The data authority is ``intrinsics/categories.toml``.  Both intrinsic resolver
generation and the WASM ABI generator consume these helpers so prefix matching,
validation, and feature/target projection cannot drift between backends.
"""

from __future__ import annotations

import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import TypeAlias

FeatureGates: TypeAlias = tuple[tuple[str, str], ...]
TargetArchExclusions: TypeAlias = tuple[tuple[str, tuple[str, ...]], ...]
FeatureTargetArchExclusions: TypeAlias = tuple[tuple[str, tuple[str, ...]], ...]


@dataclass(frozen=True)
class IntrinsicAvailability:
    feature_gates: FeatureGates
    target_arch_exclusions: TargetArchExclusions
    feature_target_arch_exclusions: FeatureTargetArchExclusions


def _string_list(value: object, *, field: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(
        isinstance(item, str) and item for item in value
    ):
        raise TypeError(f"{field} must be a list of non-empty strings")
    return tuple(dict.fromkeys(value))


def load_intrinsic_availability(
    categories_path: Path,
) -> IntrinsicAvailability:
    """Load canonical prefix feature/target facts from *categories_path*."""
    data = tomllib.loads(categories_path.read_bytes().decode())
    feature_gates: list[tuple[str, str]] = []
    target_exclusions: list[tuple[str, tuple[str, ...]]] = []
    feature_target_exclusions: dict[str, set[str]] = {}

    for mod_name, mod_data in data.get("stdlib", {}).items():
        if not isinstance(mod_data, dict):
            raise TypeError(f"stdlib.{mod_name} must be a table")
        prefixes = _string_list(
            mod_data.get("prefixes", []), field=f"stdlib.{mod_name}.prefixes"
        )
        feature = mod_data.get("feature")
        feature_prefixes: tuple[str, ...] = ()
        if feature is not None:
            if not isinstance(feature, str) or not feature:
                raise TypeError(f"stdlib.{mod_name}.feature must be a non-empty string")
            feature_prefixes = _string_list(
                mod_data.get("feature_prefixes", list(prefixes)),
                field=f"stdlib.{mod_name}.feature_prefixes",
            )
            feature_gates.extend(
                (f"molt_{prefix}", feature) for prefix in feature_prefixes
            )

        raw_arches = mod_data.get("unsupported_target_arches", [])
        if raw_arches:
            arches = _string_list(
                raw_arches, field=f"stdlib.{mod_name}.unsupported_target_arches"
            )
            target_prefixes = _string_list(
                mod_data.get("target_prefixes", list(prefixes)),
                field=f"stdlib.{mod_name}.target_prefixes",
            )
            target_exclusions.extend(
                (f"molt_{prefix}", arches) for prefix in target_prefixes
            )
            # A Cargo feature is target-unavailable only when every symbol
            # prefix it gates is covered by this module's target exclusion.
            # This derives profile construction without a second feature list.
            if feature is not None and set(feature_prefixes) <= set(target_prefixes):
                feature_target_exclusions.setdefault(feature, set()).update(arches)

    return IntrinsicAvailability(
        feature_gates=tuple(feature_gates),
        target_arch_exclusions=tuple(target_exclusions),
        feature_target_arch_exclusions=tuple(
            (feature, tuple(sorted(arches)))
            for feature, arches in sorted(feature_target_exclusions.items())
        ),
    )


def longest_prefix_value[T](symbol: str, rules: tuple[tuple[str, T], ...]) -> T | None:
    """Return the value owned by the longest matching symbol prefix."""
    best: tuple[int, T] | None = None
    for prefix, value in rules:
        if symbol.startswith(prefix):
            prefix_len = len(prefix)
            if best is None or prefix_len > best[0]:
                best = (prefix_len, value)
    return best[1] if best is not None else None


def symbol_available_on_target_arch(
    symbol: str,
    target_arch: str,
    target_exclusions: TargetArchExclusions,
) -> bool:
    arches = longest_prefix_value(symbol, target_exclusions) or ()
    return target_arch not in arches
