"""Independent oracle for Future terminal-state normalization and failure atomicity."""

import asyncio


class ConstructorFailure(Exception):
    def __init__(self):
        raise LookupError("constructor")


async def main():
    loop = asyncio.get_running_loop()
    for value in (ValueError, ValueError("identity"), StopIteration("stop")):
        future = loop.create_future()
        future.set_exception(value)
        error = future.exception()
        print("stored", type(error).__name__, error is value)
        try:
            future.result()
        except BaseException as caught:
            print('result-identity', caught is error)
        if isinstance(value, StopIteration):
            print("chain", error.__cause__ is value, error.__context__ is value)
        try:
            await future
        except BaseException as caught:
            print("await", caught is error)
        try:
            future.set_exception(object())
        except BaseException as caught:
            print("terminal", type(caught).__name__, future.exception() is error)
    for invalid in (None, 7, object(), int, ConstructorFailure):
        future = loop.create_future()
        try:
            future.set_exception(invalid)
        except BaseException as caught:
            print("invalid", type(caught).__name__, future.done())
        future.set_result("still pending")
        print("recovered", await future)
    future = loop.create_future()
    future.cancel("cancelled")
    try:
        future.set_exception(ConstructorFailure)
    except BaseException as caught:
        print("cancelled", type(caught).__name__, future.cancelled())


asyncio.run(main())
