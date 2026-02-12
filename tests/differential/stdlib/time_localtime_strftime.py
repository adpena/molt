"""Purpose: differential coverage for time.localtime/gmtime/strftime basics."""

import time

epoch_utc = time.gmtime(0)
print(tuple(epoch_utc))
print(time.strftime("%Y-%m-%d %H:%M:%S", epoch_utc))

epoch_local = time.localtime(0)
print(tuple(epoch_local))
