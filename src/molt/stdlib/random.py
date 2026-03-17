"""Deterministic random helpers for Molt (Mersenne Twister)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from bisect import bisect as _bisect
from collections.abc import Sequence as _Sequence
from itertools import accumulate as _accumulate
import os as _os

_molt_random_new = _require_intrinsic("molt_random_new", globals())
_molt_random_seed = _require_intrinsic("molt_random_seed", globals())
_molt_random_random = _require_intrinsic("molt_random_random", globals())
_molt_random_getrandbits = _require_intrinsic("molt_random_getrandbits", globals())
_molt_random_randbelow = _require_intrinsic("molt_random_randbelow", globals())
_molt_random_getstate = _require_intrinsic("molt_random_getstate", globals())
_molt_random_setstate = _require_intrinsic("molt_random_setstate", globals())
_molt_random_shuffle = _require_intrinsic("molt_random_shuffle", globals())
_molt_random_gauss = _require_intrinsic("molt_random_gauss", globals())
_molt_random_uniform = _require_intrinsic("molt_random_uniform", globals())
_molt_random_triangular = _require_intrinsic("molt_random_triangular", globals())
_molt_random_expovariate = _require_intrinsic("molt_random_expovariate", globals())
_molt_random_normalvariate = _require_intrinsic("molt_random_normalvariate", globals())
_molt_random_lognormvariate = _require_intrinsic(
    "molt_random_lognormvariate", globals()
)
_molt_random_vonmisesvariate = _require_intrinsic(
    "molt_random_vonmisesvariate", globals()
)
_molt_random_paretovariate = _require_intrinsic("molt_random_paretovariate", globals())
_molt_random_weibullvariate = _require_intrinsic(
    "molt_random_weibullvariate", globals()
)
_molt_random_gammavariate = _require_intrinsic("molt_random_gammavariate", globals())
_molt_random_betavariate = _require_intrinsic("molt_random_betavariate", globals())
_molt_random_choices = _require_intrinsic("molt_random_choices", globals())
_molt_random_sample = _require_intrinsic("molt_random_sample", globals())
_molt_random_binomialvariate = _require_intrinsic(
    "molt_random_binomialvariate", globals()
)
_molt_random_randrange = _require_intrinsic("molt_random_randrange", globals())
_molt_random_randbytes = _require_intrinsic("molt_random_randbytes", globals())

_molt_math_log2 = _require_intrinsic("molt_math_log2", globals())
_molt_math_floor = _require_intrinsic("molt_math_floor", globals())
_molt_math_fabs = _require_intrinsic("molt_math_fabs", globals())
_molt_math_sqrt = _require_intrinsic("molt_math_sqrt", globals())
_molt_math_lgamma = _require_intrinsic("molt_math_lgamma", globals())
_molt_math_log = _require_intrinsic("molt_math_log", globals())
_molt_math_isfinite = _require_intrinsic("molt_math_isfinite", globals())

_urandom = _os.urandom

__all__ = [
    "Random",
    "SystemRandom",
    "betavariate",
    "binomialvariate",
    "seed",
    "getstate",
    "setstate",
    "randrange",
    "randint",
    "randbytes",
    "shuffle",
    "random",
    "getrandbits",
    "choice",
    "choices",
    "sample",
    "uniform",
    "triangular",
    "normalvariate",
    "gauss",
    "lognormvariate",
    "expovariate",
    "vonmisesvariate",
    "gammavariate",
    "paretovariate",
    "weibullvariate",
]

BPF = 53
RECIP_BPF = 2**-BPF
_ONE = 1


def _index(value) -> int:
    """Coerce value to int via __index__ protocol."""
    if isinstance(value, int):
        return int(value)
    index = getattr(value, "__index__", None)
    if index is None:
        raise TypeError(
            f"'{type(value).__name__}' object cannot be interpreted as an integer"
        )
    result = index()
    if not isinstance(result, int):
        raise TypeError(f"__index__ returned non-int (type {type(result).__name__})")
    return int(result)


class Random:
    VERSION = 3

    def __init__(self, seed_value: int | None = None) -> None:
        self._handle = _molt_random_new()
        if seed_value is not None:
            _molt_random_seed(self._handle, seed_value, 2)

    def seed(self, a: object | None = None, version: int = 2) -> None:
        _molt_random_seed(self._handle, a, version)

    def random(self) -> float:
        return _molt_random_random(self._handle)

    def uniform(self, a, b):
        return _molt_random_uniform(self._handle, a, b)

    def triangular(self, low=0.0, high=1.0, mode=None):
        return _molt_random_triangular(self._handle, low, high, mode)

    def normalvariate(self, mu=0.0, sigma=1.0):
        return _molt_random_normalvariate(self._handle, mu, sigma)

    def gauss(self, mu=0.0, sigma=1.0):
        return _molt_random_gauss(self._handle, mu, sigma)

    def lognormvariate(self, mu, sigma):
        return _molt_random_lognormvariate(self._handle, mu, sigma)

    def expovariate(self, lambd=1.0):
        return _molt_random_expovariate(self._handle, lambd)

    def vonmisesvariate(self, mu, kappa):
        return _molt_random_vonmisesvariate(self._handle, mu, kappa)

    def gammavariate(self, alpha, beta):
        return _molt_random_gammavariate(self._handle, alpha, beta)

    def betavariate(self, alpha, beta):
        return _molt_random_betavariate(self._handle, alpha, beta)

    def paretovariate(self, alpha):
        return _molt_random_paretovariate(self._handle, alpha)

    def weibullvariate(self, alpha, beta):
        return _molt_random_weibullvariate(self._handle, alpha, beta)

    def getrandbits(self, k: int) -> int:
        return _molt_random_getrandbits(self._handle, k)

    def randbytes(self, n: int) -> bytes:
        return _molt_random_randbytes(self._handle, n)

    def _randbelow(self, n: int) -> int:
        return _molt_random_randbelow(self._handle, n)

    def randrange(self, start, stop=None, step=_ONE) -> int:
        istart = _index(start)
        istop = _index(stop) if stop is not None else None
        istep = _index(step) if step is not _ONE else 1
        return _molt_random_randrange(self._handle, istart, istop, istep)

    def randint(self, a: int, b: int) -> int:
        return self.randrange(a, b + 1)

    def choice(self, seq: _Sequence):
        if not len(seq):
            raise IndexError("Cannot choose from an empty sequence")
        return seq[self._randbelow(len(seq))]

    def choices(self, population, weights=None, *, cum_weights=None, k=1):
        n = len(population)
        if cum_weights is None:
            if weights is None:
                return _molt_random_choices(self._handle, population, None, k)
            try:
                cum_weights = list(_accumulate(weights))
            except TypeError:
                if not isinstance(weights, int):
                    raise
                k = weights
                raise TypeError(
                    f"The number of choices must be a keyword argument: {k=}"
                ) from None
        elif weights is not None:
            raise TypeError("Cannot specify both weights and cumulative weights")
        if len(cum_weights) != n:
            raise ValueError("The number of weights does not match the population")
        total = cum_weights[-1] + 0.0
        if total <= 0.0:
            raise ValueError("Total of weights must be greater than zero")
        if not _molt_math_isfinite(total):
            raise ValueError("Total of weights must be finite")
        return _molt_random_choices(self._handle, population, cum_weights, k)

    def sample(self, population, k, *, counts=None):
        if not hasattr(population, "__len__") or not hasattr(population, "__getitem__"):
            raise TypeError(
                "Population must be a sequence.  For dicts or sets, use sorted(d)."
            )
        n = len(population)
        if counts is not None:
            cum_counts = list(_accumulate(counts))
            if len(cum_counts) != n:
                raise ValueError("The number of counts does not match the population")
            total = cum_counts.pop() if cum_counts else 0
            if not isinstance(total, int):
                raise TypeError("Counts must be integers")
            if total < 0:
                raise ValueError("Counts must be non-negative")
            selections = self.sample(range(total), k=k)
            result = []
            for s in selections:
                result.append(population[_bisect(cum_counts, s)])
            return result
        if not 0 <= k <= n:
            raise ValueError("Sample larger than population or is negative")
        return _molt_random_sample(self._handle, list(population), k)

    def shuffle(self, x: list[object]) -> None:
        _molt_random_shuffle(self._handle, x)

    def getstate(self):
        return _molt_random_getstate(self._handle)

    def setstate(self, state) -> None:
        _molt_random_setstate(self._handle, state)

    def binomialvariate(self, n=1, p=0.5):
        return _molt_random_binomialvariate(self._handle, n, p)

    def __getstate__(self):
        return self.getstate()

    def __setstate__(self, state) -> None:
        self.setstate(state)

    def __reduce__(self):
        return self.__class__, (), self.getstate()


class SystemRandom(Random):
    """Alternate random number generator using OS entropy."""

    def __init__(self, seed_value: int | None = None) -> None:
        self._handle = _molt_random_new()

    def random(self) -> float:
        return (int.from_bytes(_urandom(7), "big") >> 3) * RECIP_BPF

    def getrandbits(self, k: int) -> int:
        if k < 0:
            raise ValueError("number of bits must be non-negative")
        numbytes = (k + 7) // 8
        x = int.from_bytes(_urandom(numbytes), "big")
        return x >> (numbytes * 8 - k)

    def randbytes(self, n: int) -> bytes:
        return _urandom(n)

    def seed(self, *args, **kwds) -> None:
        return None

    def _notimplemented(self, *args, **kwds):
        raise NotImplementedError("System entropy source does not have state.")

    getstate = setstate = _notimplemented


_global = Random()


def seed(value: object | None = None, version: int = 2) -> None:
    _global.seed(value, version)


def getstate():
    return _global.getstate()


def setstate(state) -> None:
    _global.setstate(state)


def randrange(start, stop=None, step=_ONE) -> int:
    return _global.randrange(start, stop, step)


def randint(a: int, b: int) -> int:
    return _global.randint(a, b)


def shuffle(x: list[object]) -> None:
    _global.shuffle(x)


def random() -> float:
    return _global.random()


def getrandbits(k: int) -> int:
    return _global.getrandbits(k)


def randbytes(n: int) -> bytes:
    return _global.randbytes(n)


def choice(seq: _Sequence):
    return _global.choice(seq)


def choices(population, weights=None, *, cum_weights=None, k=1):
    return _global.choices(population, weights=weights, cum_weights=cum_weights, k=k)


def sample(population, k, *, counts=None):
    return _global.sample(population, k, counts=counts)


def uniform(a, b):
    return _global.uniform(a, b)


def triangular(low=0.0, high=1.0, mode=None):
    return _global.triangular(low, high, mode)


def normalvariate(mu=0.0, sigma=1.0):
    return _global.normalvariate(mu, sigma)


def gauss(mu=0.0, sigma=1.0):
    return _global.gauss(mu, sigma)


def lognormvariate(mu, sigma):
    return _global.lognormvariate(mu, sigma)


def expovariate(lambd=1.0):
    return _global.expovariate(lambd)


def vonmisesvariate(mu, kappa):
    return _global.vonmisesvariate(mu, kappa)


def gammavariate(alpha, beta):
    return _global.gammavariate(alpha, beta)


def betavariate(alpha, beta):
    return _global.betavariate(alpha, beta)


def paretovariate(alpha):
    return _global.paretovariate(alpha)


def weibullvariate(alpha, beta):
    return _global.weibullvariate(alpha, beta)


def binomialvariate(n=1, p=0.5):
    return _global.binomialvariate(n, p)
