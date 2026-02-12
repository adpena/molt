"""Purpose: differential coverage for time.asctime/ctime/tzname/timezone basics."""

import time

print(isinstance(time.timezone, int))
print(isinstance(time.tzname, tuple), len(time.tzname))
print(all(isinstance(item, str) for item in time.tzname))
print(time.asctime(time.gmtime(0)))
print(len(time.ctime(0)))
