"""Minimal math shim for Molt."""

from __future__ import annotations

import sys

from _intrinsics import require_intrinsic as _require_intrinsic


_MOLT_MATH_LOG = _require_intrinsic("molt_math_log", globals())
_MOLT_MATH_LOG2 = _require_intrinsic("molt_math_log2", globals())
_MOLT_MATH_LOG10 = _require_intrinsic("molt_math_log10", globals())
_MOLT_MATH_LOG1P = _require_intrinsic("molt_math_log1p", globals())
_MOLT_MATH_EXP = _require_intrinsic("molt_math_exp", globals())
_MOLT_MATH_EXPM1 = _require_intrinsic("molt_math_expm1", globals())
_MOLT_MATH_FMA = _require_intrinsic("molt_math_fma", globals())
_MOLT_MATH_SIN = _require_intrinsic("molt_math_sin", globals())
_MOLT_MATH_COS = _require_intrinsic("molt_math_cos", globals())
_MOLT_MATH_ACOS = _require_intrinsic("molt_math_acos", globals())
_MOLT_MATH_LGAMMA = _require_intrinsic("molt_math_lgamma", globals())
_MOLT_MATH_GAMMA = _require_intrinsic("molt_math_gamma", globals())
_MOLT_MATH_ERF = _require_intrinsic("molt_math_erf", globals())
_MOLT_MATH_ERFC = _require_intrinsic("molt_math_erfc", globals())
_MOLT_MATH_ISFINITE = _require_intrinsic("molt_math_isfinite", globals())
_MOLT_MATH_ISINF = _require_intrinsic("molt_math_isinf", globals())
_MOLT_MATH_ISNAN = _require_intrinsic("molt_math_isnan", globals())
_MOLT_MATH_FABS = _require_intrinsic("molt_math_fabs", globals())
_MOLT_MATH_COPYSIGN = _require_intrinsic("molt_math_copysign", globals())
_MOLT_MATH_SQRT = _require_intrinsic("molt_math_sqrt", globals())
_MOLT_MATH_FLOOR = _require_intrinsic("molt_math_floor", globals())
_MOLT_MATH_CEIL = _require_intrinsic("molt_math_ceil", globals())
_MOLT_MATH_TRUNC = _require_intrinsic("molt_math_trunc", globals())
_MOLT_MATH_FMOD = _require_intrinsic("molt_math_fmod", globals())
_MOLT_MATH_MODF = _require_intrinsic("molt_math_modf", globals())
_MOLT_MATH_FREXP = _require_intrinsic("molt_math_frexp", globals())
_MOLT_MATH_LDEXP = _require_intrinsic("molt_math_ldexp", globals())
_MOLT_MATH_ISCLOSE = _require_intrinsic("molt_math_isclose", globals())
_MOLT_MATH_PROD = _require_intrinsic("molt_math_prod", globals())
_MOLT_MATH_FSUM = _require_intrinsic("molt_math_fsum", globals())
_MOLT_MATH_GCD = _require_intrinsic("molt_math_gcd", globals())
_MOLT_MATH_LCM = _require_intrinsic("molt_math_lcm", globals())
_MOLT_MATH_FACTORIAL = _require_intrinsic("molt_math_factorial", globals())
_MOLT_MATH_COMB = _require_intrinsic("molt_math_comb", globals())
_MOLT_MATH_PERM = _require_intrinsic("molt_math_perm", globals())
_MOLT_MATH_DEGREES = _require_intrinsic("molt_math_degrees", globals())
_MOLT_MATH_RADIANS = _require_intrinsic("molt_math_radians", globals())
_MOLT_MATH_HYPOT = _require_intrinsic("molt_math_hypot", globals())
_MOLT_MATH_TAN = _require_intrinsic("molt_math_tan", globals())
_MOLT_MATH_ASIN = _require_intrinsic("molt_math_asin", globals())
_MOLT_MATH_ATAN = _require_intrinsic("molt_math_atan", globals())
_MOLT_MATH_ATAN2 = _require_intrinsic("molt_math_atan2", globals())
_MOLT_MATH_SINH = _require_intrinsic("molt_math_sinh", globals())
_MOLT_MATH_COSH = _require_intrinsic("molt_math_cosh", globals())
_MOLT_MATH_TANH = _require_intrinsic("molt_math_tanh", globals())
_MOLT_MATH_ASINH = _require_intrinsic("molt_math_asinh", globals())
_MOLT_MATH_ACOSH = _require_intrinsic("molt_math_acosh", globals())
_MOLT_MATH_ATANH = _require_intrinsic("molt_math_atanh", globals())
_MOLT_MATH_DIST = _require_intrinsic("molt_math_dist", globals())
_MOLT_MATH_ISQRT = _require_intrinsic("molt_math_isqrt", globals())
_MOLT_MATH_NEXTAFTER = _require_intrinsic("molt_math_nextafter", globals())
_MOLT_MATH_ULP = _require_intrinsic("molt_math_ulp", globals())
_MOLT_MATH_REMAINDER = _require_intrinsic("molt_math_remainder", globals())

_MOLT_MATH_MISSING = object()
_MOLT_HAS_FMA = sys.platform != "darwin"

__all__ = [
    "ceil",
    "comb",
    "copysign",
    "e",
    "fabs",
    "factorial",
    "floor",
    "fmod",
    "frexp",
    "fsum",
    "gcd",
    "hypot",
    "inf",
    "isfinite",
    "isinf",
    "isclose",
    "isnan",
    "lcm",
    "ldexp",
    "lgamma",
    "gamma",
    "log",
    "log2",
    "log10",
    "log1p",
    "modf",
    "nan",
    "perm",
    "pi",
    "prod",
    "radians",
    "degrees",
    "sqrt",
    "sin",
    "cos",
    "acos",
    "tan",
    "asin",
    "atan",
    "atan2",
    "exp",
    "expm1",
    "sinh",
    "cosh",
    "tanh",
    "asinh",
    "acosh",
    "atanh",
    "erf",
    "erfc",
    "dist",
    "isqrt",
    "nextafter",
    "remainder",
    "ulp",
    "tau",
    "trunc",
]

if _MOLT_HAS_FMA:
    __all__.append("fma")

pi = 3.141592653589793
e = 2.718281828459045
tau = 2.0 * pi
inf = float("inf")
nan = float("nan")


# TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): finish math determinism policy.


def isfinite(x: object) -> bool:
    return _MOLT_MATH_ISFINITE(x)


def isinf(x: object) -> bool:
    return _MOLT_MATH_ISINF(x)


def isnan(x: object) -> bool:
    return _MOLT_MATH_ISNAN(x)


def sqrt(x: object) -> float:
    return _MOLT_MATH_SQRT(x)


def log(x: object, base: object = _MOLT_MATH_MISSING) -> float:
    if base is _MOLT_MATH_MISSING:
        return _MOLT_MATH_LOG(x)
    return _MOLT_MATH_LOG(x) / _MOLT_MATH_LOG(base)


def log2(x: object) -> float:
    return _MOLT_MATH_LOG2(x)


def log10(x: object, /) -> float:
    return _MOLT_MATH_LOG10(x)


def log1p(x: object, /) -> float:
    return _MOLT_MATH_LOG1P(x)


def exp(x: object) -> float:
    return _MOLT_MATH_EXP(x)


def expm1(x: object, /) -> float:
    return _MOLT_MATH_EXPM1(x)


if _MOLT_HAS_FMA:

    def fma(x: object, y: object, z: object, /) -> float:
        return _MOLT_MATH_FMA(x, y, z)


def sinh(x: object, /) -> float:
    return _MOLT_MATH_SINH(x)


def cosh(x: object, /) -> float:
    return _MOLT_MATH_COSH(x)


def tanh(x: object, /) -> float:
    return _MOLT_MATH_TANH(x)


def asinh(x: object, /) -> float:
    return _MOLT_MATH_ASINH(x)


def acosh(x: object, /) -> float:
    return _MOLT_MATH_ACOSH(x)


def atanh(x: object, /) -> float:
    return _MOLT_MATH_ATANH(x)


def sin(x: object) -> float:
    return _MOLT_MATH_SIN(x)


def cos(x: object) -> float:
    return _MOLT_MATH_COS(x)


def acos(x: object) -> float:
    return _MOLT_MATH_ACOS(x)


def tan(x: object, /) -> float:
    return _MOLT_MATH_TAN(x)


def asin(x: object, /) -> float:
    return _MOLT_MATH_ASIN(x)


def atan(x: object, /) -> float:
    return _MOLT_MATH_ATAN(x)


def atan2(y: object, x: object, /) -> float:
    return _MOLT_MATH_ATAN2(y, x)


def lgamma(x: object) -> float:
    return _MOLT_MATH_LGAMMA(x)


def gamma(x: object, /) -> float:
    return _MOLT_MATH_GAMMA(x)


def erf(x: object, /) -> float:
    return _MOLT_MATH_ERF(x)


def erfc(x: object, /) -> float:
    return _MOLT_MATH_ERFC(x)


def trunc(x: object) -> int:
    return _MOLT_MATH_TRUNC(x)


def floor(x: object) -> int:
    return _MOLT_MATH_FLOOR(x)


def ceil(x: object) -> int:
    return _MOLT_MATH_CEIL(x)


def fabs(x: object) -> float:
    return _MOLT_MATH_FABS(x)


def copysign(x: object, y: object) -> float:
    return _MOLT_MATH_COPYSIGN(x, y)


def fmod(x: object, y: object) -> float:
    return _MOLT_MATH_FMOD(x, y)


def modf(x: object) -> tuple[float, float]:
    return _MOLT_MATH_MODF(x)


def frexp(x: object) -> tuple[float, int]:
    return _MOLT_MATH_FREXP(x)


def ldexp(x: object, i: object) -> float:
    return _MOLT_MATH_LDEXP(x, i)


def isclose(
    a: object,
    b: object,
    /,
    *,
    rel_tol: float = 1e-09,
    abs_tol: float = 0.0,
) -> bool:
    return _MOLT_MATH_ISCLOSE(a, b, rel_tol, abs_tol)


def prod(iterable, /, *, start: object = 1) -> object:
    return _MOLT_MATH_PROD(iterable, start)


def fsum(iterable, /) -> float:
    return _MOLT_MATH_FSUM(iterable)


def gcd(*integers: object) -> int:
    return _MOLT_MATH_GCD(integers)


def lcm(*integers: object) -> int:
    return _MOLT_MATH_LCM(integers)


def factorial(x: object, /) -> int:
    return _MOLT_MATH_FACTORIAL(x)


def comb(n: object, k: object, /) -> int:
    return _MOLT_MATH_COMB(n, k)


def perm(n: object, k: object | None = None, /) -> int:
    return _MOLT_MATH_PERM(n, k)


def degrees(x: object, /) -> float:
    return _MOLT_MATH_DEGREES(x)


def radians(x: object, /) -> float:
    return _MOLT_MATH_RADIANS(x)


def hypot(*coordinates: object) -> float:
    return _MOLT_MATH_HYPOT(coordinates)


def dist(p: object, q: object, /) -> float:
    return _MOLT_MATH_DIST(p, q)


def isqrt(n: object, /) -> int:
    return _MOLT_MATH_ISQRT(n)


def nextafter(x: object, y: object, /) -> float:
    return _MOLT_MATH_NEXTAFTER(x, y)


def remainder(x: object, y: object, /) -> float:
    return _MOLT_MATH_REMAINDER(x, y)


def ulp(x: object, /) -> float:
    return _MOLT_MATH_ULP(x)
