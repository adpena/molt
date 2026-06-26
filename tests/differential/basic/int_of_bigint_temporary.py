"""Purpose: int() of bigint values/temporaries — refcount + range parity.

Regression for two bugs in the int() constructor (molt_int_from_obj):

1. Refcount: ``int(x)`` where ``x`` is already a heap bigint returned the same
   object WITHOUT an extra reference, so the temporary argument's cleanup
   dec_ref plus the result's dec_ref double-freed one allocation
   (use-after-free abort).

2. Truncation: ``to_i64`` succeeds for bare heap bigints that fit in i64, but
   those can exceed molt's 47-bit inline-int window; ``MoltObject::from_int``
   then silently truncated them.  ``int(x)`` for any value in (2**46, 2**63]
   must stay exact.

These surfaced via ``int(IPv6Address(...))`` but are runtime-wide (math, etc.).
"""

import math

# Value in the (2**46, 2**63] window that fits i64 but NOT the inline-int tag.
mid = 2432902008176640000  # == 20!
print(int(mid))
print(int(int(mid)))  # nested
print(mid == int(mid))

# Heap bigint temporaries consumed by int() (the double-free repro).
print(int(math.factorial(20)))
print(int(math.factorial(25)))  # exceeds i64 -> stays a bigint
print(int(math.factorial(30)))

# Boundary values around the inline window and i64.
for v in [
    2**46 - 1,
    2**46,
    2**46 + 1,
    2**62,
    2**63 - 1,
    2**63,
    2**64,
    2**100,
    -(2**46) - 1,
    -(2**63),
    -(2**100),
]:
    print(v, int(v), int(v) == v)

# int() of a string whose value lands in the truncation window.
print(int("2432902008176640000"))
print(int("9223372036854775807"))  # i64 max

# int() of an object with __int__ returning a large value.
class BigInner:
    def __int__(self):
        return 2**70 + 3


print(int(BigInner()))


# __index__ path with a large value.
class BigIndex:
    def __index__(self):
        return 2**61 + 5


print(int(BigIndex()))
