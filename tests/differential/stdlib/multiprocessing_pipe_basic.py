"""Purpose: differential coverage for multiprocessing Pipe IPC."""

import multiprocessing as mp


def worker(conn):
    conn.send(("ping", 123))
    conn.close()


def main():
    ctx = mp.get_context("spawn")
    parent_conn, child_conn = ctx.Pipe(duplex=False)
    proc = ctx.Process(target=worker, args=(child_conn,))
    proc.start()
    try:
        msg = parent_conn.recv()
        print("pipe", msg)
    finally:
        parent_conn.close()
        proc.join(timeout=5)
        if proc.is_alive():
            proc.terminate()
            proc.join(timeout=5)
    print("exitcode", proc.exitcode)


if __name__ == "__main__":
    main()
