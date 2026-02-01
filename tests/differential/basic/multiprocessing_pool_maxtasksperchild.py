"""Purpose: differential coverage for Pool maxtasksperchild behavior."""

import multiprocessing as mp


def _work(x: int) -> tuple[int, int]:
    return (x, x + 1)


def main() -> None:
    ctx = mp.get_context("spawn")
    with ctx.Pool(processes=1, maxtasksperchild=1) as pool:
        print(pool.map(_work, [1, 2, 3]))
    try:
        ctx.Pool(processes=1, maxtasksperchild=0)
    except Exception as exc:
        print(type(exc).__name__, exc)


if __name__ == "__main__":
    main()
