"""Purpose: differential coverage for multiprocessing Pool map_async."""

import multiprocessing as mp


def square(value):
    return value * value


def main():
    ctx = mp.get_context("spawn")
    with ctx.Pool(processes=2) as pool:
        async_result = pool.map_async(square, [1, 2, 3, 4])
        result = async_result.get(timeout=5)
    print("map_async", result)


if __name__ == "__main__":
    main()
