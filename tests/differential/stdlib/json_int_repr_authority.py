"""Purpose: json writes every int instance as int.__repr__ does, as values and as keys."""

import enum
import json
import sys


class Color(enum.IntEnum):
    RED = 1
    HUGE = 2**70


class Loud(int):
    def __repr__(self) -> str:
        return "loud"

    def __str__(self) -> str:
        return "loud"


values = [
    0,
    -1,
    2**46,
    -(2**46),
    2**47,
    2**61,
    -(2**63),
    2**64 + 1,
    10**30,
    -(10**40),
    hash(2**62 + 5),
    Color.RED,
    Color.HUGE,
    Loud(7),
    Loud(2**80),
    True,
]
for value in values:
    print(type(value).__name__, json.dumps(value), json.dumps([value]))

print(json.dumps({v: type(v).__name__ for v in values[:-1]}, sort_keys=False))
print(json.dumps({2**70: 1, Loud(3): 2, Color.HUGE: 3}))
print(json.loads(json.dumps(10**30)) == 10**30)

limit = sys.get_int_max_str_digits()
sys.set_int_max_str_digits(640)
try:
    json.dumps(10**700)
except ValueError as exc:
    print("ValueError", exc)
try:
    json.dumps({10**700: 1})
except ValueError as exc:
    print("ValueError key", exc)
sys.set_int_max_str_digits(limit)
