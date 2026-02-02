"""Deterministic random helpers for Molt (Mersenne Twister)."""

from __future__ import annotations

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): expand Random API parity (choices/sample/getstate/setstate) + test vectors.

__all__ = ["Random", "seed", "randrange", "randint", "shuffle"]

_N = 624
_M = 397
_MATRIX_A = 0x9908B0DF
_UPPER_MASK = 0x80000000
_LOWER_MASK = 0x7FFFFFFF


class Random:
    def __init__(self, seed_value: int | None = None) -> None:
        self._mt = [0] * _N
        self._index = _N
        self.seed(seed_value)

    def seed(self, a: int | None = None) -> None:
        if a is None:
            a = 0
        seed_val = int(a) & 0xFFFFFFFF
        self._mt[0] = seed_val
        for i in range(1, _N):
            prev = self._mt[i - 1]
            self._mt[i] = (1812433253 * (prev ^ (prev >> 30)) + i) & 0xFFFFFFFF
        self._index = _N

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
        y ^= (y >> 11) & 0xFFFFFFFF
        y ^= (y << 7) & 0x9D2C5680
        y ^= (y << 15) & 0xEFC60000
        y ^= y >> 18
        return y & 0xFFFFFFFF

    def random(self) -> float:
        a = self._rand_u32() >> 5
        b = self._rand_u32() >> 6
        return (a * 67108864.0 + b) / 9007199254740992.0

    def getrandbits(self, k: int) -> int:
        if k <= 0:
            return 0
        out = 0
        bits = 0
        while bits < k:
            r = self._rand_u32()
            take = min(32, k - bits)
            out |= (r & ((1 << take) - 1)) << bits
            bits += take
        return out

    def _randbelow(self, n: int) -> int:
        if n <= 0:
            raise ValueError("empty range for randrange()")
        k = n.bit_length()
        while True:
            r = self.getrandbits(k)
            if r < n:
                return r

    def randrange(self, start: int, stop: int | None = None, step: int = 1) -> int:
        if stop is None:
            stop = int(start)
            start = 0
        start = int(start)
        stop = int(stop)
        step = int(step)
        if step == 0:
            raise ValueError("randrange() step argument must not be zero")
        width = stop - start
        if step == 1 and width > 0:
            return start + self._randbelow(width)
        if step > 0:
            n = (width + step - 1) // step
        else:
            n = (width + step + 1) // step
        if n <= 0:
            raise ValueError("empty range for randrange()")
        return start + step * self._randbelow(n)

    def randint(self, a: int, b: int) -> int:
        return self.randrange(a, b + 1)

    def shuffle(self, x: list[object]) -> None:
        for i in range(len(x) - 1, 0, -1):
            j = self.randrange(i + 1)
            x[i], x[j] = x[j], x[i]


_global = Random()


def seed(value: int | None = None) -> None:
    _global.seed(value)


def randrange(start: int, stop: int | None = None, step: int = 1) -> int:
    return _global.randrange(start, stop, step)


def randint(a: int, b: int) -> int:
    return _global.randint(a, b)


def shuffle(x: list[object]) -> None:
    _global.shuffle(x)
