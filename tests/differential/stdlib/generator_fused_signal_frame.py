# MOLT_ENV: MOLT_CAPABILITIES=signal.signal,signal.alarm
# MOLT_META: backends=llvm,native platforms=posix
"""A signal handler receives the executing frame inside a fused generator body."""

import signal

fired = []


def handler(signum, frame):
    fired.append((frame.f_code.co_name, frame.f_back.f_code.co_name))


def waits(n):
    for i in range(n):
        if i == 1:
            signal.alarm(1)
            spins = 0
            while not fired:
                spins += 1
        yield i


def consumer():
    total = 0
    for v in waits(3):
        total += v
    return total


signal.signal(signal.SIGALRM, handler)
print(consumer(), fired)
