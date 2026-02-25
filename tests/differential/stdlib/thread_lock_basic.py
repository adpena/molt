"""Purpose: differential coverage for _thread.allocate_lock, _thread.get_ident,
_thread.TIMEOUT_MAX, _thread._count."""

import _thread

# get_ident
ident = _thread.get_ident()
print("get_ident type:", type(ident).__name__)
print("get_ident > 0:", ident > 0)
# _count
c = _thread._count()
print("_count type:", type(c).__name__)
print("_count >= 0:", c >= 0)
# TIMEOUT_MAX
print("TIMEOUT_MAX type:", type(_thread.TIMEOUT_MAX).__name__)
print("TIMEOUT_MAX > 0:", _thread.TIMEOUT_MAX > 0)
# allocate_lock
lock = _thread.allocate_lock()
print("lock type:", type(lock).__name__)
print("locked before:", lock.locked())
lock.acquire()
print("locked after acquire:", lock.locked())
lock.release()
print("locked after release:", lock.locked())
# acquire with timeout
result = lock.acquire(timeout=0.001)
print("acquire with timeout:", result)
lock.release()
