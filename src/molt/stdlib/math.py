# Shim churn audit: 42 intrinsic-direct / 52 total exports
"""Minimal math shim for Molt.

Pure-forwarding shims eliminated per MOL-215. Functions that require Python-level
argument adaptation (log with optional base, isclose with keyword defaults,
variadic gcd/lcm/hypot, prod with keyword start) retain thin wrappers.
"""

from __future__ import annotations

import sys

from _intrinsics import require_intrinsic as _require_intrinsic


# --- Direct intrinsic bindings (no Python wrapper overhead) ---

isfinite = _require_intrinsic("molt_math_isfinite", globals())
isinf = _require_intrinsic("molt_math_isinf", globals())
isnan = _require_intrinsic("molt_math_isnan", globals())
sqrt = _require_intrinsic("molt_math_sqrt", globals())
log2 = _require_intrinsic("molt_math_log2", globals())
log10 = _require_intrinsic("molt_math_log10", globals())
log1p = _require_intrinsic("molt_math_log1p", globals())
exp = _require_intrinsic("molt_math_exp", globals())
expm1 = _require_intrinsic("molt_math_expm1", globals())
sin = _require_intrinsic("molt_math_sin", globals())
cos = _require_intrinsic("molt_math_cos", globals())
acos = _require_intrinsic("molt_math_acos", globals())
tan = _require_intrinsic("molt_math_tan", globals())
asin = _require_intrinsic("molt_math_asin", globals())
atan = _require_intrinsic("molt_math_atan", globals())
atan2 = _require_intrinsic("molt_math_atan2", globals())
sinh = _require_intrinsic("molt_math_sinh", globals())
cosh = _require_intrinsic("molt_math_cosh", globals())
tanh = _require_intrinsic("molt_math_tanh", globals())
asinh = _require_intrinsic("molt_math_asinh", globals())
acosh = _require_intrinsic("molt_math_acosh", globals())
atanh = _require_intrinsic("molt_math_atanh", globals())
lgamma = _require_intrinsic("molt_math_lgamma", globals())
gamma = _require_intrinsic("molt_math_gamma", globals())
erf = _require_intrinsic("molt_math_erf", globals())
erfc = _require_intrinsic("molt_math_erfc", globals())
fabs = _require_intrinsic("molt_math_fabs", globals())
copysign = _require_intrinsic("molt_math_copysign", globals())
floor = _require_intrinsic("molt_math_floor", globals())
ceil = _require_intrinsic("molt_math_ceil", globals())
trunc = _require_intrinsic("molt_math_trunc", globals())
fmod = _require_intrinsic("molt_math_fmod", globals())
modf = _require_intrinsic("molt_math_modf", globals())
frexp = _require_intrinsic("molt_math_frexp", globals())
ldexp = _require_intrinsic("molt_math_ldexp", globals())
fsum = _require_intrinsic("molt_math_fsum", globals())
factorial = _require_intrinsic("molt_math_factorial", globals())
comb = _require_intrinsic("molt_math_comb", globals())
perm = _require_intrinsic("molt_math_perm", globals())
degrees = _require_intrinsic("molt_math_degrees", globals())
radians = _require_intrinsic("molt_math_radians", globals())
dist = _require_intrinsic("molt_math_dist", globals())
isqrt = _require_intrinsic("molt_math_isqrt", globals())
nextafter = _require_intrinsic("molt_math_nextafter", globals())
ulp = _require_intrinsic("molt_math_ulp", globals())
remainder = _require_intrinsic("molt_math_remainder", globals())

# --- Intrinsics used by retained wrappers ---

_MOLT_MATH_LOG = _require_intrinsic("molt_math_log", globals())
_MOLT_MATH_FMA = _require_intrinsic("molt_math_fma", globals())
_MOLT_MATH_ISCLOSE = _require_intrinsic("molt_math_isclose", globals())
_MOLT_MATH_PROD = _require_intrinsic("molt_math_prod", globals())
_MOLT_MATH_GCD = _require_intrinsic("molt_math_gcd", globals())
_MOLT_MATH_LCM = _require_intrinsic("molt_math_lcm", globals())
_MOLT_MATH_HYPOT = _require_intrinsic("molt_math_hypot", globals())

_MOLT_MATH_MISSING = object()
_MOLT_HAS_FMA = sys.platform != "darwin"

# --- Constants ---

pi = 3.141592653589793
e = 2.718281828459045
tau = 2.0 * pi
inf = float("inf")
nan = float("nan")

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


# NOTE(stdlib-parity, owner:stdlib, milestone:SL1): intrinsic-backed; future work may tighten determinism policy coverage and platform notes.


# --- Retained wrappers (Python-level argument adaptation required) ---


def log(x: object, base: object = _MOLT_MATH_MISSING) -> float:
    if base is _MOLT_MATH_MISSING:
        return _MOLT_MATH_LOG(x)
    return _MOLT_MATH_LOG(x) / _MOLT_MATH_LOG(base)


if _MOLT_HAS_FMA:

    def fma(x: object, y: object, z: object, /) -> float:
        return _MOLT_MATH_FMA(x, y, z)


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


def gcd(*integers: object) -> int:
    return _MOLT_MATH_GCD(integers)


def lcm(*integers: object) -> int:
    return _MOLT_MATH_LCM(integers)


def hypot(*coordinates: object) -> float:
    return _MOLT_MATH_HYPOT(coordinates)
