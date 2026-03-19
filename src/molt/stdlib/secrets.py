"""Intrinsic-backed secrets module for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_molt_secrets_token_bytes = _require_intrinsic("molt_secrets_token_bytes")
_molt_secrets_token_hex = _require_intrinsic("molt_secrets_token_hex")
_molt_secrets_token_urlsafe = _require_intrinsic(
    "molt_secrets_token_urlsafe"
)
_molt_secrets_randbits = _require_intrinsic("molt_secrets_randbits")
_molt_secrets_below = _require_intrinsic("molt_secrets_below")
_molt_secrets_choice = _require_intrinsic("molt_secrets_choice")
_molt_secrets_compare_digest = _require_intrinsic(
    "molt_secrets_compare_digest"
)

DEFAULT_ENTROPY = 32


def token_bytes(nbytes: int | None = None) -> bytes:
    if nbytes is None:
        nbytes = DEFAULT_ENTROPY
    return _molt_secrets_token_bytes(int(nbytes))


def token_hex(nbytes: int | None = None) -> str:
    if nbytes is None:
        nbytes = DEFAULT_ENTROPY
    return str(_molt_secrets_token_hex(int(nbytes)))


def token_urlsafe(nbytes: int | None = None) -> str:
    if nbytes is None:
        nbytes = DEFAULT_ENTROPY
    return str(_molt_secrets_token_urlsafe(int(nbytes)))


def randbelow(exclusive_upper_bound: int) -> int:
    if exclusive_upper_bound <= 0:
        raise ValueError("Upper bound must be positive")
    return int(_molt_secrets_below(int(exclusive_upper_bound)))


def choice(seq):
    if not seq:
        raise IndexError("Cannot choose from an empty sequence")
    return _molt_secrets_choice(seq)


def compare_digest(a, b) -> bool:
    return bool(_molt_secrets_compare_digest(a, b))


class SystemRandom:
    """Alternate random number generator using sources provided
    by the operating system (such as /dev/urandom on Unix or
    CryptGenRandom on Windows)."""

    def getrandbits(self, k: int) -> int:
        if k < 0:
            raise ValueError("number of bits must be non-negative")
        if k == 0:
            return 0
        return int(_molt_secrets_randbits(int(k)))

    def randbelow(self, exclusive_upper_bound: int) -> int:
        return randbelow(exclusive_upper_bound)

    def random(self) -> float:
        return self.getrandbits(53) * (2.0**-53)

    def randrange(self, start: int, stop: int | None = None, step: int = 1) -> int:
        if stop is None:
            if start <= 0:
                raise ValueError("empty range for randrange()")
            return randbelow(start)
        width = stop - start
        if step == 1:
            if width <= 0:
                raise ValueError("empty range for randrange()")
            return start + randbelow(width)
        if step > 0:
            n = (width + step - 1) // step
        elif step < 0:
            n = (width + step + 1) // step
        else:
            raise ValueError("zero step for randrange()")
        if n <= 0:
            raise ValueError("empty range for randrange()")
        return start + step * randbelow(n)

    def randint(self, a: int, b: int) -> int:
        return self.randrange(a, b + 1)

    def choice(self, seq):
        return choice(seq)

globals().pop("_require_intrinsic", None)
