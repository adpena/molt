"""Purpose: IPv6Address(int)/IPv4Address(int) full-range construction parity.

Regression for the P0 where IPv6Address(int) read the integer via the i64 path
and rejected every value >= 2**63 (the entire upper half of the IPv6 address
space).  The full 0..2**128 range must be constructable and round-trip through
int(), matching CPython 3.12.
"""

import ipaddress

# IPv6: cover the i64 boundary (2**63), the upper half (2**127), and the
# all-ones maximum (2**128 - 1) that the old code could not build.
v6_values = [
    0,
    1,
    2**32,
    2**63 - 1,
    2**63,  # first value the old to_i64 path rejected
    2**64,
    2**127,  # upper-half address
    2**128 - 2,
    2**128 - 1,  # all-ones, valid max
    0x20010DB8000000000000000000000001,  # 2001:db8::1
    0xFE800000000000000000000000000001,  # fe80::1
]
for n in v6_values:
    a = ipaddress.IPv6Address(n)
    # round-trip int() and render str().
    print(n, int(a), str(a), int(a) == n)

# IPv4: full 0..2**32-1 range plus boundary/out-of-range error parity.
v4_values = [0, 1, 2**24, 2**31, 2**32 - 1]
for n in v4_values:
    a = ipaddress.IPv4Address(n)
    print(n, int(a), str(a), int(a) == n)

# ip_address() dispatches int -> v4 (<=2**32-1) or v6, per CPython.
for n in [0, 2**32 - 1, 2**32, 2**128 - 1]:
    a = ipaddress.ip_address(n)
    print(n, a.version, int(a))


def construct_error(cls, value):
    try:
        cls(value)
    except ValueError:
        return "ValueError"
    except TypeError:
        return "TypeError"
    return "ok"


# Out-of-range integers raise (negative, or >= 2**width).
print(construct_error(ipaddress.IPv6Address, -1))
print(construct_error(ipaddress.IPv6Address, 2**128))
print(construct_error(ipaddress.IPv4Address, -1))
print(construct_error(ipaddress.IPv4Address, 2**32))
