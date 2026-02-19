"""Intrinsic-backed subset of ``statistics`` for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
from abc import ABCMeta as _ABCMeta
from typing import Any as _Any

import builtins as _builtins
import math
import numbers
import random
import sys
from bisect import bisect_left, bisect_right
from collections import Counter, defaultdict, namedtuple
from decimal import Decimal
from functools import reduce
from itertools import count, groupby, repeat
from math import erf, exp, fabs, fsum, hypot, log, sqrt, tau
from operator import itemgetter

try:
    from fractions import Fraction
except Exception:  # noqa: BLE001

    class Fraction(metaclass=_ABCMeta):
        pass


sumprod = getattr(math, "sumprod", None)
if sumprod is None:
    sumprod = getattr(_builtins, "sumprod", None)
if sumprod is None:

    def sumprod(p, q, /):
        return fsum(x * y for x, y in zip(p, q))


_CPYTHON_API_HELPERS = (
    math,
    numbers,
    random,
    sys,
    Fraction,
    Decimal,
    count,
    groupby,
    repeat,
    bisect_left,
    bisect_right,
    hypot,
    sqrt,
    fabs,
    exp,
    erf,
    tau,
    log,
    fsum,
    sumprod,
    reduce,
    itemgetter,
    Counter,
    namedtuple,
    defaultdict,
)

__all__ = [
    "LinearRegression",
    "NormalDist",
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


class NormalDist:
    __slots__ = ("_mu", "_sigma")

    def __init__(self, mu: float = 0.0, sigma: float = 1.0):
        try:
            mu_out, sigma_out = _MOLT_STATISTICS_NORMAL_DIST_NEW(mu, sigma)
        except ValueError as exc:
            raise StatisticsError(str(exc)) from None
        self._mu = float(mu_out)
        self._sigma = float(sigma_out)

    @classmethod
    def from_samples(cls, data: _Any):
        return cls(mean(data), stdev(data))

    def samples(self, n: int, *, seed: _Any = None) -> list[float]:
        gauss = random.gauss if seed is None else random.Random(seed).gauss
        mu = self._mu
        sigma = self._sigma
        return [gauss(mu, sigma) for _ in repeat(None, n)]

    def pdf(self, x: _Any) -> float:
        try:
            return float(_MOLT_STATISTICS_NORMAL_DIST_PDF(self._mu, self._sigma, x))
        except ValueError as exc:
            raise StatisticsError(str(exc)) from None

    def cdf(self, x: _Any) -> float:
        try:
            return float(_MOLT_STATISTICS_NORMAL_DIST_CDF(self._mu, self._sigma, x))
        except ValueError as exc:
            raise StatisticsError(str(exc)) from None

    def inv_cdf(self, p: _Any) -> float:
        try:
            return float(_MOLT_STATISTICS_NORMAL_DIST_INV_CDF(p, self._mu, self._sigma))
        except ValueError as exc:
            raise StatisticsError(str(exc)) from None

    def quantiles(self, n: int = 4) -> list[float]:
        return [self.inv_cdf(i / n) for i in range(1, n)]

    def overlap(self, other: _Any) -> float:
        if not isinstance(other, NormalDist):
            raise TypeError("Expected another NormalDist instance")
        try:
            return float(
                _MOLT_STATISTICS_NORMAL_DIST_OVERLAP(
                    self._mu, self._sigma, other._mu, other._sigma
                )
            )
        except ValueError as exc:
            raise StatisticsError(str(exc)) from None

    def zscore(self, x: _Any) -> float:
        try:
            return float(_MOLT_STATISTICS_NORMAL_DIST_ZSCORE(self._mu, self._sigma, x))
        except ValueError as exc:
            raise StatisticsError(str(exc)) from None

    @property
    def mean(self) -> float:
        return self._mu

    @property
    def median(self) -> float:
        return self._mu

    @property
    def mode(self) -> float:
        return self._mu

    @property
    def stdev(self) -> float:
        return self._sigma

    @property
    def variance(self) -> float:
        return self._sigma * self._sigma

    def __add__(x1, x2):
        if isinstance(x2, NormalDist):
            return NormalDist(x1._mu + x2._mu, hypot(x1._sigma, x2._sigma))
        return NormalDist(x1._mu + x2, x1._sigma)

    def __sub__(x1, x2):
        if isinstance(x2, NormalDist):
            return NormalDist(x1._mu - x2._mu, hypot(x1._sigma, x2._sigma))
        return NormalDist(x1._mu - x2, x1._sigma)

    def __mul__(x1, x2):
        return NormalDist(x1._mu * x2, x1._sigma * fabs(x2))

    def __truediv__(x1, x2):
        return NormalDist(x1._mu / x2, x1._sigma / fabs(x2))

    def __pos__(x1):
        return NormalDist(x1._mu, x1._sigma)

    def __neg__(x1):
        return NormalDist(0.0 - x1._mu, x1._sigma)

    __radd__ = __add__

    def __rsub__(x1, x2):
        if isinstance(x2, NormalDist):
            return NormalDist(x2._mu - x1._mu, hypot(x2._sigma, x1._sigma))
        return NormalDist(x2 - x1._mu, x1._sigma)

    __rmul__ = __mul__

    def __eq__(x1, x2):
        if not isinstance(x2, NormalDist):
            return NotImplemented
        return x1._mu == x2._mu and x1._sigma == x2._sigma

    def __hash__(self) -> int:
        return hash((self._mu, self._sigma))

    def __repr__(self) -> str:
        return f"{type(self).__name__}(mu={self._mu!r}, sigma={self._sigma!r})"

    def __getstate__(self) -> tuple[float, float]:
        return self._mu, self._sigma

    def __setstate__(self, state: tuple[float, float]) -> None:
        self._mu, self._sigma = state


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
_MOLT_STATISTICS_NORMAL_DIST_NEW = _require_intrinsic(
    "molt_statistics_normal_dist_new", globals()
)
_MOLT_STATISTICS_NORMAL_DIST_PDF = _require_intrinsic(
    "molt_statistics_normal_dist_pdf", globals()
)
_MOLT_STATISTICS_NORMAL_DIST_CDF = _require_intrinsic(
    "molt_statistics_normal_dist_cdf", globals()
)
_MOLT_STATISTICS_NORMAL_DIST_INV_CDF = _require_intrinsic(
    "molt_statistics_normal_dist_inv_cdf", globals()
)
_MOLT_STATISTICS_NORMAL_DIST_ZSCORE = _require_intrinsic(
    "molt_statistics_normal_dist_zscore", globals()
)
_MOLT_STATISTICS_NORMAL_DIST_OVERLAP = _require_intrinsic(
    "molt_statistics_normal_dist_overlap", globals()
)

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete Python 3.12+ statistics API/PEP parity beyond function surface lowering (for example NormalDist and remaining edge-case text parity).


def mean(data: _Any) -> float:
    try:
        result = _MOLT_STATISTICS_MEAN(data)
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None
    if isinstance(result, float):
        truncated = int(result)
        if result == truncated:
            return truncated
    return result


def fmean(data: _Any) -> float:
    try:
        return float(_MOLT_STATISTICS_FMEAN(data))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def stdev(data: _Any, xbar: _Any = None) -> float:
    try:
        return float(_MOLT_STATISTICS_STDEV(data, xbar))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def variance(data: _Any, xbar: _Any = None) -> float:
    try:
        return float(_MOLT_STATISTICS_VARIANCE(data, xbar))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def pvariance(data: _Any, mu: _Any = None) -> float:
    try:
        return float(_MOLT_STATISTICS_PVARIANCE(data, mu))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def pstdev(data: _Any, mu: _Any = None) -> float:
    try:
        return float(_MOLT_STATISTICS_PSTDEV(data, mu))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def median(data: _Any) -> float:
    try:
        return float(_MOLT_STATISTICS_MEDIAN(data))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def median_low(data: _Any) -> _Any:
    try:
        return _MOLT_STATISTICS_MEDIAN_LOW(data)
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def median_high(data: _Any) -> _Any:
    try:
        return _MOLT_STATISTICS_MEDIAN_HIGH(data)
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def median_grouped(data: _Any, interval: float = 1.0) -> float:
    try:
        return float(_MOLT_STATISTICS_MEDIAN_GROUPED(data, interval))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def mode(data: _Any) -> _Any:
    try:
        return _MOLT_STATISTICS_MODE(data)
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def multimode(data: _Any) -> list[_Any]:
    return list(_MOLT_STATISTICS_MULTIMODE(data))


def quantiles(data: _Any, n: int = 4, *, method: str = "exclusive") -> list[float]:
    if n < 1:
        raise StatisticsError("n must be at least 1")
    if method not in {"exclusive", "inclusive"}:
        raise StatisticsError("method must be 'exclusive' or 'inclusive'")
    try:
        return list(_MOLT_STATISTICS_QUANTILES(data, n, method == "inclusive"))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def harmonic_mean(data: _Any) -> float:
    try:
        return float(_MOLT_STATISTICS_HARMONIC_MEAN(data))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def geometric_mean(data: _Any) -> float:
    try:
        return float(_MOLT_STATISTICS_GEOMETRIC_MEAN(data))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def covariance(x: _Any, y: _Any) -> float:
    try:
        return float(_MOLT_STATISTICS_COVARIANCE(x, y))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def correlation(x: _Any, y: _Any) -> float:
    try:
        return float(_MOLT_STATISTICS_CORRELATION(x, y))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None


def linear_regression(
    x: _Any, y: _Any, /, *, proportional: bool = False
) -> LinearRegression:
    try:
        slope, intercept = _MOLT_STATISTICS_LINEAR_REGRESSION(x, y, proportional)
        return LinearRegression(float(slope), float(intercept))
    except ValueError as exc:
        raise StatisticsError(str(exc)) from None
