"""Purpose: preserve Future terminal error classes and stored object identity."""

import builtins
from concurrent.futures import CancelledError, ThreadPoolExecutor, TimeoutError
from threading import Event


def terminal_error(future, expected):
    for method in (future.result, future.exception):
        try:
            method(timeout=0)
        except expected as error:
            print(type(error) is expected, error.args)
        else:
            raise AssertionError("terminal read did not raise")


print("timeout_alias", TimeoutError is builtins.TimeoutError)
started = Event()
release = Event()
payload = ["retained"]


def blocked():
    started.set()
    release.wait()
    return payload


executor = ThreadPoolExecutor(max_workers=1)
try:
    running = executor.submit(blocked)
    if not started.wait(timeout=5):
        raise AssertionError("worker did not start")
    terminal_error(running, TimeoutError)
    print("still_running", running.running(), running.done())
    cancelled = executor.submit(lambda: None)
    print("cancel", cancelled.cancel(), cancelled.cancel())
    terminal_error(cancelled, CancelledError)
    cancelled_by_shutdown = executor.submit(lambda: None)
    executor.shutdown(wait=False, cancel_futures=True)
    terminal_error(cancelled_by_shutdown, CancelledError)
finally:
    release.set()
    executor.shutdown(wait=True)

first = running.result()
second = running.result()
print("result_identity", first is payload, first is second, running.exception() is None)
del running
print("result_survives", first, second is first)

original = ValueError("original")


def fail():
    raise original


with ThreadPoolExecutor(max_workers=1) as executor:
    failed = executor.submit(fail)
    first_error = failed.exception()
    second_error = failed.exception()
    print("exception_identity", first_error is original, second_error is first_error)
    try:
        failed.result()
    except ValueError as raised:
        print("raised_identity", raised is original)
    else:
        raise AssertionError("worker failure did not raise")
del failed
print("exception_survives", first_error.args, second_error is first_error)
