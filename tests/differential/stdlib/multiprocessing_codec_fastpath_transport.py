"""Purpose: differential coverage for multiprocessing codec fast-path transport."""

import multiprocessing as mp


def _payload(seed: int) -> dict[str, object]:
    return {
        "seed": seed,
        "nums": [1 + seed, -2 - seed, 2**80 + seed],
        "blob": b"abc\x00" + bytes([seed % 256]),
        "pairs": (("left", 10 + seed), ("right", 20 + seed)),
        "set": {1 + seed, 2 + seed, 3 + seed},
        "frozen": frozenset({"x", "y", f"z{seed}"}),
        "complex": complex(3.5 + seed, -1.25),
        "nested": {"ok": True, "none": None, "flt": 2.5 + seed},
    }


def _normalize(value):
    if isinstance(value, dict):
        items = [(_normalize(key), _normalize(item)) for key, item in value.items()]
        items.sort(key=repr)
        return ("dict", tuple(items))
    if isinstance(value, list):
        return ("list", tuple(_normalize(item) for item in value))
    if isinstance(value, tuple):
        return ("tuple", tuple(_normalize(item) for item in value))
    if isinstance(value, set):
        return ("set", tuple(sorted((_normalize(item) for item in value), key=repr)))
    if isinstance(value, frozenset):
        return (
            "frozenset",
            tuple(sorted((_normalize(item) for item in value), key=repr)),
        )
    if isinstance(value, complex):
        return ("complex", value.real, value.imag)
    return value


def _queue_pipe_worker(queue, conn) -> None:
    payload = _payload(3)
    queue.put(payload)
    conn.send(payload)
    conn.close()


def _pool_worker(seed: int) -> dict[str, object]:
    return _payload(seed)


def main() -> None:
    ctx = mp.get_context("spawn")
    queue = ctx.Queue()
    recv_conn, send_conn = ctx.Pipe(duplex=False)

    proc = ctx.Process(target=_queue_pipe_worker, args=(queue, send_conn))
    proc.start()
    try:
        queue_payload = queue.get(timeout=5)
        pipe_payload = recv_conn.recv()
    finally:
        recv_conn.close()
        proc.join(timeout=5)
        if proc.is_alive():
            proc.terminate()
            proc.join(timeout=5)

    queue_norm = _normalize(queue_payload)
    pipe_norm = _normalize(pipe_payload)
    print("queue_pipe_equal", queue_norm == pipe_norm)
    print("queue_norm", queue_norm)
    print("pipe_norm", pipe_norm)
    print("proc_exitcode", proc.exitcode)

    with ctx.Pool(processes=1) as pool:
        pool_payload = pool.apply_async(_pool_worker, args=(7,)).get(timeout=5)
    print("pool_norm", _normalize(pool_payload))


if __name__ == "__main__":
    main()
