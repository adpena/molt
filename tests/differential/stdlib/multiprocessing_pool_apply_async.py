"""Purpose: differential coverage for multiprocessing Pool apply_async."""

import multiprocessing as mp


def mul(a, b):
    return a * b


def main():
    ctx = mp.get_context("spawn")
    with ctx.Pool(processes=2) as pool:
        async_result = pool.apply_async(mul, args=(6, 7))
        result = async_result.get(timeout=5)
    print("apply_async", result)


if __name__ == "__main__":
    main()
