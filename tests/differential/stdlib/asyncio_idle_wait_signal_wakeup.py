# MOLT_ENV: MOLT_CAPABILITIES=thread,signal.signal,signal.raise,process
# MOLT_META: backends=llvm,native platforms=posix
"""Signals delivered while an asyncio loop is parked reach it promptly.

Every wait is bounded by a far timeout, so a lost wake stalls and prints False
instead of hanging the suite.
"""

import asyncio
import contextvars
import os
import signal
import threading
import time

FAR = 30.0
PROMPT = 5.0
origin = contextvars.ContextVar("origin", default="unset")


def prompt(started):
    return time.monotonic() - started < PROMPT


async def handler_from_other_thread():
    loop = asyncio.get_running_loop()
    received = loop.create_future()
    origin.set("registered")
    # The callback runs in the context captured at registration.
    loop.add_signal_handler(signal.SIGUSR1, lambda: received.set_result(origin.get()))
    origin.set("changed")

    def work():
        time.sleep(0.05)
        # Thread-directed: the parked main thread is not the receiving thread.
        signal.raise_signal(signal.SIGUSR1)

    started = time.monotonic()
    thread = threading.Thread(target=work)
    thread.start()
    value = await asyncio.wait_for(received, FAR)
    thread.join()
    print("handler", value, prompt(started))
    print(
        "removed",
        loop.remove_signal_handler(signal.SIGUSR1),
        loop.remove_signal_handler(signal.SIGUSR1),
    )


def interrupt_parked_run():
    def work():
        # Keep delivery off this thread: the process-directed SIGINT must reach
        # (or wake) the parked main thread.
        signal.pthread_sigmask(signal.SIG_BLOCK, [signal.SIGINT])
        time.sleep(0.2)
        os.kill(os.getpid(), signal.SIGINT)

    started = time.monotonic()
    thread = threading.Thread(target=work)
    thread.start()
    try:
        asyncio.run(asyncio.sleep(FAR))
    except KeyboardInterrupt:
        print("interrupt", prompt(started))
    thread.join()
    print(
        "sigint restored",
        signal.getsignal(signal.SIGINT) is signal.default_int_handler,
    )


async def seven():
    await asyncio.sleep(0)
    return 7


def second_run_reinstalls_and_restores():
    started = time.monotonic()
    value = asyncio.run(seven())
    print(
        "second run",
        value,
        signal.getsignal(signal.SIGINT) is signal.default_int_handler,
        prompt(started),
    )


def close_removes_handlers():
    loop = asyncio.new_event_loop()
    loop.add_signal_handler(signal.SIGUSR2, print, "never")
    loop.close()
    print("close removed", signal.getsignal(signal.SIGUSR2) == signal.SIG_DFL)


asyncio.run(handler_from_other_thread())
interrupt_parked_run()
second_run_reinstalls_and_restores()
close_removes_handlers()
