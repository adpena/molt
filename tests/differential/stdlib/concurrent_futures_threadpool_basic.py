"""Purpose: differential coverage for concurrent futures threadpool basic."""

from concurrent.futures import ThreadPoolExecutor, wait


def add(x: int, y: int) -> int:
    return x + y


with ThreadPoolExecutor(max_workers=2) as executor:
    fut1 = executor.submit(add, 1, 2)
    fut2 = executor.submit(add, 3, 4)
    done, pending = wait([fut1, fut2])
    results = sorted(f.result() for f in done)
    print(results, len(pending))
