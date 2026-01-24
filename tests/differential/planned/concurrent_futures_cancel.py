"""Purpose: differential coverage for concurrent futures cancel."""

from concurrent.futures import ThreadPoolExecutor


def slow() -> int:
    return 1


with ThreadPoolExecutor(max_workers=1) as executor:
    fut = executor.submit(slow)
    cancelled = fut.cancel()
    print(cancelled, fut.cancelled())
