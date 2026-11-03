# MOLT_ENV: MOLT_CAPABILITIES=signal.signal,signal.alarm
# MOLT_META: backends=llvm,native platforms=posix
"""OS delivery reaches a loop with no Python call in its body."""

import signal

fired = False


def record(signum, frame):
    global fired
    fired = True


def raising(signum, frame):
    raise ValueError("backedge signal")


def keyboard(signum, frame):
    raise KeyboardInterrupt


old = signal.signal(signal.SIGALRM, record)
try:
    signal.alarm(1)
    total = 0
    for i in range(500_000_000):
        total += i
        if fired:
            break
    print("handler in tight loop", fired)

    signal.signal(signal.SIGALRM, raising)
    signal.alarm(1)
    try:
        for i in range(500_000_000):
            total += i
        print("raising handler missing")
    except ValueError as exc:
        print(type(exc).__name__, exc)

    signal.signal(signal.SIGALRM, keyboard)
    signal.alarm(1)
    try:
        for i in range(500_000_000):
            total += i
        print("keyboard handler missing")
    except KeyboardInterrupt:
        print("KeyboardInterrupt at backedge")
finally:
    signal.alarm(0)
    signal.signal(signal.SIGALRM, old)
