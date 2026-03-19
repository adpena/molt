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

isfinite = _require_intrinsic("molt_math_isfinite")
isinf = _require_intrinsic("molt_math_isinf")
isnan = _require_intrinsic("molt_math_isnan")
sqrt = _require_intrinsic("molt_math_sqrt")
log2 = _require_intrinsic("molt_math_log2")
log10 = _require_intrinsic("molt_math_log10")
log1p = _require_intrinsic("molt_math_log1p")
exp = _require_intrinsic("molt_math_exp")
expm1 = _require_intrinsic("molt_math_expm1")
sin = _require_intrinsic("molt_math_sin")
cos = _require_intrinsic("molt_math_cos")
acos = _require_intrinsic("molt_math_acos")
tan = _require_intrinsic("molt_math_tan")
asin = _require_intrinsic("molt_math_asin")
atan = _require_intrinsic("molt_math_atan")
atan2 = _require_intrinsic("molt_math_atan2")
sinh = _require_intrinsic("molt_math_sinh")
cosh = _require_intrinsic("molt_math_cosh")
tanh = _require_intrinsic("molt_math_tanh")
asinh = _require_intrinsic("molt_math_asinh")
acosh = _require_intrinsic("molt_math_acosh")
atanh = _require_intrinsic("molt_math_atanh")
lgamma = _require_intrinsic("molt_math_lgamma")
gamma = _require_intrinsic("molt_math_gamma")
erf = _require_intrinsic("molt_math_erf")
erfc = _require_intrinsic("molt_math_erfc")
fabs = _require_intrinsic("molt_math_fabs")
copysign = _require_intrinsic("molt_math_copysign")
floor = _require_intrinsic("molt_math_floor")
ceil = _require_intrinsic("molt_math_ceil")
trunc = _require_intrinsic("molt_math_trunc")
fmod = _require_intrinsic("molt_math_fmod")
modf = _require_intrinsic("molt_math_modf")
frexp = _require_intrinsic("molt_math_frexp")
ldexp = _require_intrinsic("molt_math_ldexp")
fsum = _require_intrinsic("molt_math_fsum")
factorial = _require_intrinsic("molt_math_factorial")
comb = _require_intrinsic("molt_math_comb")
perm = _require_intrinsic("molt_math_perm")
degrees = _require_intrinsic("molt_math_degrees")
radians = _require_intrinsic("molt_math_radians")
dist = _require_intrinsic("molt_math_dist")
isqrt = _require_intrinsic("molt_math_isqrt")
nextafter = _require_intrinsic("molt_math_nextafter")
ulp = _require_intrinsic("molt_math_ulp")
remainder = _require_intrinsic("molt_math_remainder")

# --- Intrinsics used by retained wrappers ---

_MOLT_MATH_LOG = _require_intrinsic("molt_math_log")
_MOLT_MATH_FMA = _require_intrinsic("molt_math_fma")
_MOLT_MATH_ISCLOSE = _require_intrinsic("molt_math_isclose")
_MOLT_MATH_PROD = _require_intrinsic("molt_math_prod")
_MOLT_MATH_GCD = _require_intrinsic("molt_math_gcd")
_MOLT_MATH_LCM = _require_intrinsic("molt_math_lcm")
_MOLT_MATH_HYPOT = _require_intrinsic("molt_math_hypot")

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

globals().pop("_require_intrinsic", None)
