# MOLT_ENV: MOLT_CAPABILITIES=signal.signal,signal.alarm
# MOLT_META: backends=llvm,native platforms=posix
"""A signal reaches a loop whose only call is inlined, and sees its locals.

The call's own poll leaves with the call when the callee is inlined; the
loop's back edge still polls. The handler then reads the interrupted
function's locals: they must be a state some iteration boundary produced,
so a home written late or not at all shows up as a mismatch.
"""

import signal


def add(a, b):
    return a + b


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
    # At the back edge total holds sum(range(i + 1)); inside the call (CPython
    # polls at the callee's entry) it still holds sum(range(i)).
    consistent = total in (i * (i - 1) // 2, i * (i + 1) // 2)
    raise Stop(f"consistent={consistent}")


def kernel(n):
    total = 0
    for i in range(n):
        total = add(total, i)
    return total


old = signal.signal(signal.SIGALRM, check_locals)
try:
    signal.alarm(1)
    try:
        kernel(50_000_000_000)
        print("handler missing")
    except Stop as exc:
        print("Stop", exc)
finally:
    signal.alarm(0)
    signal.signal(signal.SIGALRM, old)

print("after", kernel(1000))
