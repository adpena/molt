"""Purpose: differential coverage for multiprocessing spawn + queue IPC."""

import multiprocessing as mp


def worker(queue):
    queue.put(("ok", 21 * 2))


def main():
    ctx = mp.get_context("spawn")
    queue = ctx.Queue()
    proc = ctx.Process(target=worker, args=(queue,))
    proc.start()
    try:
        msg = queue.get(timeout=5)
        print("msg", msg)
    finally:
        proc.join(timeout=5)
        if proc.is_alive():
            proc.terminate()
            proc.join(timeout=5)
        queue.close()
        queue.join_thread()
    print("exitcode", proc.exitcode)


if __name__ == "__main__":
    main()
