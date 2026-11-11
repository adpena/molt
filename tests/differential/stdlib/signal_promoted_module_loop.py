# MOLT_ENV: MOLT_CAPABILITIES=signal.signal,signal.alarm
# MOLT_META: backends=llvm,native platforms=posix
"""A module-level loop that carries its variables in registers still serves a
signal. The handler reads what the loop stored, and the loop reads what the
handler stored. A loop that missed the signal runs to its bound and prints
False instead of hanging."""

import signal

seen = -1


def stop_the_loop(signum, frame):
    global seen, limit
    seen = count
    limit = 0


def boom(signum, frame):
    raise ValueError("stop")


old = signal.signal(signal.SIGALRM, stop_the_loop)
try:
    signal.alarm(1)
    count = 0
    limit = 4_000_000_000
    while count < limit:
        count = count + 1
    print("the loop saw the handler's store", limit == 0)
    # CPython polls after the loop test, so one more iteration may follow.
    print("the handler saw the loop's store", 0 < seen <= count <= seen + 1)

    signal.signal(signal.SIGALRM, boom)
    signal.alarm(1)
    total = 0
    n = 0
    try:
        while n < 4_000_000_000:
            total = total + n
            n = n + 1
        print("handler missing")
    except ValueError as exc:
        print(type(exc).__name__, exc)
        print("both stores are visible together", total == n * (n - 1) // 2, n > 0)
finally:
    signal.alarm(0)
    signal.signal(signal.SIGALRM, old)
