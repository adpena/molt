"""UTC conversion belongs to calendar; time has CPython's public surface."""

import time


import calendar

print("timegm" in dir(time), hasattr(time, "timegm"))
_timegm = calendar.timegm

print(_timegm((1970, 1, 1, 0, 0, 0, 3, 1, 0)))
print(_timegm((2024, 2, 29, 12, 34, 56, 0, 0, -1)))
print(_timegm(time.gmtime(0)))

try:
    _timegm((2024, 1, 1))
except Exception as exc:
    print(type(exc).__name__)
