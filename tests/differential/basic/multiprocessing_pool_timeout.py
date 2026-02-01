"""Purpose: differential coverage for multiprocessing Pool timeout handling."""

import multiprocessing as mp
import time


def slow(value):
    time.sleep(0.2)
    return value


def main():
    ctx = mp.get_context("spawn")
    with ctx.Pool(processes=1) as pool:
        async_result = pool.map_async(slow, [1])
        try:
            async_result.get(timeout=0.01)
            print("timeout", "missed")
        except Exception as exc:
            print("timeout", type(exc).__name__)
        result = async_result.get(timeout=5)
    print("result", result)


if __name__ == "__main__":
    main()
