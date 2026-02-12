"""Purpose: differential coverage for RLock reentrancy."""

try:
    import threading
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    rlock = threading.RLock()
    print("first", rlock.acquire())
    print("second", rlock.acquire())
    print("locked", rlock.locked())
    rlock.release()
    print("locked1", rlock.locked())
    rlock.release()
    print("locked2", rlock.locked())
