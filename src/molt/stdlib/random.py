"""Deterministic random helpers for Molt (partial)."""

from __future__ import annotations


# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:partial): implement
# CPython-compatible Random with full API + test vectors (Mersenne Twister parity).

_MASK_64 = (1 << 64) - 1
_A = 6364136223846793005
_C = 1
_state = 0


def seed(value: int | None = None) -> None:
    global _state
    if value is None:
        _state = 0
        return
    _state = int(value) & _MASK_64


def _next_u64() -> int:
    global _state
    _state = (_state * _A + _C) & _MASK_64
    return _state


def _bit_length(n: int) -> int:
    bits = 0
    while n:
        bits += 1
        n >>= 1
    return bits


def _randbits(k: int) -> int:
    if k <= 0:
        return 0
    out = 0
    shift = 0
    while k > 0:
        chunk = _next_u64()
        take = 64 if k >= 64 else k
        out |= (chunk & ((1 << take) - 1)) << shift
        shift += take
        k -= take
    return out


def _randbelow(n: int) -> int:
    if n <= 0:
        raise ValueError("empty range for randrange()")
    k = _bit_length(n - 1)
    while True:
        r = _randbits(k)
        if r < n:
            return r


def randrange(start: int, stop: int | None = None, step: int = 1) -> int:
    if stop is None:
        stop = int(start)
        start = 0
    start = int(start)
    stop = int(stop)
    step = int(step)
    if step == 0:
        raise ValueError("randrange() step argument must not be zero")
    rng = range(start, stop, step)
    if len(rng) == 0:
        raise ValueError("empty range for randrange()")
    return rng[_randbelow(len(rng))]


def shuffle(x: list[object]) -> None:
    for i in range(len(x) - 1, 0, -1):
        j = randrange(i + 1)
        x[i], x[j] = x[j], x[i]


__all__ = ["randrange", "seed", "shuffle"]
