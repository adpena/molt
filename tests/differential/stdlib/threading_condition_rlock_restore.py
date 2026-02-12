"""Purpose: verify Condition wait restores RLock recursion depth."""

try:
    import threading
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    lock = threading.RLock()
    cond = threading.Condition(lock)
    events: list[str] = []
    ready = threading.Event()

    def worker() -> None:
        lock.acquire()
        lock.acquire()
        events.append("acquired_twice")
        ready.set()
        ok = cond.wait(timeout=0.5)
        events.append(f"wait:{ok}")
        try:
            lock.release()
            lock.release()
            events.append("released_twice")
        except Exception as inner:
            events.append(type(inner).__name__)

    t = threading.Thread(target=worker)
    t.start()

    ready.wait(timeout=0.5)
    with cond:
        cond.notify_all()

    t.join(timeout=1.0)
    events.append(f"alive:{t.is_alive()}")
    probe = lock.acquire(False)
    if probe:
        lock.release()
    events.append(f"probe:{probe}")
    print(events)
