# MOLT_ENV: MOLT_CAPABILITIES=thread,signal.signal,signal.raise
# MOLT_META: backends=llvm,native
"""Loop signal-handler registration follows CPython's Unix event loop."""

import asyncio
import signal
import sys
import threading


def outcome(action):
    try:
        action()
    except Exception as exc:
        return f"{type(exc).__name__} {exc}"
    return "ok"


async def coroutine_callback():
    return None


def registration_errors():
    loop = asyncio.new_event_loop()
    try:
        print(
            "coroutine",
            outcome(lambda: loop.add_signal_handler(signal.SIGUSR1, coroutine_callback)),
        )
        print("not int", outcome(lambda: loop.add_signal_handler("x", print)))
        print("invalid", outcome(lambda: loop.add_signal_handler(4242, print)))
        seen = []

        def from_thread():
            seen.append(outcome(lambda: loop.add_signal_handler(signal.SIGUSR1, print)))

        thread = threading.Thread(target=from_thread)
        thread.start()
        thread.join()
        print("thread", seen[0])
    finally:
        loop.close()
    print("closed", outcome(lambda: loop.add_signal_handler(signal.SIGUSR1, print)))


def sigint_removal_restores_default_int_handler():
    loop = asyncio.new_event_loop()
    try:
        loop.add_signal_handler(signal.SIGINT, print, "never")
        print("sigint removed", loop.remove_signal_handler(signal.SIGINT))
        print(
            "sigint default",
            signal.getsignal(signal.SIGINT) is signal.default_int_handler,
        )
    finally:
        loop.close()


async def count_deliveries():
    loop = asyncio.get_running_loop()
    seen = []
    loop.add_signal_handler(signal.SIGUSR2, seen.append, "usr2")
    signal.raise_signal(signal.SIGUSR2)
    signal.raise_signal(signal.SIGUSR2)
    for _ in range(5):
        await asyncio.sleep(0)
    loop.remove_signal_handler(signal.SIGUSR2)
    print("deliveries", seen)


if sys.platform == "win32":
    loop = asyncio.new_event_loop()
    try:
        # CPython Windows loops reject before validating signal, callback,
        # closed state, or attempting any OS registration.
        for sig in (signal.SIGINT, "invalid", 4242):
            for callback in (print, coroutine_callback):
                result = outcome(lambda: loop.add_signal_handler(sig, callback))
                assert result == "NotImplementedError "
                print("windows add", result)
            result = outcome(lambda: loop.remove_signal_handler(sig))
            assert result == "NotImplementedError "
            print("windows remove", result)
    finally:
        loop.close()
    print("windows closed add", outcome(lambda: loop.add_signal_handler(signal.SIGINT, print)))
    print("windows closed remove", outcome(lambda: loop.remove_signal_handler(signal.SIGINT)))
else:
    registration_errors()
    sigint_removal_restores_default_int_handler()
    asyncio.run(count_deliveries())
