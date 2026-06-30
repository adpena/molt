"""Single implementation authority for metric-ratio arithmetic.

Performance, budget, and benchmark tooling all need ratios, but raw division in
each consumer is how Molt ended up with directionless ``time/time`` fields and
unguarded None/0/NaN cases. This module is the implementation authority:
callers choose an explicit :class:`RatioDirection`, degenerate operands return
``None`` instead of a finite value, and compile-budget utilization uses the
separate zero-spend-valid domain.

``tools/perf_authority.py`` re-exports these primitives for historical tooling
imports while keeping perf provenance/freshness policy in tools.
"""

from __future__ import annotations

import enum
import math


class RatioDirection(enum.Enum):
    """Explicit semantic direction for a serialized metric ratio.

    A ratio of two times is meaningless without direction: ``a/b`` and ``b/a``
    can describe the same measurement while one says Molt is faster and the
    other says Molt is slower. Serialized ratio fields must carry one of these
    values so consumers know which side of ``1.0`` is good.
    """

    SPEEDUP = "speedup:baseline_time/candidate_time;>1=candidate_faster"
    MOLT_OVER_BASELINE = "molt_over_baseline:molt_time/baseline_time;<1=molt_faster"
    RATIO = "ratio:numerator/denominator"


def _finite_positive(value: float | None) -> float | None:
    """Return ``value`` as a finite, strictly-positive float, else ``None``."""
    if value is None:
        return None
    try:
        v = float(value)
    except (TypeError, ValueError):
        return None
    if not math.isfinite(v) or v <= 0.0:
        return None
    return v


def safe_speedup(cpython_time: float | None, molt_time: float | None) -> float | None:
    """Speedup = ``cpython_time / molt_time``, or ``None`` when unmeasurable."""
    return signed_ratio_value(
        cpython_time,
        molt_time,
        direction=RatioDirection.SPEEDUP,
    )


def signed_ratio(
    numerator: float | None,
    denominator: float | None,
    *,
    direction: RatioDirection,
) -> dict[str, object]:
    """Return a guarded ratio block with explicit direction metadata.

    ``value`` is ``numerator / denominator`` only when both operands are present,
    numeric, finite, and strictly positive. Otherwise the ratio is
    unmeasurable and ``value`` is ``None``. That fail-closed shape is what keeps
    absent external runtimes, failed builds, and zero timings from becoming
    finite speedups or slowdowns.
    """
    if not isinstance(direction, RatioDirection):
        raise TypeError(
            f"direction must be a RatioDirection, got {type(direction).__name__}"
        )
    numer = _finite_positive(numerator)
    denom = _finite_positive(denominator)
    value = (numer / denom) if (numer is not None and denom is not None) else None
    return {
        "value": value,
        "direction": direction.value,
        "numerator_ok": numer is not None,
        "denominator_ok": denom is not None,
    }


def signed_ratio_value(
    numerator: float | None,
    denominator: float | None,
    *,
    direction: RatioDirection,
) -> float | None:
    """Scalar projection of :func:`signed_ratio`."""
    value = signed_ratio(numerator, denominator, direction=direction)["value"]
    return value if isinstance(value, (int, float)) else None


def relative_time_delta(
    current_time: float | None,
    baseline_time: float | None,
) -> float | None:
    """Return ``current / baseline - 1`` with the same guarded ratio semantics."""
    ratio = signed_ratio_value(
        current_time,
        baseline_time,
        direction=RatioDirection.MOLT_OVER_BASELINE,
    )
    return None if ratio is None else ratio - 1.0


def budget_utilization(
    spent_time: float | None, budget_time: float | None
) -> float | None:
    """Compile/work budget utilization = ``spent / budget``.

    Unlike wall-clock speedup, zero spent time is a valid 0.0 utilization.
    The budget denominator still must be present, finite, and positive.
    """
    denom = _finite_positive(budget_time)
    if denom is None:
        return None
    if spent_time is None:
        return None
    try:
        numer = float(spent_time)
    except (TypeError, ValueError):
        return None
    if not math.isfinite(numer) or numer < 0.0:
        return None
    return numer / denom
