# MOLT_ENV: MOLT_CAPABILITIES=signal.signal,signal.alarm
# MOLT_META: backends=llvm,native platforms=posix
"""A signal reaches a call-free counted loop and sees its locals.

The loop polls once per chunk of iterations, at the boundary where its exit
test meets the chunk limit, not on every back edge. The handler reads the
interrupted function's locals: CPython polls at the back edge, after
`total += i`, so `total` must equal sum(range(i + 1)) exactly. A home the
boundary wrote stale, or the next iteration's counter, shows as a mismatch.
"""

import signal


class Stop(Exception):
    pass


def kernel_frame(frame):
    while frame is not None and frame.f_code.co_name != "kernel":
        frame = frame.f_back
    return frame


def check_locals(signum, frame):
    frame = kernel_frame(frame)
    if frame is None:
        raise Stop("no kernel frame")
    local = frame.f_locals
    i, total = local["i"], local["total"]
    raise Stop(f"exact={total == i * (i + 1) // 2} started={i > 0}")


def kernel(n):
    total = 0
    for i in range(n):
        total += i
    return total


old = signal.signal(signal.SIGALRM, check_locals)
try:
    signal.alarm(1)
    try:
        kernel(2**45)
        print("handler missing")
    except Stop as exc:
        print("Stop", exc)
finally:
    signal.alarm(0)
    signal.signal(signal.SIGALRM, old)

print("after", kernel(5000))
