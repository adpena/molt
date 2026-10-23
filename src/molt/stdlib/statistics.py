"""Intrinsic-backed subset of ``statistics`` for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
from abc import ABCMeta as _ABCMeta
from typing import Any as _Any

import builtins as _builtins
import fractions as _fractions
import math
import numbers as _numbers
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


StatisticsError = _require_intrinsic("molt_builtin_class_lookup")("StatisticsError")


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
        mu_out, sigma_out = _MOLT_STATISTICS_NORMAL_DIST_NEW(mu, sigma)
        self._mu = float(mu_out)
        self._sigma = float(sigma_out)

    @classmethod
    def from_samples(cls, data: _Any):
        return cls(mean(data), stdev(data))

    def samples(self, n: int, *, seed: _Any = None) -> list[float]:
        # CPython switched NormalDist.samples implementation in 3.14.
        if sys.version_info >= (3, 14):
            return list(
                _MOLT_STATISTICS_NORMAL_DIST_SAMPLES(
                    self._mu,
                    self._sigma,
                    n,
                    ("__statistics_inv_cdf_mode__", seed),
                    random.random,
                )
            )
        return list(
            _MOLT_STATISTICS_NORMAL_DIST_SAMPLES(
                self._mu, self._sigma, n, seed, random.gauss
            )
        )

    def pdf(self, x: _Any) -> float:
        return float(_MOLT_STATISTICS_NORMAL_DIST_PDF(self._mu, self._sigma, x))

    def cdf(self, x: _Any) -> float:
        return float(_MOLT_STATISTICS_NORMAL_DIST_CDF(self._mu, self._sigma, x))

    def inv_cdf(self, p: _Any) -> float:
        return float(_MOLT_STATISTICS_NORMAL_DIST_INV_CDF(p, self._mu, self._sigma))

    def quantiles(self, n: int = 4) -> list[float]:
        return [self.inv_cdf(i / n) for i in range(1, n)]

    def overlap(self, other: _Any) -> float:
        if not isinstance(other, NormalDist):
            raise TypeError("Expected another NormalDist instance")
        return float(
            _MOLT_STATISTICS_NORMAL_DIST_OVERLAP(
                self._mu, self._sigma, other._mu, other._sigma
            )
        )

    def zscore(self, x: _Any) -> float:
        return float(_MOLT_STATISTICS_NORMAL_DIST_ZSCORE(self._mu, self._sigma, x))

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
_MOLT_STATISTICS_MEDIAN_LOW = _require_intrinsic("molt_statistics_median_low")
_MOLT_STATISTICS_MEDIAN_HIGH = _require_intrinsic("molt_statistics_median_high")
_MOLT_STATISTICS_MEDIAN_GROUPED = _require_intrinsic("molt_statistics_median_grouped")
_MOLT_STATISTICS_MODE = _require_intrinsic("molt_statistics_mode")
_MOLT_STATISTICS_MULTIMODE = _require_intrinsic("molt_statistics_multimode")
_MOLT_STATISTICS_QUANTILES = _require_intrinsic("molt_statistics_quantiles")
_MOLT_STATISTICS_HARMONIC_MEAN = _require_intrinsic("molt_statistics_harmonic_mean")
_MOLT_STATISTICS_GEOMETRIC_MEAN = _require_intrinsic("molt_statistics_geometric_mean")
_MOLT_STATISTICS_COVARIANCE = _require_intrinsic("molt_statistics_covariance")
_MOLT_STATISTICS_CORRELATION = _require_intrinsic("molt_statistics_correlation")
_MOLT_STATISTICS_LINEAR_REGRESSION = _require_intrinsic(
    "molt_statistics_linear_regression"
)
_MOLT_STATISTICS_NORMAL_DIST_NEW = _require_intrinsic("molt_statistics_normal_dist_new")
_MOLT_STATISTICS_NORMAL_DIST_SAMPLES = _require_intrinsic(
    "molt_statistics_normal_dist_samples"
)
_MOLT_STATISTICS_NORMAL_DIST_PDF = _require_intrinsic("molt_statistics_normal_dist_pdf")
_MOLT_STATISTICS_NORMAL_DIST_CDF = _require_intrinsic("molt_statistics_normal_dist_cdf")
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


# Adapted from CPython v3.14.9 Lib/statistics.py, lines 1453-1649 and 1706-1759.
# https://github.com/python/cpython/blob/v3.14.9/Lib/statistics.py
# Copyright (c) 2001-2026 Python Software Foundation. All rights reserved.
# PSF license: collections/LICENSE.cpython (packaged with this standard library).
# Adaptations: direct implementation imports and guarded native specializations.
# Native kernels specialize builtin sequences; these compiled helpers own
# generic numeric protocols, callback order and Decimal/Fraction result types.
_sqrt_bit_width = 2 * sys.float_info.mant_dig + 3


def _sum(data):
    count = 0
    types = set()
    types_add = types.add
    partials = {}
    partials_get = partials.get
    for typ, values in _groupby_impl(data, type):
        types_add(typ)
        for n, d in map(_exact_ratio, values):
            count += 1
            partials[d] = partials_get(d, 0) + n
    if None in partials:
        total = partials[None]
        assert not _isfinite(total)
    else:
        total = sum((Fraction(n, d) for d, n in partials.items()))
    T = _reduce_impl(_coerce, types, int)
    return (T, total, count)


def _ss(data, c=None):
    if c is not None:
        T, ssd, count = _sum(((d := (x - c)) * d for x in data))
        return (T, ssd, c, count)
    count = 0
    types = set()
    types_add = types.add
    sx_partials = defaultdict(int)
    sxx_partials = defaultdict(int)
    for typ, values in _groupby_impl(data, type):
        types_add(typ)
        for n, d in map(_exact_ratio, values):
            count += 1
            sx_partials[d] += n
            sxx_partials[d] += n * n
    if not count:
        ssd = c = Fraction(0)
    elif None in sx_partials:
        ssd = c = sx_partials[None]
        assert not _isfinite(ssd)
    else:
        sx = sum((Fraction(n, d) for d, n in sx_partials.items()))
        sxx = sum((Fraction(n, d * d) for d, n in sxx_partials.items()))
        ssd = (count * sxx - sx * sx) / count
        c = sx / count
    T = _reduce_impl(_coerce, types, int)
    return (T, ssd, c, count)


def _isfinite(x):
    try:
        return x.is_finite()
    except AttributeError:
        return math.isfinite(x)


def _coerce(T, S):
    assert T is not bool, "initial type T is bool"
    if T is S:
        return T
    if S is int or S is bool:
        return T
    if T is int:
        return S
    if issubclass(S, T):
        return S
    if issubclass(T, S):
        return T
    if issubclass(T, int):
        return S
    if issubclass(S, int):
        return T
    if issubclass(T, Fraction) and issubclass(S, float):
        return S
    if issubclass(T, float) and issubclass(S, Fraction):
        return T
    msg = "don't know how to coerce %s and %s"
    raise TypeError(msg % (T.__name__, S.__name__))


def _exact_ratio(x):
    try:
        return x.as_integer_ratio()
    except AttributeError:
        pass
    except (OverflowError, ValueError):
        assert not _isfinite(x)
        return (x, None)
    try:
        return (x.numerator, x.denominator)
    except AttributeError:
        msg = f"can't convert type '{type(x).__name__}' to numerator/denominator"
        raise TypeError(msg)


def _convert(value, T):
    if type(value) is T:
        return value
    if issubclass(T, int) and value.denominator != 1:
        T = float
    try:
        return T(value)
    except TypeError:
        if issubclass(T, Decimal):
            return T(value.numerator) / T(value.denominator)
        else:
            raise


def _fail_neg(values, errmsg="negative value"):
    for x in values:
        if x < 0:
            raise StatisticsError(errmsg)
        yield x


def _integer_sqrt_of_frac_rto(n: int, m: int) -> int:
    a = math.isqrt(n // m)
    return a | (a * a * m != n)


def _float_sqrt_of_frac(n: int, m: int) -> float:
    q = (n.bit_length() - m.bit_length() - _sqrt_bit_width) // 2
    if q >= 0:
        numerator = _integer_sqrt_of_frac_rto(n, m << 2 * q) << q
        denominator = 1
    else:
        numerator = _integer_sqrt_of_frac_rto(n << -2 * q, m)
        denominator = 1 << -q
    return numerator / denominator


def _decimal_sqrt_of_frac(n: int, m: int) -> Decimal:
    if n <= 0:
        if not n:
            return Decimal("0.0")
        n, m = (-n, -m)
    root = (Decimal(n) / Decimal(m)).sqrt()
    nr, dr = root.as_integer_ratio()
    plus = root.next_plus()
    np, dp = plus.as_integer_ratio()
    if 4 * n * (dr * dp) ** 2 > m * (dr * np + dp * nr) ** 2:
        return plus
    minus = root.next_minus()
    nm, dm = minus.as_integer_ratio()
    if 4 * n * (dr * dm) ** 2 < m * (dr * nm + dm * nr) ** 2:
        return minus
    return root


def mean(data):
    result = _MOLT_STATISTICS_MEAN(data)
    if result is not NotImplemented:
        return result
    T, total, n = _sum(data)
    if n < 1:
        raise StatisticsError("mean requires at least one data point")
    return _convert(total / n, T)


def fmean(data: _Any) -> float:
    return float(_MOLT_STATISTICS_FMEAN(data))


def stdev(data, xbar=None):
    result = _MOLT_STATISTICS_STDEV(data, xbar)
    if result is not NotImplemented:
        return result
    T, ss, c, n = _ss(data, xbar)
    if n < 2:
        raise StatisticsError("stdev requires at least two data points")
    mss = ss / (n - 1)
    if sys.version_info < (3, 13):
        if issubclass(T, Decimal):
            return _decimal_sqrt_of_frac(mss.numerator, mss.denominator)
        return _float_sqrt_of_frac(mss.numerator, mss.denominator)
    try:
        mss_numerator = mss.numerator
        mss_denominator = mss.denominator
    except AttributeError:
        raise ValueError("inf or nan encountered in data")
    if issubclass(T, Decimal):
        return _decimal_sqrt_of_frac(mss_numerator, mss_denominator)
    return _float_sqrt_of_frac(mss_numerator, mss_denominator)


def variance(data, xbar=None):
    result = _MOLT_STATISTICS_VARIANCE(data, xbar)
    if result is not NotImplemented:
        return result
    T, ss, c, n = _ss(data, xbar)
    if n < 2:
        raise StatisticsError("variance requires at least two data points")
    return _convert(ss / (n - 1), T)


def pvariance(data, mu=None):
    result = _MOLT_STATISTICS_PVARIANCE(data, mu)
    if result is not NotImplemented:
        return result
    T, ss, c, n = _ss(data, mu)
    if n < 1:
        raise StatisticsError("pvariance requires at least one data point")
    return _convert(ss / n, T)


def pstdev(data, mu=None):
    result = _MOLT_STATISTICS_PSTDEV(data, mu)
    if result is not NotImplemented:
        return result
    T, ss, c, n = _ss(data, mu)
    if n < 1:
        raise StatisticsError("pstdev requires at least one data point")
    mss = ss / n
    if sys.version_info < (3, 13):
        if issubclass(T, Decimal):
            return _decimal_sqrt_of_frac(mss.numerator, mss.denominator)
        return _float_sqrt_of_frac(mss.numerator, mss.denominator)
    try:
        mss_numerator = mss.numerator
        mss_denominator = mss.denominator
    except AttributeError:
        raise ValueError("inf or nan encountered in data")
    if issubclass(T, Decimal):
        return _decimal_sqrt_of_frac(mss_numerator, mss_denominator)
    return _float_sqrt_of_frac(mss_numerator, mss_denominator)


def median(data: _Any) -> _Any:
    return _MOLT_STATISTICS_MEDIAN(data)


def median_low(data: _Any) -> _Any:
    return _MOLT_STATISTICS_MEDIAN_LOW(data)


def median_high(data: _Any) -> _Any:
    return _MOLT_STATISTICS_MEDIAN_HIGH(data)


def median_grouped(data: _Any, interval: float = 1.0) -> float:
    return float(_MOLT_STATISTICS_MEDIAN_GROUPED(data, interval))


def mode(data: _Any) -> _Any:
    return _MOLT_STATISTICS_MODE(data)


def multimode(data: _Any) -> list[_Any]:
    return list(_MOLT_STATISTICS_MULTIMODE(data))


def quantiles(data: _Any, n: int = 4, *, method: str = "exclusive") -> list[_Any]:
    if n < 1:
        raise StatisticsError("n must be at least 1")
    if type(method) is str and method in {"inclusive", "exclusive"}:
        result = _MOLT_STATISTICS_QUANTILES(data, n, method == "inclusive")
        if result is not NotImplemented:
            return result
    data = sorted(data)
    ld = len(data)
    if ld < 2 and sys.version_info < (3, 13):
        raise StatisticsError("must have at least two data points")
    if ld < 2:
        if ld == 1:
            return data * (n - 1)
        raise StatisticsError("must have at least one data point")
    if method == "inclusive":
        m = ld - 1
        result = []
        for i in range(1, n):
            j, delta = divmod(i * m, n)
            interpolated = (data[j] * (n - delta) + data[j + 1] * delta) / n
            result.append(interpolated)
        return result
    if method == "exclusive":
        m = ld + 1
        result = []
        for i in range(1, n):
            j = i * m // n
            j = 1 if j < 1 else ld - 1 if j > ld - 1 else j
            delta = i * m - j * n
            interpolated = (data[j - 1] * (n - delta) + data[j] * delta) / n
            result.append(interpolated)
        return result
    raise ValueError(f"Unknown method: {method!r}")


def harmonic_mean(data: _Any) -> _Any:
    result = _MOLT_STATISTICS_HARMONIC_MEAN(data)
    if result is not NotImplemented:
        return result
    if iter(data) is data:
        data = list(data)
    errmsg = "harmonic mean does not support negative values"
    n = len(data)
    if n < 1:
        raise StatisticsError("harmonic_mean requires at least one data point")
    if n == 1:
        x = data[0]
        if isinstance(x, (_numbers.Real, Decimal)):
            if x < 0:
                raise StatisticsError(errmsg)
            return x
        raise TypeError("unsupported type")
    try:
        T, total, count = _sum(1 / x for x in _fail_neg(data, errmsg))
    except ZeroDivisionError:
        return 0
    if total <= 0:
        raise StatisticsError("Weighted sum must be positive")
    return _convert(n / total, T)


def geometric_mean(data: _Any) -> float:
    # CPython 3.12 deliberately translates ValueError from the entire operation.
    # From 3.13 onward, callback exceptions retain their original identity.
    if sys.version_info < (3, 13):
        try:
            return float(_MOLT_STATISTICS_GEOMETRIC_MEAN(data))
        except ValueError:
            raise StatisticsError(
                "geometric mean requires a non-empty dataset containing positive numbers"
            ) from None
    return float(_MOLT_STATISTICS_GEOMETRIC_MEAN(data))


def covariance(x: _Any, y: _Any) -> float:
    return float(_MOLT_STATISTICS_COVARIANCE(x, y))


def correlation(x: _Any, y: _Any) -> float:
    return float(_MOLT_STATISTICS_CORRELATION(x, y))


def linear_regression(
    x: _Any, y: _Any, /, *, proportional: bool = False
) -> LinearRegression:
    slope, intercept = _MOLT_STATISTICS_LINEAR_REGRESSION(x, y, proportional)
    return LinearRegression(float(slope), float(intercept))


globals().pop("_require_intrinsic", None)
