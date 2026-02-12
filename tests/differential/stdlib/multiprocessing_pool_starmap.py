"""Purpose: differential coverage for multiprocessing Pool starmap."""
# MOLT_ENV: MOLT_CAPABILITIES=process.exec,env.read

import multiprocessing as mp


def add(a, b):
    return a + b


def main():
    ctx = mp.get_context("spawn")
    with ctx.Pool(processes=2) as pool:
        result = pool.starmap(add, [(1, 2), (3, 4), (5, 6)])
    print("starmap", result)


if __name__ == "__main__":
    main()
