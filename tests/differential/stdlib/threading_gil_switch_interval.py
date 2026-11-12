# MOLT_ENV: MOLT_CAPABILITIES=thread.spawn,thread.shared
# MOLT_META: backends=llvm,native platforms=posix
"""A thread that waits for the GIL gets it from a holder that never blocks.

CPython's waiter asks the holder to drop the GIL after the switch interval,
and the holder hands it over at its next eval-breaker check. A runtime that
releases the GIL only around blocking calls starves the other thread for the
whole busy loop. Each loop is bounded, so a starved thread prints False
instead of hanging.
"""

import sys
import threading
import time

BOUND = 5.0


def busy_wait_for(event_list, started):
    while not event_list and time.monotonic() - started < BOUND:
        pass
    return bool(event_list)


def new_thread_starts_while_the_holder_spins():
    ran = []
    thread = threading.Thread(target=lambda: ran.append(1))
    started = time.monotonic()
    thread.start()
    print("new thread ran", busy_wait_for(ran, started))
    thread.join()


def sleeping_thread_reacquires_while_the_holder_spins():
    woke = []

    def worker():
        time.sleep(0.05)
        woke.append(1)

    thread = threading.Thread(target=worker)
    started = time.monotonic()
    thread.start()
    print("sleeper reacquired", busy_wait_for(woke, started))
    thread.join()


def two_spinning_threads_both_progress():
    counts = [0, 0]
    stop = []

    def spin(slot):
        while not stop:
            counts[slot] += 1

    threads = [threading.Thread(target=spin, args=(i,)) for i in range(2)]
    for thread in threads:
        thread.start()
    started = time.monotonic()
    while (counts[0] == 0 or counts[1] == 0) and time.monotonic() - started < BOUND:
        time.sleep(0.01)
    stop.append(1)
    for thread in threads:
        thread.join()
    print("both spinners progressed", counts[0] > 0 and counts[1] > 0)


def a_short_interval_is_honoured():
    previous = sys.getswitchinterval()
    sys.setswitchinterval(0.001)
    try:
        sleeping_thread_reacquires_while_the_holder_spins()
    finally:
        sys.setswitchinterval(previous)
    print("interval restored", sys.getswitchinterval() == previous)


new_thread_starts_while_the_holder_spins()
sleeping_thread_reacquires_while_the_holder_spins()
two_spinning_threads_both_progress()
a_short_interval_is_honoured()
