"""Purpose: differential coverage for multiprocessing imap timeouts."""

import multiprocessing as mp
import time


def slow(value):
    time.sleep(0.2)
    return value


def main():
    ctx = mp.get_context("spawn")
    with ctx.Pool(processes=1) as pool:
        iterator = pool.imap(slow, [1, 2])
        try:
            iterator.next(timeout=0.01)
            print("timeout", "missed")
        except Exception as exc:
            print("timeout", type(exc).__name__)
        print("first", iterator.next(timeout=5))


if __name__ == "__main__":
    main()
