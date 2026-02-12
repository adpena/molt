"""Purpose: differential coverage for Barrier synchronization."""

import time

try:
    import threading
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    barrier = threading.Barrier(2, timeout=0.2)
    results: list[object] = []

    def worker() -> None:
        try:
            idx = barrier.wait()
            results.append(("worker", idx, barrier.broken))
        except Exception as exc:
            results.append(("worker", type(exc).__name__))

    t = threading.Thread(target=worker)
    t.start()
    deadline = time.monotonic() + 1.0
    while barrier.n_waiting < 1 and t.is_alive():
        if time.monotonic() >= deadline:
            break
        time.sleep(0.001)
    try:
        idx = barrier.wait()
        results.append(("main", idx, barrier.broken))
    except Exception as exc:
        results.append(("main", type(exc).__name__))

    t.join(timeout=1.0)
    print(sorted(results))
