"""Purpose: differential coverage for multiprocessing Value/Array shared memory."""

import multiprocessing as mp


def worker(value, array):
    value.value += 1
    for idx in range(len(array)):
        array[idx] = array[idx] + 1


def main():
    ctx = mp.get_context("spawn")
    value = ctx.Value("i", 10)
    array = ctx.Array("i", [1, 2, 3])
    proc = ctx.Process(target=worker, args=(value, array))
    proc.start()
    proc.join(timeout=5)
    if proc.is_alive():
        proc.terminate()
        proc.join(timeout=5)
    print("value", value.value)
    print("array", list(array))
    print("exitcode", proc.exitcode)


if __name__ == "__main__":
    main()
