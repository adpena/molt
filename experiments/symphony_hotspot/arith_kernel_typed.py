"""Pure arithmetic loop with type hints — tests fast_int path."""

import time


def bench() -> None:
    start: int = time.perf_counter_ns()
    total: int = 0
    i: int = 0
    while i < 10000000:
        total = total + i * 3 + 7
        i += 1
    end: int = time.perf_counter_ns()
    elapsed_ms: int = (end - start) // 1000000
    per_iter_ns: int = (end - start) // 10000000
    print("Result: " + str(total))
    print("INTERNAL_TIME_MS=" + str(elapsed_ms))
    print("PER_ITER_NS=" + str(per_iter_ns))


bench()
