"""Purpose: differential coverage for Lock acquire timeout behavior."""

try:
    import threading
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    lock = threading.Lock()
    print("acquire0", lock.acquire())

    attempts: list[bool] = []

    def worker() -> None:
        got = lock.acquire(timeout=0.01)
        attempts.append(got)
        if got:
            lock.release()

    t = threading.Thread(target=worker)
    t.start()
    t.join(timeout=1.0)
    print("attempts", attempts)

    lock.release()
    try:
        lock.release()
    except Exception as exc:
        print(type(exc).__name__, exc)
