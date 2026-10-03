"""Purpose: timeout protocol, target limit and terminal-state short circuits."""

import _thread
from concurrent.futures import Future, ThreadPoolExecutor
from threading import Condition, Event, Semaphore, TIMEOUT_MAX


class FloatOnly:
    def __gt__(self, other):
        return True

    def __float__(self):
        return 0.0


class IndexOnly:
    def __gt__(self, other):
        return True

    def __index__(self):
        return 0


class Nonpositive:
    def __gt__(self, other):
        return False


values = [
    ("nan", float("nan")),
    ("negative", -2.0),
    ("negative_inf", -float("inf")),
    ("inf", float("inf")),
    ("huge", 1e300),
    ("platform_limit", TIMEOUT_MAX + 1),
    ("huge_int", 10**1000),
    ("huge_negative_int", -(10**1000)),
    ("string", "0"),
    ("float_only", FloatOnly()),
    ("index_only", IndexOnly()),
    ("nonpositive", Nonpositive()),
]


def outcome(label, callback, timeout):
    try:
        value = callback(timeout=timeout)
        print(label, "value", value)
    except Exception as error:
        print(label, "error", type(error).__name__)


print("shared_max", TIMEOUT_MAX == _thread.TIMEOUT_MAX)
for name, timeout in values:
    for factory in (_thread.allocate_lock, _thread.RLock):
        lock = factory()
        try:
            acquired = lock.acquire(timeout=timeout)
            print("lock", name, acquired)
            if acquired:
                lock.release()
        except Exception as error:
            print("lock", name, type(error).__name__)
    condition = Condition()
    with condition:
        outcome("condition " + name, condition.wait, timeout)

condition = Condition()
token = ["predicate-result"]
with condition:
    print("predicate_short_circuit", condition.wait_for(lambda: token, object()) is token)
    print("predicate_zero", condition.wait_for(lambda: False, 0))

event = Event()
event.set()
semaphore = Semaphore(1)
for name, timeout in values:
    print("ready", name, event.wait(timeout), semaphore.acquire(timeout=timeout))
    semaphore.release()


def probe_future(label, future):
    for name, timeout in values:
        outcome(label + " result " + name, future.result, timeout)
        outcome(label + " exception " + name, future.exception, timeout)


standalone = Future()
probe_future("pending", standalone)
standalone.set_result(19)
probe_future("done", standalone)
cancelled = Future()
cancelled.cancel()
probe_future("cancelled", cancelled)

started = Event()
release = Event()


def blocked():
    started.set()
    release.wait()
    return 19


executor = ThreadPoolExecutor(max_workers=1)
try:
    running = executor.submit(blocked)
    if not started.wait(timeout=5):
        raise AssertionError("worker did not start")
    probe_future("worker_pending", running)
    cancelled = executor.submit(lambda: None)
    cancelled.cancel()
    probe_future("worker_cancelled", cancelled)
finally:
    release.set()
    executor.shutdown(wait=True)
probe_future("worker_done", running)
