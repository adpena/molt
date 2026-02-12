"""Purpose: differential coverage for Semaphore/BoundedSemaphore."""

try:
    import threading
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    sem = threading.Semaphore(2)
    print(sem.acquire())
    print(sem.acquire(timeout=0.01))
    print(sem.acquire(timeout=0.01))
    sem.release()
    sem.release()

    bounded = threading.BoundedSemaphore(1)
    print(bounded.acquire())
    bounded.release()
    try:
        bounded.release()
    except Exception as exc:
        print(type(exc).__name__, exc)
