"""Intrinsic-backed subset of ``statistics`` for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

TYPE_CHECKING = False
if TYPE_CHECKING:
    from typing import Any
else:
    Any = object

__all__ = [
    "LinearRegression",
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


class LinearRegression(tuple):
    __slots__ = ()
    _fields = ("slope", "intercept")

    def __new__(cls, slope: float, intercept: float):
        return tuple.__new__(cls, (float(slope), float(intercept)))

    @property
    def slope(self) -> float:
        return self[0]

    @property
    def intercept(self) -> float:
        return self[1]

    def __repr__(self) -> str:
        return (
            f"{self.__class__.__name__}(slope={self.slope!r}, "
            f"intercept={self.intercept!r})"
        )


_MOLT_STATISTICS_MEAN = _require_intrinsic("molt_statistics_mean", globals())
_MOLT_STATISTICS_FMEAN = _require_intrinsic("molt_statistics_fmean", globals())
_MOLT_STATISTICS_STDEV = _require_intrinsic("molt_statistics_stdev", globals())
_MOLT_STATISTICS_VARIANCE = _require_intrinsic("molt_statistics_variance", globals())
_MOLT_STATISTICS_PVARIANCE = _require_intrinsic("molt_statistics_pvariance", globals())
_MOLT_STATISTICS_PSTDEV = _require_intrinsic("molt_statistics_pstdev", globals())
_MOLT_STATISTICS_MEDIAN = _require_intrinsic("molt_statistics_median", globals())
_MOLT_STATISTICS_MEDIAN_LOW = _require_intrinsic(
    "molt_statistics_median_low", globals()
)
_MOLT_STATISTICS_MEDIAN_HIGH = _require_intrinsic(
    "molt_statistics_median_high", globals()
)
_MOLT_STATISTICS_MEDIAN_GROUPED = _require_intrinsic(
    "molt_statistics_median_grouped", globals()
)
_MOLT_STATISTICS_MODE = _require_intrinsic("molt_statistics_mode", globals())
_MOLT_STATISTICS_MULTIMODE = _require_intrinsic("molt_statistics_multimode", globals())
_MOLT_STATISTICS_QUANTILES = _require_intrinsic("molt_statistics_quantiles", globals())
_MOLT_STATISTICS_HARMONIC_MEAN = _require_intrinsic(
    "molt_statistics_harmonic_mean", globals()
)
_MOLT_STATISTICS_GEOMETRIC_MEAN = _require_intrinsic(
    "molt_statistics_geometric_mean", globals()
)
_MOLT_STATISTICS_COVARIANCE = _require_intrinsic(
    "molt_statistics_covariance", globals()
)
_MOLT_STATISTICS_CORRELATION = _require_intrinsic(
    "molt_statistics_correlation", globals()
)
_MOLT_STATISTICS_LINEAR_REGRESSION = _require_intrinsic(
    "molt_statistics_linear_regression", globals()
)

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete Python 3.12+ statistics API/PEP parity beyond function surface lowering (for example NormalDist and remaining edge-case text parity).


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
    try:
        return float(_MOLT_STATISTICS_FMEAN(data))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def stdev(data: Any, xbar: Any = None) -> float:
    try:
        return float(_MOLT_STATISTICS_STDEV(data, xbar))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def variance(data: Any, xbar: Any = None) -> float:
    try:
        return float(_MOLT_STATISTICS_VARIANCE(data, xbar))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def pvariance(data: Any, mu: Any = None) -> float:
    try:
        return float(_MOLT_STATISTICS_PVARIANCE(data, mu))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def pstdev(data: Any, mu: Any = None) -> float:
    try:
        return float(_MOLT_STATISTICS_PSTDEV(data, mu))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def median(data: Any) -> float:
    try:
        return float(_MOLT_STATISTICS_MEDIAN(data))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def median_low(data: Any) -> Any:
    try:
        return _MOLT_STATISTICS_MEDIAN_LOW(data)
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def median_high(data: Any) -> Any:
    try:
        return _MOLT_STATISTICS_MEDIAN_HIGH(data)
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def median_grouped(data: Any, interval: float = 1.0) -> float:
    try:
        return float(_MOLT_STATISTICS_MEDIAN_GROUPED(data, interval))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def mode(data: Any) -> Any:
    try:
        return _MOLT_STATISTICS_MODE(data)
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def multimode(data: Any) -> list[Any]:
    return list(_MOLT_STATISTICS_MULTIMODE(data))


def quantiles(data: Any, n: int = 4, *, method: str = "exclusive") -> list[float]:
    if n < 1:
        raise StatisticsError("n must be at least 1")
    if method not in {"exclusive", "inclusive"}:
        raise StatisticsError("method must be 'exclusive' or 'inclusive'")
    try:
        return list(_MOLT_STATISTICS_QUANTILES(data, n, method == "inclusive"))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def harmonic_mean(data: Any) -> float:
    try:
        return float(_MOLT_STATISTICS_HARMONIC_MEAN(data))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def geometric_mean(data: Any) -> float:
    try:
        return float(_MOLT_STATISTICS_GEOMETRIC_MEAN(data))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def covariance(x: Any, y: Any) -> float:
    try:
        return float(_MOLT_STATISTICS_COVARIANCE(x, y))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def correlation(x: Any, y: Any) -> float:
    try:
        return float(_MOLT_STATISTICS_CORRELATION(x, y))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def linear_regression(
    x: Any, y: Any, /, *, proportional: bool = False
) -> LinearRegression:
    try:
        slope, intercept = _MOLT_STATISTICS_LINEAR_REGRESSION(x, y, proportional)
        return LinearRegression(float(slope), float(intercept))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None
