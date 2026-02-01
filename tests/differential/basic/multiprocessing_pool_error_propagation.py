"""Purpose: differential coverage for multiprocessing Pool error propagation."""

import multiprocessing as mp


def boom(value):
    if value == 2:
        raise ValueError("boom")
    return value


def main():
    ctx = mp.get_context("spawn")
    with ctx.Pool(processes=2) as pool:
        async_result = pool.apply_async(boom, args=(2,))
        try:
            async_result.get(timeout=5)
            print("error", "missed")
        except Exception as exc:
            print("error", type(exc).__name__, str(exc))


if __name__ == "__main__":
    main()
