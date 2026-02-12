"""Purpose: differential coverage for threading rlock local."""

import threading


r = threading.RLock()
print(r.acquire())
print(r.acquire())
r.release()
r.release()

local = threading.local()
local.value = 1
print(local.value)
