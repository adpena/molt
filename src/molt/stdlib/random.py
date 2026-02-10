"""Deterministic random helpers for Molt (Mersenne Twister)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


from bisect import bisect as _bisect
from collections.abc import Sequence as _Sequence
from itertools import accumulate as _accumulate, repeat as _repeat
import math as _math
import os as _os
from typing import SupportsInt, cast

_require_intrinsic("molt_stdlib_probe", globals())


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

_N = 624
_M = 397
_MATRIX_A = 0x9908B0DF
_UPPER_MASK = 0x80000000
_LOWER_MASK = 0x7FFFFFFF
_MASK_32 = 0xFFFFFFFF
_MASK_64 = (1 << 64) - 1
_ONE = 1
BPF = 53
RECIP_BPF = 2**-BPF
_sha512 = None
_urandom = _os.urandom
_log = _math.log
_exp = _math.exp
_pi = _math.pi
_e = _math.e
_ceil = _math.ceil
_sqrt = _math.sqrt
_acos = _math.acos
_cos = _math.cos
_sin = _math.sin
TWOPI = _math.tau
_floor = _math.floor
_isfinite = _math.isfinite
_lgamma = _math.lgamma
_fabs = _math.fabs
_log2 = _math.log2

NV_MAGICCONST = 4 * _exp(-0.5) / _sqrt(2.0)
LOG4 = _log(4.0)
SG_MAGICCONST = 1.0 + _log(4.5)


def _index(value) -> int:
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


def _next_power_of_four(value: int) -> int:
    size = 1
    while size < value:
        size *= 4
    return size


def _gammavariate_alpha_gt1(random_fn, alpha: float, beta: float) -> float:
    ainv = _sqrt(2.0 * alpha - 1.0)
    bbb = alpha - LOG4
    ccc = alpha + ainv
    while True:
        u1 = random_fn()
        if not 1e-7 < u1 < 0.9999999:
            continue
        u2 = 1.0 - random_fn()
        v = _log(u1 / (1.0 - u1)) / ainv
        x = alpha * _exp(v)
        z = u1 * u1 * u2
        r = bbb + ccc * v - x
        if r + SG_MAGICCONST - 4.5 * z >= 0.0 or r >= _log(z):
            return x * beta


def _gammavariate_alpha_lt1(random_fn, alpha: float, beta: float) -> float:
    while True:
        u = random_fn()
        b = (_e + alpha) / _e
        p = b * u
        if p <= 1.0:
            x = p ** (1.0 / alpha)
        else:
            x = -_log((b - p) / alpha)
        u1 = random_fn()
        if p > 1.0:
            if u1 <= x ** (alpha - 1.0):
                break
        elif u1 == 0.0 or _log(u1) <= 0.0 - x:
            break
    return x * beta


class Random:
    VERSION = 3

    def __init__(self, seed_value: int | None = None) -> None:
        self._mt = [0] * _N
        self._index = _N
        self.gauss_next = None
        self.seed(seed_value)

    def seed(self, a: object | None = None, version: int = 2) -> None:
        if a is None:
            a = 0

        if version == 1 and isinstance(a, (str, bytes)):
            a = a.decode("latin-1") if isinstance(a, bytes) else a
            x = ord(a[0]) << 7 if a else 0
            for c in map(ord, a):
                x = ((1000003 * x) ^ c) & _MASK_64
            x ^= len(a)
            a = -2 if x == -1 else x

        elif version == 2 and isinstance(a, (str, bytes, bytearray)):
            global _sha512
            if _sha512 is None:
                import hashlib as _hashlib

                _sha512 = _hashlib.sha512
            if isinstance(a, str):
                a = a.encode()
            a = int.from_bytes(a + _sha512(a).digest(), "big")

        elif not isinstance(a, (int, float, str, bytes, bytearray)):
            raise TypeError(
                "The only supported seed types are:\n"
                "None, int, float, str, bytes, and bytearray."
            )

        seed_int = self._coerce_seed(a)
        self._init_by_array(_int_to_key(seed_int))
        self._index = _N
        self.gauss_next = None

    def _coerce_seed(self, a: object) -> int:
        if isinstance(a, int):
            return abs(a)
        if isinstance(a, float):
            return hash(a) & _MASK_64
        if isinstance(a, (str, bytes, bytearray)):
            return hash(a) & _MASK_64
        return abs(int(cast(SupportsInt, a)))

    def _init_genrand(self, seed: int) -> None:
        self._mt[0] = seed & _MASK_32
        for i in range(1, _N):
            prev = self._mt[i - 1]
            self._mt[i] = (1812433253 * (prev ^ (prev >> 30)) + i) & _MASK_32

    def _init_by_array(self, init_key: list[int]) -> None:
        self._init_genrand(19650218)
        i = 1
        j = 0
        key_length = len(init_key)
        k = _N if _N > key_length else key_length
        for _ in range(k):
            prev = self._mt[i - 1]
            self._mt[i] = (
                (self._mt[i] ^ ((prev ^ (prev >> 30)) * 1664525)) + init_key[j] + j
            ) & _MASK_32
            i += 1
            j += 1
            if i >= _N:
                self._mt[0] = self._mt[_N - 1]
                i = 1
            if j >= key_length:
                j = 0
        for _ in range(_N - 1):
            prev = self._mt[i - 1]
            self._mt[i] = (
                (self._mt[i] ^ ((prev ^ (prev >> 30)) * 1566083941)) - i
            ) & _MASK_32
            i += 1
            if i >= _N:
                self._mt[0] = self._mt[_N - 1]
                i = 1
        self._mt[0] = _UPPER_MASK

    def _twist(self) -> None:
        for i in range(_N):
            y = (self._mt[i] & _UPPER_MASK) | (self._mt[(i + 1) % _N] & _LOWER_MASK)
            self._mt[i] = self._mt[(i + _M) % _N] ^ (y >> 1)
            if y & 1:
                self._mt[i] ^= _MATRIX_A
        self._index = 0

    def _rand_u32(self) -> int:
        if self._index >= _N:
            self._twist()
        y = self._mt[self._index]
        self._index += 1
        y ^= (y >> 11) & _MASK_32
        y ^= (y << 7) & 0x9D2C5680
        y ^= (y << 15) & 0xEFC60000
        y ^= y >> 18
        return y & _MASK_32

    def random(self) -> float:
        a = self._rand_u32() >> 5
        b = self._rand_u32() >> 6
        return (a * 67108864.0 + b) / 9007199254740992.0

    def uniform(self, a, b):
        """Get a random number in the range [a, b) or [a, b] depending on rounding."""

        return a + (b - a) * self.random()

    def triangular(self, low=0.0, high=1.0, mode=None):
        """Triangular distribution."""
        u = self.random()
        try:
            c = 0.5 if mode is None else (mode - low) / (high - low)
        except ZeroDivisionError:
            return low
        if u > c:
            u = 1.0 - u
            c = 1.0 - c
            low, high = high, low
        return low + (high - low) * _sqrt(u * c)

    def normalvariate(self, mu=0.0, sigma=1.0):
        """Normal distribution."""
        random = self.random
        while True:
            u1 = random()
            u2 = 1.0 - random()
            z = NV_MAGICCONST * (u1 - 0.5) / u2
            zz = z * z / 4.0
            if zz <= -_log(u2):
                break
        return mu + z * sigma

    def gauss(self, mu=0.0, sigma=1.0):
        """Gaussian distribution (faster than normalvariate)."""
        random = self.random
        z = self.gauss_next
        self.gauss_next = None
        if z is None:
            x2pi = random() * TWOPI
            g2rad = _sqrt(-2.0 * _log(1.0 - random()))
            z = _cos(x2pi) * g2rad
            self.gauss_next = _sin(x2pi) * g2rad
        return mu + z * sigma

    def lognormvariate(self, mu, sigma):
        """Log normal distribution."""
        return _exp(self.normalvariate(mu, sigma))

    def expovariate(self, lambd=1.0):
        """Exponential distribution."""
        return -_log(1.0 - self.random()) / lambd

    def vonmisesvariate(self, mu, kappa):
        """Circular data distribution."""
        random = self.random
        if kappa <= 1e-6:
            return TWOPI * random()

        s = 0.5 / kappa
        r = s + _sqrt(1.0 + s * s)

        while True:
            u1 = random()
            z = _cos(_pi * u1)

            d = z / (r + z)
            u2 = random()
            if u2 < 1.0 - d * d or u2 <= (1.0 - d) * _exp(d):
                break

        q = 1.0 / r
        f = (q + z) / (1.0 + q * z)
        u3 = random()
        if u3 > 0.5:
            theta = (mu + _acos(f)) % TWOPI
        else:
            theta = (mu - _acos(f)) % TWOPI

        return theta

    def gammavariate(self, alpha, beta):
        """Gamma distribution (not the gamma function)."""
        if alpha <= 0.0 or beta <= 0.0:
            raise ValueError("gammavariate: alpha and beta must be > 0.0")

        random_fn = self.random
        if alpha > 1.0:
            return _gammavariate_alpha_gt1(random_fn, alpha, beta)
        elif alpha == 1.0:
            return -_log(1.0 - random_fn()) * beta
        return _gammavariate_alpha_lt1(random_fn, alpha, beta)

    def betavariate(self, alpha, beta):
        """Beta distribution."""
        y = self.gammavariate(alpha, 1.0)
        if y:
            return y / (y + self.gammavariate(beta, 1.0))
        return 0.0

    def paretovariate(self, alpha):
        """Pareto distribution."""
        u = 1.0 - self.random()
        return u ** (-1.0 / alpha)

    def weibullvariate(self, alpha, beta):
        """Weibull distribution."""
        u = 1.0 - self.random()
        return alpha * (-_log(u)) ** (1.0 / beta)

    def binomialvariate(self, n=1, p=0.5):
        """Binomial random variable."""
        if n < 0:
            raise ValueError("n must be non-negative")
        if p <= 0.0 or p >= 1.0:
            if p == 0.0:
                return 0
            if p == 1.0:
                return n
            raise ValueError("p must be in the range 0.0 <= p <= 1.0")

        random = self.random

        if n == 1:
            return _index(random() < p)

        if p > 0.5:
            return n - self.binomialvariate(n, 1.0 - p)

        if n * p < 10.0:
            x = y = 0
            c = _log2(1.0 - p)
            if not c:
                return x
            while True:
                y += _floor(_log2(random()) / c) + 1
                if y > n:
                    return x
                x += 1

        setup_complete = False

        spq = _sqrt(n * p * (1.0 - p))
        b = 1.15 + 2.53 * spq
        a = -0.0873 + 0.0248 * b + 0.01 * p
        c = n * p + 0.5
        vr = 0.92 - 4.2 / b

        while True:
            u = random()
            u -= 0.5
            us = 0.5 - _fabs(u)
            k = _floor((2.0 * a / us + b) * u + c)
            if k < 0 or k > n:
                continue

            v = random()
            if us >= 0.07 and v <= vr:
                return k

            if not setup_complete:
                alpha = (2.83 + 5.1 / b) * spq
                lpq = _log(p / (1.0 - p))
                m = _floor((n + 1) * p)
                h = _lgamma(m + 1) + _lgamma(n - m + 1)
                setup_complete = True
            v *= alpha / (a / (us * us) + b)
            if _log(v) <= h - _lgamma(k + 1) - _lgamma(n - k + 1) + (k - m) * lpq:
                return k

    def getrandbits(self, k: int) -> int:
        if k < 0:
            raise ValueError("Cannot convert negative int")
        if k == 0:
            return 0
        words = (k - 1) // 32 + 1
        out = 0
        top_bits = k & 31
        for i in range(words):
            r = self._rand_u32()
            if i == words - 1 and top_bits:
                r >>= 32 - top_bits
            out |= r << (i * 32)
        return out

    def randbytes(self, n: int) -> bytes:
        return self.getrandbits(n * 8).to_bytes(n, "little")

    def _randbelow(self, n: int) -> int:
        if n <= 0:
            raise ValueError("empty range for randrange()")
        k = n.bit_length()
        while True:
            r = self.getrandbits(k)
            if r < n:
                return r

    def randrange(self, start, stop=None, step=_ONE) -> int:
        istart = _index(start)
        if stop is None:
            if step is not _ONE:
                raise TypeError("Missing a non-None stop argument")
            if istart > 0:
                return self._randbelow(istart)
            raise ValueError("empty range for randrange()")

        istop = _index(stop)
        width = istop - istart
        istep = _index(step)
        if istep == 1:
            if width > 0:
                return istart + self._randbelow(width)
            raise ValueError(f"empty range in randrange({start}, {stop})")

        if istep > 0:
            n = (width + istep - 1) // istep
        elif istep < 0:
            n = (width + istep + 1) // istep
        else:
            raise ValueError("zero step for randrange()")
        if n <= 0:
            raise ValueError(f"empty range in randrange({start}, {stop}, {step})")
        return istart + istep * self._randbelow(n)

    def randint(self, a: int, b: int) -> int:
        return self.randrange(a, b + 1)

    def choice(self, seq: _Sequence):
        if not len(seq):
            raise IndexError("Cannot choose from an empty sequence")
        return seq[self._randbelow(len(seq))]

    def choices(self, population, weights=None, *, cum_weights=None, k=1):
        random = self.random
        n = len(population)
        if cum_weights is None:
            if weights is None:
                n += 0.0
                result = []
                for _ in _repeat(None, k):
                    result.append(population[_floor(random() * n)])
                return result
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
        if not _isfinite(total):
            raise ValueError("Total of weights must be finite")
        hi = n - 1
        result = []
        for _ in _repeat(None, k):
            result.append(population[_bisect(cum_weights, random() * total, 0, hi)])
        return result

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

        randbelow = self._randbelow
        if not 0 <= k <= n:
            raise ValueError("Sample larger than population or is negative")
        result = [None] * k
        setsize = 21
        if k > 5:
            setsize += _next_power_of_four(k * 3)
        if n <= setsize:
            pool = [population[i] for i in range(n)]
            for i in range(k):
                j = randbelow(n - i)
                result[i] = pool[j]
                pool[j] = pool[n - i - 1]
        else:
            selected: set[int] = set()
            selected_add = selected.add
            for i in range(k):
                j = randbelow(n)
                while j in selected:
                    j = randbelow(n)
                selected_add(j)
                result[i] = population[j]
        return result

    def shuffle(self, x: list[object]) -> None:
        randbelow = self._randbelow
        for i in reversed(range(1, len(x))):
            j = randbelow(i + 1)
            x[i], x[j] = x[j], x[i]

    def getstate(self):
        return self.VERSION, tuple(self._mt) + (self._index,), self.gauss_next

    def setstate(self, state) -> None:
        version = state[0]
        if version == 3:
            _, internalstate, self.gauss_next = state
        elif version == 2:
            _, internalstate, self.gauss_next = state
            try:
                internalstate = tuple(x % (2**32) for x in internalstate)
            except ValueError as exc:
                raise TypeError from exc
        else:
            raise ValueError(
                f"state with version {version} passed to Random.setstate() "
                f"of version {self.VERSION}"
            )
        if len(internalstate) != _N + 1:
            raise ValueError("state vector has incorrect size")
        self._mt = list(internalstate[:-1])
        self._index = int(internalstate[-1])

    def __getstate__(self):
        return self.getstate()

    def __setstate__(self, state) -> None:
        self.setstate(state)

    def __reduce__(self):
        return self.__class__, (), self.getstate()


class SystemRandom(Random):
    """Alternate random number generator using OS entropy."""

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


def _int_to_key(value: int) -> list[int]:
    value = abs(int(value))
    key: list[int] = []
    while value:
        key.append(value & _MASK_32)
        value >>= 32
    if not key:
        key = [0]
    return key


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
