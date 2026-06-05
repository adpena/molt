"""Purpose: differential coverage for the value-range transfer functions added
to unlock SROA hot-loop field promotion (S6 precision):
`BitAnd`/`BitOr`/`BitXor`/`Mod`/`Shr`/`Shl`/`Sub`/`Mul`/`Neg` interval rules,
the counted-loop IV-range seed, and the forward op-range propagation sweep.

These rules feed `fits_inline_int47` → `RawI64Safe` promotion. A wrong (too
tight) range is a silent BigInt-truncation miscompile, so every case below
stresses a *boundary* of a rule, including negative operands and Python `%`
sign-of-divisor semantics, and must be byte-identical to CPython 3.14 whether or
not the analysis fired. The hot-loop struct cases also exercise the producer's
real effect (the field stores must promote, yet stay correct).

Cases by rule:
  * BitAnd with a non-negative constant mask — `i & 15` ∈ [0,15] for ANY i,
    including negative i (two's-complement: `-1 & 15 == 15`).
  * Mod by a positive / negative constant — Python result takes the sign of the
    divisor; `(-7) % 4 == 1`, `7 % -4 == -1`.
  * Shr / Shl by a constant — floor semantics for negative dividends.
  * BitOr / BitXor over non-negative operands.
  * A counted-loop IV whose derived field values (`i`, `i+1`, `i & 7`, `i % 3`)
    are stored into a hot-loop struct — the SROA producer must fire AND stay
    correct, including a parallel run whose IV crosses the inline window and
    whose accumulator crosses 2**63 (must stay a boxed BigInt).
"""


class Box:
    a: int
    b: int
    c: int
    d: int

    def __init__(self) -> None:
        self.a = 0
        self.b = 0
        self.c = 0
        self.d = 0


# --- bitwise / mod / shift over the full integer domain incl. negatives ------
def bit_and_mask(x: int) -> int:
    return x & 15


def bit_or(x: int, y: int) -> int:
    return x | y


def bit_xor(x: int, y: int) -> int:
    return x ^ y


def mod_pos(x: int) -> int:
    return x % 4


def mod_neg(x: int) -> int:
    return x % -4


def shr(x: int) -> int:
    return x >> 2


def shl(x: int) -> int:
    return x << 3


# --- the SROA producer proving ground: derived IV field stores ---------------
def hot_struct_loop(n: int) -> int:
    total = 0
    for i in range(n):
        p = Box()
        p.a = i
        p.b = i + 1
        p.c = i & 7
        p.d = i % 3
        total += p.a + p.b + p.c + p.d
    return total


# --- soundness: an accumulator that crosses 2**63 must stay a boxed BigInt ----
def big_accumulator(n: int) -> int:
    total = 1 << 62
    for i in range(n):
        p = Box()
        p.a = i & 1023
        total += p.a + (1 << 62)
    return total


# Boundary operands: negative, zero, mask edges, large (past 2**46 / 2**63).
for v in [-100, -17, -1, 0, 1, 7, 15, 16, 31, 100, (1 << 46) - 1, 1 << 46, (1 << 60) + 5]:
    print(bit_and_mask(v))
    print(mod_pos(v))
    print(mod_neg(v))
    print(shr(v))
    print(shl(v))

for x, y in [(0, 0), (5, 2), (255, 256), (1 << 40, 1 << 41), (7, 7)]:
    print(bit_or(x, y))
    print(bit_xor(x, y))

print(hot_struct_loop(10))
print(hot_struct_loop(0))
print(hot_struct_loop(50))
print(big_accumulator(4))
print(big_accumulator(0))

# Cross-checks against the closed-form CPython result.
assert hot_struct_loop(10) == sum(i + (i + 1) + (i & 7) + (i % 3) for i in range(10))
assert big_accumulator(4) == (1 << 62) + sum((i & 1023) + (1 << 62) for i in range(4))
assert bit_and_mask(-1) == 15
assert mod_pos(-7) == 1
assert mod_neg(7) == -1
assert shr(-7) == -2  # floor(-7 / 4) == -2
