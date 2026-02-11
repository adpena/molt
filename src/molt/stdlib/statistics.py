"""Intrinsic-backed subset of ``statistics`` for Molt."""

from __future__ import annotations

import math as _math

from _intrinsics import require_intrinsic as _require_intrinsic

TYPE_CHECKING = False
if TYPE_CHECKING:
    from typing import Any
else:
    Any = object

__all__ = [
    "StatisticsError",
    "correlation",
    "covariance",
    "fmean",
    "geometric_mean",
    "harmonic_mean",
    "linear_regression",
    "mean",
    "median",
    "median_grouped",
    "median_high",
    "median_low",
    "mode",
    "multimode",
    "pstdev",
    "pvariance",
    "quantiles",
    "stdev",
    "variance",
]


class StatisticsError(ValueError):
    pass


_MOLT_STATISTICS_MEAN = _require_intrinsic("molt_statistics_mean", globals())
_MOLT_STATISTICS_STDEV = _require_intrinsic("molt_statistics_stdev", globals())

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): move the remaining statistics helpers below to dedicated Rust intrinsics for full lowering parity.


def _as_list(data: Any, *, opname: str) -> list[Any]:
    try:
        out = list(data)
    except TypeError as exc:
        raise TypeError(f"{opname} requires an iterable input") from exc
    return out


def _as_float_list(data: Any, *, opname: str) -> list[float]:
    out = _as_list(data, opname=opname)
    try:
        return [float(v) for v in out]
    except (TypeError, ValueError) as exc:
        raise TypeError(f"{opname} requires numeric data") from exc


def _require_nonempty(values: list[Any], *, opname: str) -> None:
    if not values:
        raise StatisticsError(f"{opname} requires at least one data point")


def _sorted_float_values(data: Any, *, opname: str) -> list[float]:
    values = _as_float_list(data, opname=opname)
    _require_nonempty(values, opname=opname)
    values.sort()
    return values


def _sorted_values(data: Any, *, opname: str) -> list[Any]:
    values = _as_list(data, opname=opname)
    _require_nonempty(values, opname=opname)
    values.sort()
    return values


def mean(data: Any) -> float:
    try:
        result = _MOLT_STATISTICS_MEAN(data)
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None
    if isinstance(result, float):
        truncated = int(result)
        if result == truncated:
            return truncated
    return result


def fmean(data: Any) -> float:
    values = _as_float_list(data, opname="fmean")
    _require_nonempty(values, opname="fmean")
    return sum(values) / len(values)


def stdev(data: Any, xbar: Any = None) -> float:
    try:
        return float(_MOLT_STATISTICS_STDEV(data, xbar))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def variance(data: Any, xbar: Any = None) -> float:
    values = _as_float_list(data, opname="variance")
    if len(values) < 2:
        raise StatisticsError("variance requires at least two data points")
    mean_value = float(xbar) if xbar is not None else (sum(values) / len(values))
    sum_sq = sum((value - mean_value) ** 2 for value in values)
    return sum_sq / (len(values) - 1)


def pvariance(data: Any, mu: Any = None) -> float:
    values = _as_float_list(data, opname="pvariance")
    _require_nonempty(values, opname="pvariance")
    mean_value = float(mu) if mu is not None else (sum(values) / len(values))
    sum_sq = sum((value - mean_value) ** 2 for value in values)
    return sum_sq / len(values)


def pstdev(data: Any, mu: Any = None) -> float:
    return _math.sqrt(pvariance(data, mu))


def median(data: Any) -> float:
    values = _sorted_float_values(data, opname="median")
    n = len(values)
    mid = n // 2
    if n % 2:
        return values[mid]
    return (values[mid - 1] + values[mid]) / 2.0


def median_low(data: Any) -> float:
    values = _sorted_values(data, opname="median_low")
    return values[(len(values) - 1) // 2]


def median_high(data: Any) -> float:
    values = _sorted_values(data, opname="median_high")
    return values[len(values) // 2]


def median_grouped(data: Any, interval: float = 1.0) -> float:
    values = _sorted_float_values(data, opname="median_grouped")
    x = median(values)
    i = float(interval)
    lower = x - (i / 2.0)
    cf = sum(1 for value in values if value < x)
    f = sum(1 for value in values if value == x)
    if f == 0:
        raise StatisticsError("no grouped median for empty class")
    return lower + i * ((len(values) / 2.0 - cf) / f)


def mode(data: Any) -> Any:
    values = _as_list(data, opname="mode")
    _require_nonempty(values, opname="mode")
    counts: dict[Any, int] = {}
    for value in values:
        counts[value] = counts.get(value, 0) + 1
    best_value = values[0]
    best_count = counts[best_value]
    for value in values:
        count = counts[value]
        if count > best_count:
            best_value = value
            best_count = count
    return best_value


def multimode(data: Any) -> list[Any]:
    values = _as_list(data, opname="multimode")
    if not values:
        return []
    counts: dict[Any, int] = {}
    first_index: dict[Any, int] = {}
    for idx, value in enumerate(values):
        counts[value] = counts.get(value, 0) + 1
        if value not in first_index:
            first_index[value] = idx
    max_count = max(counts.values())
    modes = [value for value, count in counts.items() if count == max_count]
    modes.sort(key=lambda value: first_index[value])
    return modes


def quantiles(data: Any, n: int = 4, *, method: str = "exclusive") -> list[float]:
    values = _sorted_float_values(data, opname="quantiles")
    if n < 1:
        raise StatisticsError("n must be at least 1")
    if len(values) < 2:
        raise StatisticsError("must have at least two data points")
    if method not in {"exclusive", "inclusive"}:
        raise StatisticsError("method must be 'exclusive' or 'inclusive'")

    result: list[float] = []
    if method == "exclusive":
        m = len(values) + 1
        for i in range(1, n):
            j, delta = divmod(i * m, n)
            if j <= 0:
                result.append(values[0])
                continue
            if j >= len(values):
                result.append(values[-1])
                continue
            lo = values[j - 1]
            hi = values[j]
            result.append(lo + (delta / n) * (hi - lo))
        return result

    m = len(values) - 1
    for i in range(1, n):
        j, delta = divmod(i * m, n)
        lo = values[j]
        hi = values[j + 1]
        result.append(lo + (delta / n) * (hi - lo))
    return result


def harmonic_mean(data: Any) -> float:
    values = _as_float_list(data, opname="harmonic_mean")
    _require_nonempty(values, opname="harmonic_mean")
    if any(value < 0.0 for value in values):
        raise StatisticsError("harmonic mean does not support negative values")
    if any(value == 0.0 for value in values):
        return 0.0
    return len(values) / sum(1.0 / value for value in values)


def geometric_mean(data: Any) -> float:
    values = _as_float_list(data, opname="geometric_mean")
    _require_nonempty(values, opname="geometric_mean")
    if any(value < 0.0 for value in values):
        raise StatisticsError("geometric mean does not support negative values")
    if any(value == 0.0 for value in values):
        return 0.0
    return _math.exp(sum(_math.log(value) for value in values) / len(values))


def covariance(x: Any, y: Any) -> float:
    x_values = _as_float_list(x, opname="covariance")
    y_values = _as_float_list(y, opname="covariance")
    if len(x_values) != len(y_values):
        raise StatisticsError(
            "covariance requires that both inputs have the same length"
        )
    if len(x_values) < 2:
        raise StatisticsError("covariance requires at least two data points")
    x_mean = sum(x_values) / len(x_values)
    y_mean = sum(y_values) / len(y_values)
    accum = 0.0
    for xv, yv in zip(x_values, y_values):
        accum += (xv - x_mean) * (yv - y_mean)
    return accum / (len(x_values) - 1)


def correlation(x: Any, y: Any) -> float:
    x_values = _as_float_list(x, opname="correlation")
    y_values = _as_float_list(y, opname="correlation")
    if len(x_values) != len(y_values):
        raise StatisticsError(
            "correlation requires that both inputs have the same length"
        )
    if len(x_values) < 2:
        raise StatisticsError("correlation requires at least two data points")
    x_mean = sum(x_values) / len(x_values)
    y_mean = sum(y_values) / len(y_values)
    num = 0.0
    x_var = 0.0
    y_var = 0.0
    for xv, yv in zip(x_values, y_values):
        dx = xv - x_mean
        dy = yv - y_mean
        num += dx * dy
        x_var += dx * dx
        y_var += dy * dy
    denom = _math.sqrt(x_var * y_var)
    if denom == 0.0:
        raise StatisticsError("at least one of the inputs is constant")
    return num / denom


def linear_regression(x: Any, y: Any) -> tuple[float, float]:
    x_values = _as_float_list(x, opname="linear_regression")
    y_values = _as_float_list(y, opname="linear_regression")
    if len(x_values) != len(y_values):
        raise StatisticsError("x and y must have the same number of data points")
    if len(x_values) < 2:
        raise StatisticsError("linear_regression requires at least two data points")
    x_mean = sum(x_values) / len(x_values)
    y_mean = sum(y_values) / len(y_values)
    sxx = 0.0
    sxy = 0.0
    for xv, yv in zip(x_values, y_values):
        dx = xv - x_mean
        sxx += dx * dx
        sxy += dx * (yv - y_mean)
    if sxx == 0.0:
        raise StatisticsError("x is constant")
    slope = sxy / sxx
    intercept = y_mean - slope * x_mean
    return (slope, intercept)
