"""Purpose: differential coverage for concurrent.futures processpool surface."""
# MOLT_ENV: MOLT_CAPABILITIES=process.exec,env.read,fs.read,fs.write,thread

from __future__ import annotations

from concurrent.futures import (
    FIRST_COMPLETED,
    ProcessPoolExecutor,
    wait,
)


def mul(x: int, y: int) -> int:
    return x * y


def inc(x: int) -> int:
    return x + 1


def kwadd(base: int, *, inc_by: int = 0) -> int:
    return base + inc_by


def boom() -> int:
    raise ValueError("boom")


def main() -> None:
    with ProcessPoolExecutor(max_workers=2) as executor:
        first = executor.submit(mul, 6, 7)
        second = executor.submit(kwadd, 10, inc_by=5)
        print("process_submit", first.result(), second.result())

        mapped = list(executor.map(inc, [1, 2, 3], chunksize=1))
        print("process_map", mapped)

        futures = [executor.submit(mul, value, 2) for value in (2, 3, 4)]
        done, pending = wait(futures, return_when=FIRST_COMPLETED)
        print("process_wait_first", len(done) >= 1, len(pending) <= 2)

        failing = executor.submit(boom)
        try:
            failing.result(timeout=3)
        except Exception as exc:
            print("process_error", type(exc).__name__, str(exc))

    with ProcessPoolExecutor(max_workers=1, max_tasks_per_child=1) as executor:
        print("process_maxtasks", list(executor.map(inc, [7, 8], chunksize=1)))


if __name__ == "__main__":
    main()
