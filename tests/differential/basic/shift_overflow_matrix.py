"""Purpose: differential coverage for the int shift overflow contract.

The I64 fast lane (LLVM `emit_bitwise`, WASM `emit_lir_i64_binary_or_boxed`)
may only emit a RAW machine `<<`/`>>` when the value-range analysis proves the
result fits the inline window AND the shift count is proven in `[0, 63]`. Every
other shape — a result that overflows i64, a count outside `[0, 63]`, a negative
count, a bool/bigint operand, or a user `__lshift__` — MUST bail to the boxed
runtime (`molt_lshift`/`molt_rshift`), which is BigInt- and exception-correct.

A raw machine shift past the operand width is poison on LLVM and a wrong-value
mask-mod-64 on WASM, so a missing guard silently produces the WRONG VALUE — the
worst bug class. This matrix straddles the 61/62/63/64/65/80/2**20-bit result
boundaries on both the binary (`<<`) and in-place (`<<=`) spellings, and is run
byte-for-byte against CPython on native + llvm.
"""


def show(label, value):
    print(label, repr(value))


# ---- Left shift straddling the i64 / inline-int47 boundaries. ----
# 47-bit window upper edge and just past it (promotes inline-int -> nothing,
# stays i64), then past 63 bits (genuine bigint).
for bits in (44, 45, 46, 47, 60, 61, 62, 63, 64, 65, 80, 127, 200):
    show(f"1<<{bits}", 1 << bits)

# Non-unit operands shifted across the boundary (operand magnitude matters for
# the result-range proof: `3 << 62` overflows even though `1 << 62` fits i64).
for bits in (60, 61, 62, 63, 64, 80):
    show(f"3<<{bits}", 3 << bits)
    show(f"255<<{bits}", 255 << bits)

# A 2**20-bit result: large but allocatable bigint (NOT an OverflowError).
big = 1 << (2 ** 20)
show("1<<2**20 bit_length", big.bit_length())
show("1<<2**20 low64", big & ((1 << 64) - 1))

# ---- Right shift: arithmetic floor semantics, negative lhs, saturation. ----
show("(1<<80)>>79", (1 << 80) >> 79)
show("(1<<80)>>80", (1 << 80) >> 80)
show("(1<<80)>>81", (1 << 80) >> 81)
show("5>>100", 5 >> 100)
show("-5>>100", -5 >> 100)
show("-5>>2", -5 >> 2)
show("-1>>5", -1 >> 5)
show("-1>>63", -1 >> 63)
show("-1>>64", -1 >> 64)
show("(-(1<<80))>>40", (-(1 << 80)) >> 40)
show("(-(1<<80))>>80", (-(1 << 80)) >> 80)
show("(-(1<<80))>>200", (-(1 << 80)) >> 200)

# ---- Shift count exactly at / past the i64 machine width. ----
# The result fits i64 (lhs small / zero) yet the COUNT is >= 64: the raw lane
# must NOT fire (LLVM poison / WASM mask-mod-64). `0 << k` is 0 for every k;
# `1 << 63` is a negative-looking i64 bit pattern only if mis-lowered raw.
show("0<<64", 0 << 64)
show("0<<70", 0 << 70)
show("0<<200", 0 << 200)
show("1<<63", 1 << 63)
show("1<<64", 1 << 64)

# ---- Bool operands promote to int (and still overflow correctly). ----
show("True<<3", True << 3)
show("True<<True", True << True)
show("False>>1", False >> 1)
show("True<<80", True << 80)
show("True<<63", True << 63)

# ---- In-place spellings must match the binary forms exactly. ----
a = 1
a <<= 80
show("a<<=80", a)

b = 1
b <<= 63
show("b<<=63", b)

c = 1 << 90
c >>= 85
show("c>>=85", c)

d = -(1 << 80)
d >>= 40
show("d>>=40 neg", d)

e = 7
e <<= 0
show("e<<=0", e)

f = 0
f <<= 200
show("f<<=200", f)

# ---- Accumulating loop (the peel / raw-lane perf shape): constant count 1,
# masked so the result stays in the inline window — must stay fast AND exact. ----
MASK = (1 << 32) - 1
s = 1
for _ in range(64):
    s = (s << 1) & MASK
show("loop (s<<1)&MASK", s)

# Accumulating loop that DOES overflow i64 (no mask): each step doubles, so the
# accumulator must promote to bigint, not wrap.
acc = 1
for _ in range(70):
    acc = acc << 1
show("loop acc<<1 x70", acc)

# ---- Negative shift count -> ValueError (binary + in-place). ----
try:
    _ = 1 << -1
except ValueError as exc:
    show("1<<-1", str(exc))

try:
    _ = 5 >> -3
except ValueError as exc:
    show("5>>-3", str(exc))

try:
    g = 1
    g <<= -10
except ValueError as exc:
    show("g<<=-10", str(exc))

try:
    _ = (1 << 80) >> -1
except ValueError as exc:
    show("(1<<80)>>-1", str(exc))

# ---- Absurd shift count -> OverflowError "too many digits in integer". ----
try:
    _ = 1 << (2 ** 63)
except OverflowError as exc:
    show("1<<2**63", str(exc))

# `0 << huge` is 0 (zero lhs short-circuits before the too-large check).
show("0<<2**63", 0 << (2 ** 63))
# `x >> huge` saturates (0 / -1), never OverflowError.
show("5>>2**63", 5 >> (2 ** 63))
show("-5>>2**63", -5 >> (2 ** 63))


# ---- User-class __lshift__ / __rshift__ chain (regression for the dunder
# routing the boxed path must preserve). ----
class Shifter:
    def __init__(self, v):
        self.v = v

    def __lshift__(self, other):
        return ("lshift", self.v, other)

    def __rshift__(self, other):
        return ("rshift", self.v, other)

    def __rlshift__(self, other):
        return ("rlshift", other, self.v)

    def __ilshift__(self, other):
        return ("ilshift", self.v, other)


sh = Shifter(10)
show("Shifter<<3", sh << 3)
show("Shifter>>2", sh >> 2)
show("5<<Shifter", 5 << sh)
si = Shifter(99)
si <<= 7
show("Shifter<<=7", si)
