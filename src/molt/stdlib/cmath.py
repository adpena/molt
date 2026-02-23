"""Minimal intrinsic-gated `cmath` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# --- intrinsic bindings ---

_MOLT_CMATH_ACOS = _require_intrinsic("molt_cmath_acos", globals())
_MOLT_CMATH_ACOSH = _require_intrinsic("molt_cmath_acosh", globals())
_MOLT_CMATH_ASIN = _require_intrinsic("molt_cmath_asin", globals())
_MOLT_CMATH_ASINH = _require_intrinsic("molt_cmath_asinh", globals())
_MOLT_CMATH_ATAN = _require_intrinsic("molt_cmath_atan", globals())
_MOLT_CMATH_ATANH = _require_intrinsic("molt_cmath_atanh", globals())
_MOLT_CMATH_COS = _require_intrinsic("molt_cmath_cos", globals())
_MOLT_CMATH_COSH = _require_intrinsic("molt_cmath_cosh", globals())
_MOLT_CMATH_SIN = _require_intrinsic("molt_cmath_sin", globals())
_MOLT_CMATH_SINH = _require_intrinsic("molt_cmath_sinh", globals())
_MOLT_CMATH_TAN = _require_intrinsic("molt_cmath_tan", globals())
_MOLT_CMATH_TANH = _require_intrinsic("molt_cmath_tanh", globals())
_MOLT_CMATH_EXP = _require_intrinsic("molt_cmath_exp", globals())
_MOLT_CMATH_LOG = _require_intrinsic("molt_cmath_log", globals())
_MOLT_CMATH_LOG10 = _require_intrinsic("molt_cmath_log10", globals())
_MOLT_CMATH_SQRT = _require_intrinsic("molt_cmath_sqrt", globals())
_MOLT_CMATH_PHASE = _require_intrinsic("molt_cmath_phase", globals())
_MOLT_CMATH_POLAR = _require_intrinsic("molt_cmath_polar", globals())
_MOLT_CMATH_RECT = _require_intrinsic("molt_cmath_rect", globals())
_MOLT_CMATH_ISFINITE = _require_intrinsic("molt_cmath_isfinite", globals())
_MOLT_CMATH_ISINF = _require_intrinsic("molt_cmath_isinf", globals())
_MOLT_CMATH_ISNAN = _require_intrinsic("molt_cmath_isnan", globals())
_MOLT_CMATH_ISCLOSE = _require_intrinsic("molt_cmath_isclose", globals())
_MOLT_CMATH_CONSTANTS = _require_intrinsic("molt_cmath_constants", globals())

# --- constants ---

_consts = _MOLT_CMATH_CONSTANTS()
pi: float = _consts["pi"]
e: float = _consts["e"]
tau: float = _consts["tau"]
inf: float = _consts["inf"]
infj: complex = _consts["infj"]
nan: float = _consts["nan"]
nanj: complex = _consts["nanj"]
del _consts


# --- trigonometric functions ---


def acos(z):
    z = complex(z)
    r, i = _MOLT_CMATH_ACOS(z.real, z.imag)
    return complex(r, i)


def acosh(z):
    z = complex(z)
    r, i = _MOLT_CMATH_ACOSH(z.real, z.imag)
    return complex(r, i)


def asin(z):
    z = complex(z)
    r, i = _MOLT_CMATH_ASIN(z.real, z.imag)
    return complex(r, i)


def asinh(z):
    z = complex(z)
    r, i = _MOLT_CMATH_ASINH(z.real, z.imag)
    return complex(r, i)


def atan(z):
    z = complex(z)
    r, i = _MOLT_CMATH_ATAN(z.real, z.imag)
    return complex(r, i)


def atanh(z):
    z = complex(z)
    r, i = _MOLT_CMATH_ATANH(z.real, z.imag)
    return complex(r, i)


def cos(z):
    z = complex(z)
    r, i = _MOLT_CMATH_COS(z.real, z.imag)
    return complex(r, i)


def cosh(z):
    z = complex(z)
    r, i = _MOLT_CMATH_COSH(z.real, z.imag)
    return complex(r, i)


def sin(z):
    z = complex(z)
    r, i = _MOLT_CMATH_SIN(z.real, z.imag)
    return complex(r, i)


def sinh(z):
    z = complex(z)
    r, i = _MOLT_CMATH_SINH(z.real, z.imag)
    return complex(r, i)


def tan(z):
    z = complex(z)
    r, i = _MOLT_CMATH_TAN(z.real, z.imag)
    return complex(r, i)


def tanh(z):
    z = complex(z)
    r, i = _MOLT_CMATH_TANH(z.real, z.imag)
    return complex(r, i)


# --- exponential and logarithmic functions ---


def exp(z):
    z = complex(z)
    r, i = _MOLT_CMATH_EXP(z.real, z.imag)
    return complex(r, i)


def log(z, base=None):
    z = complex(z)
    r, i = _MOLT_CMATH_LOG(z.real, z.imag)
    result = complex(r, i)
    if base is not None:
        base = complex(base)
        br, bi = _MOLT_CMATH_LOG(base.real, base.imag)
        result = result / complex(br, bi)
    return result


def log10(z):
    z = complex(z)
    r, i = _MOLT_CMATH_LOG10(z.real, z.imag)
    return complex(r, i)


# --- power and root functions ---


def sqrt(z):
    z = complex(z)
    r, i = _MOLT_CMATH_SQRT(z.real, z.imag)
    return complex(r, i)


# --- polar/rectangular conversion ---


def phase(z):
    z = complex(z)
    return float(_MOLT_CMATH_PHASE(z.real, z.imag))


def polar(z):
    z = complex(z)
    result = _MOLT_CMATH_POLAR(z.real, z.imag)
    return (float(result[0]), float(result[1]))


def rect(r, phi):
    result = _MOLT_CMATH_RECT(float(r), float(phi))
    return complex(result[0], result[1])


# --- classification functions ---


def isfinite(z):
    z = complex(z)
    return bool(_MOLT_CMATH_ISFINITE(z.real, z.imag))


def isinf(z):
    z = complex(z)
    return bool(_MOLT_CMATH_ISINF(z.real, z.imag))


def isnan(z):
    z = complex(z)
    return bool(_MOLT_CMATH_ISNAN(z.real, z.imag))


def isclose(a, b, *, rel_tol=1e-09, abs_tol=0.0):
    a = complex(a)
    b = complex(b)
    return bool(_MOLT_CMATH_ISCLOSE(a.real, a.imag, b.real, b.imag))


__all__ = [
    "acos",
    "acosh",
    "asin",
    "asinh",
    "atan",
    "atanh",
    "cos",
    "cosh",
    "sin",
    "sinh",
    "tan",
    "tanh",
    "exp",
    "log",
    "log10",
    "sqrt",
    "phase",
    "polar",
    "rect",
    "isfinite",
    "isinf",
    "isnan",
    "isclose",
    "pi",
    "e",
    "tau",
    "inf",
    "infj",
    "nan",
    "nanj",
]
