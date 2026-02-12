"""Purpose: differential coverage for time.daylight/time.altzone/time.mktime."""

import time

print(isinstance(time.daylight, int))
print(isinstance(time.altzone, int))
print(time.daylight in (0, 1))
print(abs(time.altzone - time.timezone) <= 24 * 3600)

probe = 1_700_000_000
parts = time.localtime(probe)
round_trip = time.localtime(int(time.mktime(parts)))
print(type(time.mktime(parts)).__name__)
print(tuple(parts)[:6] == tuple(round_trip)[:6])

try:
    time.mktime((2024, 1, 1))
except Exception as exc:
    print(type(exc).__name__)
