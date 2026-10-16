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

from molt._host_capabilities_generated import SUPPORTED_TARGET_PLATFORMS

FeatureGates: TypeAlias = tuple[tuple[str, str], ...]
TargetArchExclusions: TypeAlias = tuple[tuple[str, tuple[str, ...]], ...]
TargetOsRequirements: TypeAlias = tuple[tuple[str, tuple[str, ...]], ...]
FeatureTargetArchExclusions: TypeAlias = tuple[tuple[str, tuple[str, ...]], ...]


@dataclass(frozen=True)
class IntrinsicAvailability:
    builtin_symbols: tuple[str, ...]
    feature_gates: FeatureGates
    target_arch_exclusions: TargetArchExclusions
    target_os_requirements: TargetOsRequirements
    feature_target_arch_exclusions: FeatureTargetArchExclusions

    def feature_gate_for_symbol(self, symbol: str) -> str | None:
        """Return the feature owning *symbol*, with exact categories first."""
        if symbol in self.builtin_symbols:
            return None
        return longest_prefix_value(symbol, self.feature_gates)

    def target_arch_exclusions_for_symbol(self, symbol: str) -> tuple[str, ...]:
        """Return excluded arches, with exact always-available symbols first."""
        if symbol in self.builtin_symbols:
            return ()
        return longest_prefix_value(symbol, self.target_arch_exclusions) or ()

    def supported_target_oses_for_symbol(self, symbol: str) -> tuple[str, ...]:
        """Return required OSes, or no OS restriction, with exact builtins first."""
        if symbol in self.builtin_symbols:
            return ()
        return longest_prefix_value(symbol, self.target_os_requirements) or ()

    def symbol_available_on_target(
        self, symbol: str, *, target_arch: str | None, target_os: str | None
    ) -> bool:
        supported_oses = self.supported_target_oses_for_symbol(symbol)
        return target_arch not in self.target_arch_exclusions_for_symbol(symbol) and (
            not supported_oses or target_os in supported_oses
        )


def _string_list(value: object, *, field: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(
        isinstance(item, str) and item for item in value
    ):
        raise TypeError(f"{field} must be a list of non-empty strings")
    return tuple(dict.fromkeys(value))


def _append_prefix_rule[T](
    rules: list[tuple[str, T]],
    owners: dict[str, T],
    *,
    prefix: str,
    value: T,
    field: str,
) -> None:
    """Append one prefix fact while rejecting ambiguous sibling ownership."""
    previous = owners.setdefault(prefix, value)
    if previous != value:
        raise TypeError(
            f"{field} conflicts with another category for prefix {prefix!r}: "
            f"{previous!r} != {value!r}"
        )
    if (prefix, value) not in rules:
        rules.append((prefix, value))


def load_intrinsic_availability(
    categories_path: Path,
) -> IntrinsicAvailability:
    """Load canonical exact and prefix availability facts from *categories_path*."""
    data = tomllib.loads(categories_path.read_bytes().decode())
    builtin_symbols: list[str] = []
    feature_gates: list[tuple[str, str]] = []
    target_exclusions: list[tuple[str, tuple[str, ...]]] = []
    target_os_requirements: list[tuple[str, tuple[str, ...]]] = []
    feature_target_exclusions: dict[str, set[str]] = {}
    feature_prefix_owners: dict[str, str] = {}
    target_prefix_owners: dict[str, tuple[str, ...]] = {}
    target_os_prefix_owners: dict[str, tuple[str, ...]] = {}

    builtin = data.get("builtin", {})
    if not isinstance(builtin, dict):
        raise TypeError("builtin must be a table")
    for category, raw_symbols in builtin.items():
        builtin_symbols.extend(_string_list(raw_symbols, field=f"builtin.{category}"))

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
            for prefix in feature_prefixes:
                _append_prefix_rule(
                    feature_gates,
                    feature_prefix_owners,
                    prefix=f"molt_{prefix}",
                    value=feature,
                    field=f"stdlib.{mod_name}.feature_prefixes",
                )

        raw_arches = mod_data.get("unsupported_target_arches", [])
        raw_oses = mod_data.get("supported_target_oses")
        supported_oses: tuple[str, ...] = ()
        target_prefixes: tuple[str, ...] = ()
        if raw_oses is not None:
            field = f"stdlib.{mod_name}.supported_target_oses"
            supported_oses = _string_list(raw_oses, field=field)
            if not supported_oses or not set(supported_oses) <= set(
                SUPPORTED_TARGET_PLATFORMS
            ):
                raise TypeError(
                    f"{field} must be a non-empty subset of "
                    f"{SUPPORTED_TARGET_PLATFORMS!r}"
                )
        if raw_arches or supported_oses:
            target_prefixes = _string_list(
                mod_data.get("target_prefixes", list(prefixes)),
                field=f"stdlib.{mod_name}.target_prefixes",
            )
            if not target_prefixes:
                raise TypeError(f"stdlib.{mod_name}.target_prefixes must not be empty")
            for prefix in target_prefixes:
                if supported_oses:
                    _append_prefix_rule(
                        target_os_requirements,
                        target_os_prefix_owners,
                        prefix=f"molt_{prefix}",
                        value=supported_oses,
                        field=f"stdlib.{mod_name}.supported_target_oses",
                    )
        if raw_arches:
            arches = _string_list(
                raw_arches, field=f"stdlib.{mod_name}.unsupported_target_arches"
            )
            for prefix in target_prefixes:
                _append_prefix_rule(
                    target_exclusions,
                    target_prefix_owners,
                    prefix=f"molt_{prefix}",
                    value=arches,
                    field=f"stdlib.{mod_name}.target_prefixes",
                )
            # A Cargo feature is target-unavailable only when every symbol
            # prefix it gates is covered by this module's target exclusion.
            # This derives profile construction without a second feature list.
            if feature is not None and set(feature_prefixes) <= set(target_prefixes):
                feature_target_exclusions.setdefault(feature, set()).update(arches)

    return IntrinsicAvailability(
        builtin_symbols=tuple(dict.fromkeys(builtin_symbols)),
        feature_gates=tuple(feature_gates),
        target_arch_exclusions=tuple(target_exclusions),
        target_os_requirements=tuple(target_os_requirements),
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
