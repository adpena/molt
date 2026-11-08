# MOLT_ENV: TZ=UTC
"""Purpose: time zone constants and localtime under TZ=UTC.

UTC has no daylight saving.
"""

import time

print(time.timezone, time.altzone, time.daylight, time.tzname)
for instant in (1767268800, 1783080000):
    local = time.localtime(instant)
    print(local.tm_hour, local.tm_isdst, local.tm_zone, local.tm_gmtoff)
    print(time.mktime(local) == instant)
