"""Purpose: differential coverage for threading.TIMEOUT_MAX."""

try:
    import threading
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    lock = threading.Lock()
    try:
        limit = threading.TIMEOUT_MAX
    except Exception as exc:
        print(type(exc).__name__, exc)
    else:
        try:
            lock.acquire(timeout=limit + 1)
        except Exception as exc:
            print(type(exc).__name__, exc)
