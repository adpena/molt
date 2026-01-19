"""Minimal math shim for Molt."""

from __future__ import annotations

from typing import Any, cast

__all__ = [
    "e",
    "inf",
    "isfinite",
    "isinf",
    "isnan",
    "nan",
    "pi",
]

pi = 3.141592653589793
e = 2.718281828459045
inf = float("inf")
nan = float("nan")


# TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): implement full math module surface beyond basic float predicates/constants.


def _coerce_real(name: str, value: object) -> float:
    try:
        return float(cast(Any, value))
    except Exception as exc:  # pragma: no cover - runtime-dependent type failures
        raise TypeError(
            f"{name}() argument must be a real number, not {type(value).__name__}"
        ) from exc


def isfinite(x: object) -> bool:
    value = _coerce_real("isfinite", x)
    return value == value and value != inf and value != -inf


def isinf(x: object) -> bool:
    value = _coerce_real("isinf", x)
    return value == inf or value == -inf


def isnan(x: object) -> bool:
    value = _coerce_real("isnan", x)
    return value != value
