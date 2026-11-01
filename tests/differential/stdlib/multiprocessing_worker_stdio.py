"""Worker output and IPC remain independent (HF-627)."""
# MOLT_ENV: MOLT_CAPABILITIES=process.exec,env.read,fs.read,fs.write

import multiprocessing as mp
import os
import sys


def report(label):
    print(label, "stdout", flush=True)
    print(label, "stderr", file=sys.stderr, flush=True)
    os.write(1, (label + " fd1\n").encode())
    os.write(2, (label + " fd2\n").encode())
    print(label, "stdin", repr(sys.stdin.read()), flush=True)


def worker(queue, pipe):
    report("process")
    queue.put(("queue", 42))
    pipe.send(("pipe", 43))
    pipe.close()


def task(value):
    report("pool")
    return value * value


def main():
    ctx = mp.get_context("spawn")
    queue = ctx.Queue()
    reader, writer = ctx.Pipe(duplex=False)
    proc = ctx.Process(target=worker, args=(queue, writer))
    proc.start()
    try:
        print(queue.get(timeout=10), flush=True)
        print(reader.recv(), flush=True)
    finally:
        proc.join(timeout=10)
        if proc.is_alive():
            proc.terminate()
            proc.join(timeout=10)
        reader.close()
        writer.close()
        queue.close()
        queue.join_thread()
    print("exit", proc.exitcode, flush=True)
    with ctx.Pool(4) as pool:
        print("result", pool.map(task, [7]), flush=True)


if __name__ == "__main__":
    mp.freeze_support()
    main()
