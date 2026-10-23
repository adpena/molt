"""Canonical intrinsic feature and target-availability classification.

The data authority is each entry's ``# cfg:`` declaration in
``intrinsics/manifest.pyi`` (HF-487): one exact fact per executable symbol, never
a name-prefix rule. Both intrinsic resolver generation and the WASM ABI
generator consume these helpers so feature and target projection cannot drift
between backends.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType

from molt._host_capabilities_generated import (
    SUPPORTED_TARGET_ARCHITECTURES,
    SUPPORTED_TARGET_PLATFORMS,
)
from tools.intrinsic_manifest import (
    ALWAYS_DEFINED,
    IntrinsicEntry,
    RuntimeCfg,
    parse_intrinsic_manifest,
)


@dataclass(frozen=True)
class IntrinsicAvailability:
    """Exact declared ``cfg`` of every canonical intrinsic executable."""

    symbol_cfgs: Mapping[str, RuntimeCfg]

    def cfg_for_symbol(self, symbol: str) -> RuntimeCfg:
        return self.symbol_cfgs.get(symbol, ALWAYS_DEFINED)

    def feature_gate_for_symbol(self, symbol: str) -> str | None:
        """Return the Cargo feature that defines *symbol*, or None."""
        return self.cfg_for_symbol(symbol).feature

    def target_arch_exclusions_for_symbol(self, symbol: str) -> tuple[str, ...]:
        return self.cfg_for_symbol(symbol).unsupported_target_arches

    def supported_target_oses_for_symbol(self, symbol: str) -> tuple[str, ...]:
        return self.cfg_for_symbol(symbol).supported_target_oses

    def unsupported_target_oses_for_symbol(self, symbol: str) -> tuple[str, ...]:
        return self.cfg_for_symbol(symbol).unsupported_target_oses

    def symbol_available_on_target(
        self, symbol: str, *, target_arch: str | None, target_os: str | None
    ) -> bool:
        return self.cfg_for_symbol(symbol).available_on_target(
            target_arch=target_arch, target_os=target_os
        )

    def gated_symbols(self) -> tuple[str, ...]:
        return tuple(sorted(self.symbol_cfgs))

    def features(self) -> frozenset[str]:
        return frozenset(
            cfg.feature for cfg in self.symbol_cfgs.values() if cfg.feature is not None
        )

    def feature_target_arch_exclusions(self) -> tuple[tuple[str, tuple[str, ...]], ...]:
        """Features that no target-arch provides, by every symbol they gate.

        A feature is unavailable on an architecture exactly when every symbol
        it defines excludes that architecture; the fact is derived, never
        declared twice.
        """
        return _feature_exclusions(
            self.symbol_cfgs, lambda cfg: set(cfg.unsupported_target_arches)
        )

    def feature_target_os_exclusions(self) -> tuple[tuple[str, tuple[str, ...]], ...]:
        """Features whose every symbol excludes a supported target OS."""

        def excluded(cfg: RuntimeCfg) -> set[str]:
            if cfg.supported_target_oses:
                return set(SUPPORTED_TARGET_PLATFORMS) - set(cfg.supported_target_oses)
            return set(cfg.unsupported_target_oses)

        return _feature_exclusions(self.symbol_cfgs, excluded)


def _feature_exclusions(
    symbol_cfgs: Mapping[str, RuntimeCfg], excluded_of
) -> tuple[tuple[str, tuple[str, ...]], ...]:
    by_feature: dict[str, set[str] | None] = {}
    for cfg in symbol_cfgs.values():
        if cfg.feature is None:
            continue
        excluded = excluded_of(cfg)
        prior = by_feature.get(cfg.feature)
        by_feature[cfg.feature] = excluded if prior is None else prior & excluded
    return tuple(
        (feature, tuple(sorted(values)))
        for feature, values in sorted(by_feature.items())
        if values
    )


def intrinsic_availability_from_entries(
    entries: Iterable[IntrinsicEntry],
) -> IntrinsicAvailability:
    """Index the declared cfg of each canonical executable symbol."""
    symbol_cfgs: dict[str, RuntimeCfg] = {}
    for entry in entries:
        if entry.symbol != entry.name:
            continue
        cfg = entry.cfg
        if cfg == ALWAYS_DEFINED:
            continue
        unknown_arches = set(cfg.unsupported_target_arches) - set(
            SUPPORTED_TARGET_ARCHITECTURES
        )
        if unknown_arches:
            raise TypeError(
                f"{entry.name}: unknown target_arch {sorted(unknown_arches)}; "
                f"expected a subset of {SUPPORTED_TARGET_ARCHITECTURES!r}"
            )
        unknown_oses = (
            set(cfg.supported_target_oses) | set(cfg.unsupported_target_oses)
        ) - set(SUPPORTED_TARGET_PLATFORMS)
        if unknown_oses:
            raise TypeError(
                f"{entry.name}: unknown target_os {sorted(unknown_oses)}; "
                f"expected a subset of {SUPPORTED_TARGET_PLATFORMS!r}"
            )
        symbol_cfgs[entry.symbol] = cfg
    return IntrinsicAvailability(
        symbol_cfgs=MappingProxyType(dict(sorted(symbol_cfgs.items())))
    )


def load_intrinsic_availability(manifest_path: Path) -> IntrinsicAvailability:
    """Load the exact declared availability from the intrinsic manifest."""
    return intrinsic_availability_from_entries(
        parse_intrinsic_manifest(manifest_path.read_text(encoding="utf-8"))
    )
