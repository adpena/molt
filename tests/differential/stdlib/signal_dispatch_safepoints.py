# MOLT_ENV: MOLT_CAPABILITIES=thread,signal.signal,signal.raise,signal.set_wakeup_fd
# MOLT_META: backends=llvm,native platforms=posix
"""Python signal handlers run on the main thread at interpreter safepoints.

Each case distinguishes CPython's eval-breaker dispatch from a runtime that
only dispatches inside an event loop (or never): the busy loop below is
bounded, and a missing dispatch prints False instead of hanging.
"""

import _thread
import signal
import threading
import time

BOUND = 5.0


def outcome(action):
    try:
        action()
    except Exception as exc:
        return f"{type(exc).__name__} {exc}"
    return "ok"


def raise_signal_runs_the_handler_before_returning():
    hits = []
    signal.signal(signal.SIGUSR1, lambda signum, frame: hits.append(signum))
    signal.raise_signal(signal.SIGUSR1)
    print("immediate", hits == [signal.SIGUSR1])
    signal.signal(signal.SIGUSR1, signal.SIG_DFL)


def delivery_on_another_thread_reaches_a_busy_main_thread():
    flag = []
    signal.signal(signal.SIGUSR2, lambda signum, frame: flag.append(signum))

    def fire():
        time.sleep(0.05)
        # Thread-directed: the handler must still run on the main thread.
        signal.raise_signal(signal.SIGUSR2)

    thread = threading.Thread(target=fire)
    started = time.monotonic()
    thread.start()
    spins = 0
    while not flag and time.monotonic() - started < BOUND:
        spins += 1
    thread.join()
    print("busy loop", flag == [signal.SIGUSR2], time.monotonic() - started < BOUND)
    signal.signal(signal.SIGUSR2, signal.SIG_DFL)


def handler_exception_propagates_from_the_safepoint():
    def boom(signum, frame):
        raise ValueError("from handler")

    signal.signal(signal.SIGUSR1, boom)
    try:
        signal.raise_signal(signal.SIGUSR1)
        print("raised", "nothing")
    except ValueError as exc:
        print("raised", exc)
    signal.signal(signal.SIGUSR1, signal.SIG_DFL)


def sigint_default_policy():
    print("default", signal.getsignal(signal.SIGINT) is signal.default_int_handler)
    try:
        signal.raise_signal(signal.SIGINT)
        print("keyboard interrupt", "missing")
    except KeyboardInterrupt:
        print("keyboard interrupt", "raised")
    signal.signal(signal.SIGINT, signal.SIG_DFL)
    print("sig_dfl", signal.getsignal(signal.SIGINT) == signal.SIG_DFL)
    previous = signal.signal(signal.SIGINT, signal.default_int_handler)
    print(
        "restored",
        previous == signal.SIG_DFL,
        signal.getsignal(signal.SIGINT) is signal.default_int_handler,
    )


def interrupt_main_simulates_a_delivery():
    try:
        _thread.interrupt_main()
        for _ in range(1000):
            pass
        print("interrupt_main", "not delivered")
    except KeyboardInterrupt:
        print("interrupt_main", "delivered")
    signal.signal(signal.SIGINT, signal.SIG_DFL)
    _thread.interrupt_main()
    for _ in range(1000):
        pass
    print("interrupt_main", "ignored under SIG_DFL")
    signal.signal(signal.SIGINT, signal.default_int_handler)
    print("interrupt_main range", outcome(lambda: _thread.interrupt_main(0)))


def main_thread_only_apis():
    seen = []

    def from_thread():
        seen.append(outcome(lambda: signal.signal(signal.SIGUSR1, signal.SIG_IGN)))
        seen.append(outcome(lambda: signal.set_wakeup_fd(-1)))

    thread = threading.Thread(target=from_thread)
    thread.start()
    thread.join()
    print("thread signal", seen[0])
    print("thread wakeup", seen[1])


raise_signal_runs_the_handler_before_returning()
delivery_on_another_thread_reaches_a_busy_main_thread()
handler_exception_propagates_from_the_safepoint()
sigint_default_policy()
interrupt_main_simulates_a_delivery()
main_thread_only_apis()
