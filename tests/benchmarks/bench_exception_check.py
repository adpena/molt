import time


def bench_happy_path():
    """5M iterations of try/except that never raises."""
    n = 5_000_000
    total = 0
    start = time.monotonic()
    for i in range(n):
        try:
            total += 1
        except ValueError:
            pass
    elapsed = time.monotonic() - start
    print(f"happy_path: {elapsed:.3f}s ({n / elapsed:.0f} iter/s)")


def bench_raising_path():
    """500K iterations, raises every 100th."""
    n = 500_000
    total = 0
    start = time.monotonic()
    for i in range(n):
        try:
            if i % 100 == 0:
                raise ValueError("test")
            total += 1
        except ValueError:
            total += 2
    elapsed = time.monotonic() - start
    print(f"raising_path: {elapsed:.3f}s ({n / elapsed:.0f} iter/s)")


bench_happy_path()
bench_raising_path()
print("DONE")
