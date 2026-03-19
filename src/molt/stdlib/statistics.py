"""Intrinsic-backed subset of ``statistics`` for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
from abc import ABCMeta as _ABCMeta
from typing import Any as _Any

import builtins as _builtins
import fractions as _fractions
import math
import numbers
import random
import sys
from bisect import bisect_left as _bisect_left_impl
from bisect import bisect_right as _bisect_right_impl
from collections import Counter, defaultdict, namedtuple
from decimal import Decimal
from functools import reduce as _reduce_impl
from itertools import count as _count_impl
from itertools import groupby as _groupby_impl
from itertools import repeat as _repeat_impl
from math import erf as _erf_impl
from math import exp as _exp_impl
from math import fabs as _fabs_impl
from math import fsum as _fsum_impl
from math import hypot as _hypot_impl
from math import log as _log_impl
from math import sqrt as _sqrt_impl
from math import tau
from operator import itemgetter as _itemgetter_impl

_FRACTION_TYPE = getattr(_fractions, "Fraction", None)
if _FRACTION_TYPE is None:

    class Fraction(metaclass=_ABCMeta):
        pass

else:
    Fraction = _FRACTION_TYPE


class _BuiltinFunctionOrMethod:
    __slots__ = ("_fn",)

    def __init__(self, fn):
        self._fn = fn

    def __call__(self, *args, **kwargs):
        return self._fn(*args, **kwargs)

    def __getattr__(self, name: str):
        return getattr(self._fn, name)


_BuiltinFunctionOrMethod.__name__ = "builtin_function_or_method"


def _make_type_proxy(fn):
    class _TypeProxy:
        def __new__(cls, *args, **kwargs):
            return fn(*args, **kwargs)

    return _TypeProxy


_sumprod_impl = getattr(math, "sumprod", None)
if _sumprod_impl is None:
    _sumprod_impl = getattr(_builtins, "sumprod", None)
if _sumprod_impl is None:

    def sumprod(p, q, /):
        return _fsum_impl(x * y for x, y in zip(p, q))

    _sumprod_impl = sumprod


bisect_left = _BuiltinFunctionOrMethod(_bisect_left_impl)
bisect_right = _BuiltinFunctionOrMethod(_bisect_right_impl)
erf = _BuiltinFunctionOrMethod(_erf_impl)
exp = _BuiltinFunctionOrMethod(_exp_impl)
fabs = _BuiltinFunctionOrMethod(_fabs_impl)
fsum = _BuiltinFunctionOrMethod(_fsum_impl)
hypot = _BuiltinFunctionOrMethod(_hypot_impl)
log = _BuiltinFunctionOrMethod(_log_impl)
reduce = _BuiltinFunctionOrMethod(_reduce_impl)
sqrt = _BuiltinFunctionOrMethod(_sqrt_impl)
sumprod = _BuiltinFunctionOrMethod(_sumprod_impl)

count = _make_type_proxy(_count_impl)
groupby = _make_type_proxy(_groupby_impl)
repeat = _make_type_proxy(_repeat_impl)
itemgetter = _make_type_proxy(_itemgetter_impl)


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


class LinearRegression:
    __slots__ = ("slope", "intercept")
    _fields = ("slope", "intercept")

    def __init__(self, slope: _Any, intercept: _Any):
        self.slope = float(slope)
        self.intercept = float(intercept)

    def __iter__(self):
        yield self.slope
        yield self.intercept

    def __len__(self) -> int:
        return 2

    def __getitem__(self, index: int) -> float:
        if index == 0:
            return self.slope
        if index == 1:
            return self.intercept
        raise IndexError(index)

    def __eq__(self, other: _Any) -> bool:
        if isinstance(other, LinearRegression):
            return (self.slope, self.intercept) == (other.slope, other.intercept)
        if isinstance(other, tuple):
            return (self.slope, self.intercept) == other
        return False

    def __repr__(self) -> str:
        return f"LinearRegression(slope={self.slope!r}, intercept={self.intercept!r})"


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
        # CPython switched NormalDist.samples implementation in 3.14.
        if sys.version_info >= (3, 14):
            try:
                return list(
                    _MOLT_STATISTICS_NORMAL_DIST_SAMPLES(
                        self._mu,
                        self._sigma,
                        n,
                        ("__statistics_inv_cdf_mode__", seed),
                        random.random,
                    )
                )
            except ValueError:
                raise ValueError("inv_cdf undefined for these parameters") from None
        return list(
            _MOLT_STATISTICS_NORMAL_DIST_SAMPLES(
                self._mu, self._sigma, n, seed, random.gauss
            )
        )

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


_MOLT_STATISTICS_MEAN = _require_intrinsic("molt_statistics_mean")
_MOLT_STATISTICS_FMEAN = _require_intrinsic("molt_statistics_fmean")
_MOLT_STATISTICS_STDEV = _require_intrinsic("molt_statistics_stdev")
_MOLT_STATISTICS_VARIANCE = _require_intrinsic("molt_statistics_variance")
_MOLT_STATISTICS_PVARIANCE = _require_intrinsic("molt_statistics_pvariance")
_MOLT_STATISTICS_PSTDEV = _require_intrinsic("molt_statistics_pstdev")
_MOLT_STATISTICS_MEDIAN = _require_intrinsic("molt_statistics_median")
_MOLT_STATISTICS_MEDIAN_LOW = _require_intrinsic(
    "molt_statistics_median_low"
)
_MOLT_STATISTICS_MEDIAN_HIGH = _require_intrinsic(
    "molt_statistics_median_high"
)
_MOLT_STATISTICS_MEDIAN_GROUPED = _require_intrinsic(
    "molt_statistics_median_grouped"
)
_MOLT_STATISTICS_MODE = _require_intrinsic("molt_statistics_mode")
_MOLT_STATISTICS_MULTIMODE = _require_intrinsic("molt_statistics_multimode")
_MOLT_STATISTICS_QUANTILES = _require_intrinsic("molt_statistics_quantiles")
_MOLT_STATISTICS_HARMONIC_MEAN = _require_intrinsic(
    "molt_statistics_harmonic_mean"
)
_MOLT_STATISTICS_GEOMETRIC_MEAN = _require_intrinsic(
    "molt_statistics_geometric_mean"
)
_MOLT_STATISTICS_COVARIANCE = _require_intrinsic(
    "molt_statistics_covariance"
)
_MOLT_STATISTICS_CORRELATION = _require_intrinsic(
    "molt_statistics_correlation"
)
_MOLT_STATISTICS_LINEAR_REGRESSION = _require_intrinsic(
    "molt_statistics_linear_regression"
)
_MOLT_STATISTICS_NORMAL_DIST_NEW = _require_intrinsic(
    "molt_statistics_normal_dist_new"
)
_MOLT_STATISTICS_NORMAL_DIST_SAMPLES = _require_intrinsic(
    "molt_statistics_normal_dist_samples"
)
_MOLT_STATISTICS_NORMAL_DIST_PDF = _require_intrinsic(
    "molt_statistics_normal_dist_pdf"
)
_MOLT_STATISTICS_NORMAL_DIST_CDF = _require_intrinsic(
    "molt_statistics_normal_dist_cdf"
)
_MOLT_STATISTICS_NORMAL_DIST_INV_CDF = _require_intrinsic(
    "molt_statistics_normal_dist_inv_cdf"
)
_MOLT_STATISTICS_NORMAL_DIST_ZSCORE = _require_intrinsic(
    "molt_statistics_normal_dist_zscore"
)
_MOLT_STATISTICS_NORMAL_DIST_OVERLAP = _require_intrinsic(
    "molt_statistics_normal_dist_overlap"
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

globals().pop("_require_intrinsic", None)
