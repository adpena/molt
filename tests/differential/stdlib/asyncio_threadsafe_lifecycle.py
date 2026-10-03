"""Thread-safe submission returns the concurrent Future completion contract."""
import asyncio
import concurrent.futures


async def value():
    await asyncio.sleep(0)
    return 42


async def fail(error):
    raise error


loop = asyncio.new_event_loop()
try:
    result = asyncio.run_coroutine_threadsafe(value(), loop)
    print("concurrent", isinstance(result, concurrent.futures.Future))
    try:
        result.result(timeout=0)
    except TimeoutError:
        print("pending-timeout", not result.done())
    print("result", loop.run_until_complete(asyncio.wrap_future(result, loop=loop)), result.result(0))

    error = ValueError("original")
    failed = asyncio.run_coroutine_threadsafe(fail(error), loop)
    try:
        loop.run_until_complete(asyncio.wrap_future(failed, loop=loop))
    except ValueError as caught:
        print("exception-identity", caught is error, failed.exception(0) is error)

    cancelled = asyncio.run_coroutine_threadsafe(value(), loop)
    print("cancel", cancelled.cancel(), cancelled.cancel())
    loop.run_until_complete(asyncio.sleep(0))
    loop.run_until_complete(asyncio.sleep(0))
    try:
        cancelled.result(0)
    except concurrent.futures.CancelledError:
        print("cancelled", cancelled.cancelled(), len(asyncio.all_tasks(loop)))

    try:
        asyncio.run_coroutine_threadsafe(object(), loop)
    except TypeError as caught:
        print("invalid", str(caught))
finally:
    loop.close()
