"""Pure arithmetic loop — Molt's best case."""
import time


def bench() -> None:
    start = time.perf_counter_ns()
    total = 0
    i = 0
    while i < 10000000:
        total = total + i * 3 + 7
        i += 1
    end = time.perf_counter_ns()
    elapsed_ms = (end - start) / 1000000.0
    per_iter_ns = (end - start) / 10000000.0
    print("Result: " + str(total))
    print("INTERNAL_TIME_MS=" + str(int(elapsed_ms)))
    print("PER_ITER_NS=" + str(int(per_iter_ns)))


bench()
