"""Purpose: differential coverage for intrinsic-backed time.timegm semantics."""

import time


if hasattr(time, "timegm"):
    _timegm = time.timegm
else:
    import calendar

    _timegm = calendar.timegm

print(_timegm((1970, 1, 1, 0, 0, 0, 3, 1, 0)))
print(_timegm((2024, 2, 29, 12, 34, 56, 0, 0, -1)))
print(_timegm(time.gmtime(0)))

try:
    _timegm((2024, 1, 1))
except Exception as exc:
    print(type(exc).__name__)
