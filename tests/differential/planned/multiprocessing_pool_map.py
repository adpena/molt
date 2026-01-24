"""Purpose: differential coverage for multiprocessing Pool map."""

import multiprocessing as mp


def square(value):
    return value * value


def main():
    ctx = mp.get_context("spawn")
    with ctx.Pool(processes=2) as pool:
        result = pool.map(square, [1, 2, 3, 4])
    print("pool", result)


if __name__ == "__main__":
    main()
