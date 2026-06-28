"""Decimal float literals and json.loads must preserve CPython f64 bits."""

import json


def ident(x):
    return x


hard = [
    0.9999999999999999,
    123456789012345.67,
    9999999999999999.0,
    1.0000000000000002,
    2.2250738585072011e-308,
    5e-324,
]
for value in hard:
    print(value.hex())

nines = [0.9999999999999999]
print(nines[0] < 1.0)
print(nines[0] == 1.0)
print(ident(0.9999999999999999) < 1.0)
print(ident(123456789012345.67).hex())

for value in [0.1, 0.2, 0.3, 3.141592653589793, 1e300, 1e23]:
    print(value.hex())

print(repr(0.9999999999999999))
print(str(123456789012345.67))
print(repr(1.0000000000000002))

print(json.loads("0.9999999999999999").hex())
print(json.loads("123456789012345.67").hex())
print(json.loads("[0.1, 0.2, 0.9999999999999999]")[2] < 1.0)
