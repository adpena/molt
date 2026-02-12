"""Purpose: differential coverage for multiprocessing imap error propagation."""

import multiprocessing as mp


def boom(value):
    if value == 2:
        raise ValueError("boom")
    return value


def main():
    ctx = mp.get_context("spawn")
    with ctx.Pool(processes=1) as pool:
        iterator = pool.imap(boom, [1, 2])
        print("first", next(iterator))
        try:
            next(iterator)
            print("error", "missed")
        except Exception as exc:
            print("error", type(exc).__name__, str(exc))


if __name__ == "__main__":
    main()
