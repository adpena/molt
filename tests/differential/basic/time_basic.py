"""Purpose: differential coverage for time basics."""

import time

print(time.time() > 0)
print(time.monotonic() > 0)
info = time.get_clock_info("monotonic")
print(info.monotonic)
