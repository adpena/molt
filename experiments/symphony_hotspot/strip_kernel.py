"""Absolute minimal: just string.strip() + lower() in a loop."""

import time


def bench() -> None:
    s = "  Hello World  "
    start = time.perf_counter_ns()
    result = ""
    i = 0
    while i < 1000000:
        result = s.strip().lower()
        i += 1
    end = time.perf_counter_ns()
    elapsed_ms = (end - start) / 1000000.0
    per_call_ns = (end - start) / 1000000.0
    print("Result: " + result)
    print("INTERNAL_TIME_MS=" + str(int(elapsed_ms)))
    print("PER_CALL_NS=" + str(int(per_call_ns)))


bench()
