"""Hash metadata and consumers share the target ABI on native and wasm32."""

import sys
from decimal import Decimal
from fractions import Fraction


info = sys.hash_info
width = info.width
modulus = info.modulus
print("target modulus", modulus == (2**61 - 1 if width == 64 else 2**31 - 1))
print("modulus residues", hash(modulus) == 0, hash(modulus + 1) == 1, hash(-modulus - 1) == -2)
print("rational modulus", hash(Fraction(1, modulus)) == info.inf)
print("numeric agreement", hash(2**52 + 1) == hash(float(2**52 + 1)))
print("subnormal agreement", hash(float.fromhex("0x0.0000000000001p-1022")) == hash(Fraction(1, 2**1074)))
print("decimal agreement", hash(Decimal("-0.1")) == hash(Fraction(-1, 10)))

# Upstream CPython v3.12.0 Lib/test/test_tuple.py::test_hash_exact.
tuple_vectors = [
    ((), 750394483, 5740354900026072187),
    ((0,), 1214856301, -8753497827991233192),
    ((0, 0), -168982784, -8458139203682520985),
    ((0.5,), 2077348973, -408149959306781352),
]
print("tuple vectors", all(hash(value) == (e64 if width == 64 else e32) for value, e32, e64 in tuple_vectors))

# Upstream Objects/setobject.c: its empty/dummy contributions cancel out.
# These fixed values are derived from that algorithm at each target width.
frozen_vectors = [
    ((), -1572407560, 133146708735736),
    ((0,), -281444354, -2704248722033767810),
    ((1,), 882226578, -558064481276695278),
    ((1, 2), -489709338, -1826646154956904602),
    ((1, 2, 3), -2021384008, -272375401224217160),
]
print("frozenset vectors", all(hash(frozenset(value)) == (e64 if width == 64 else e32) for value, e32, e64 in frozen_vectors))

samples = [None, "abc", "\u00e4\u00fa\u2211\u2107", b"abc", 3000j, (0.5,), frozenset((1, 2, 3))]
low = -(2 ** (width - 1))
high = 2 ** (width - 1)
print("signed width", all(low <= hash(value) < high and hash(value) != -1 for value in samples))
print("complex width", hash(3000j) == (3000009000 if width == 64 else -1294958296))


class HashResult:
    def __init__(self, value):
        self.value = value

    def __hash__(self):
        return self.value


print("custom signed fit", hash(HashResult(high - 1)) == high - 1)
print("custom overflow", hash(HashResult(high)) == hash(high))
print("custom sentinel", hash(HashResult(-1)) == -2)
