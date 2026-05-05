"""Shared benchmark JSON evidence policy."""

from __future__ import annotations

import math
from typing import Any


METRIC_OK_GATES: dict[str, tuple[str, ...]] = {
    "codon_time_s": ("codon_ok",),
    "molt_codon_ratio": ("molt_ok", "codon_ok"),
    "molt_cpython_ratio": ("molt_ok",),
    "molt_nuitka_ratio": ("molt_ok", "nuitka_ok"),
    "molt_pypy_ratio": ("molt_ok", "pypy_ok"),
    "molt_pyodide_ratio": ("molt_ok", "pyodide_ok"),
    "molt_speedup": ("molt_ok",),
    "molt_time_s": ("molt_ok",),
    "molt_wasm_control_time_s": ("molt_wasm_control_ok",),
    "molt_wasm_time_s": ("molt_wasm_ok",),
    "nuitka_time_s": ("nuitka_ok",),
    "pypy_time_s": ("pypy_ok",),
    "pyodide_time_s": ("pyodide_ok",),
}


def valid_positive_number(value: Any) -> float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    normalized = float(value)
    if not math.isfinite(normalized) or normalized <= 0:
        return None
    return normalized


def metric_is_comparable(entry: dict[str, Any], metric: str) -> bool:
    return all(entry.get(gate) is True for gate in METRIC_OK_GATES.get(metric, ()))


def comparator_time(entry: dict[str, Any], lane: str) -> float | None:
    if not entry.get(f"{lane}_ok"):
        return None
    return valid_positive_number(entry.get(f"{lane}_time_s"))


def native_molt_time(entry: dict[str, Any]) -> float | None:
    if not entry.get("molt_ok"):
        return None
    return valid_positive_number(entry.get("molt_time_s"))


def native_molt_speedup(entry: dict[str, Any]) -> float | None:
    if not entry.get("molt_ok"):
        return None
    return valid_positive_number(entry.get("molt_speedup"))


def wasm_molt_time(entry: dict[str, Any]) -> float | None:
    if not entry.get("molt_wasm_ok"):
        return None
    return valid_positive_number(entry.get("molt_wasm_time_s"))


def validated_runtime_samples(entry: dict[str, Any]) -> list[float] | None:
    if not entry.get("molt_ok"):
        return None

    raw_samples = entry.get("molt_samples_s")
    if raw_samples is None:
        stats = entry.get("super_stats", {}).get("molt")
        if stats is None:
            return None
        raw_samples = stats.get("samples_s")

    if raw_samples is None:
        return None
    if not isinstance(raw_samples, list) or not raw_samples:
        raise ValueError("invalid raw sample array for runtime metric")

    samples: list[float] = []
    for sample in raw_samples:
        value = valid_positive_number(sample)
        if value is None:
            raise ValueError(f"invalid raw sample for runtime metric: {sample!r}")
        samples.append(value)
    return samples
