"""Purpose: differential coverage for multiprocessing Pool starmap_async."""

import multiprocessing as mp


def add(a, b):
    return a + b


def main():
    ctx = mp.get_context("spawn")
    with ctx.Pool(processes=2) as pool:
        async_result = pool.starmap_async(add, [(1, 2), (3, 4), (5, 6)])
        result = async_result.get(timeout=5)
    print("starmap_async", result)


if __name__ == "__main__":
    main()
