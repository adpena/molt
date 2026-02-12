"""Purpose: differential coverage for multiprocessing Pool imap_unordered."""

import multiprocessing as mp


def square(value):
    return value * value


def main():
    ctx = mp.get_context("spawn")
    with ctx.Pool(processes=2) as pool:
        iterator = pool.imap_unordered(square, [1, 2, 3, 4], chunksize=1)
        result = sorted(list(iterator))
    print("imap_unordered", result)


if __name__ == "__main__":
    main()
