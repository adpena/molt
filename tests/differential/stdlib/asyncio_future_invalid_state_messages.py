"""Purpose: differential coverage for Future InvalidStateError message parity.

Pins the exact CPython 3.12+ (_asyncio C accelerator) message strings:
  - Future.result() on a pending future -> InvalidStateError('Result is not set.')
  - Future.exception() on a pending future -> InvalidStateError('Exception is not set.')
  - Future.set_result()/set_exception() on a done future -> InvalidStateError(...)

These messages desynchronized when HEADER_FLAG_TASK_DONE (native header flag)
and the Future done-state had two independent sources of truth (task #25).
"""

import asyncio


async def main() -> None:
    loop = asyncio.get_running_loop()

    # result() before done.
    fut: asyncio.Future[int] = loop.create_future()
    try:
        fut.result()
    except asyncio.InvalidStateError as exc:
        print("result_pending", repr(str(exc)))

    # exception() before done.
    fut2: asyncio.Future[int] = loop.create_future()
    try:
        fut2.exception()
    except asyncio.InvalidStateError as exc:
        print("exception_pending", repr(str(exc)))

    # set_result on an already-done future raises InvalidStateError('invalid state').
    fut3: asyncio.Future[int] = loop.create_future()
    fut3.set_result(1)
    try:
        fut3.set_result(2)
    except asyncio.InvalidStateError as exc:
        print("set_result_done", type(exc).__name__, repr(str(exc)))

    # set_exception on an already-done future raises InvalidStateError('invalid state').
    fut4: asyncio.Future[int] = loop.create_future()
    fut4.set_result(1)
    try:
        fut4.set_exception(ValueError("x"))
    except asyncio.InvalidStateError as exc:
        print("set_exception_done", type(exc).__name__, repr(str(exc)))

    # After a result is set, result()/exception() succeed (state is consistent).
    fut5: asyncio.Future[int] = loop.create_future()
    fut5.set_result(42)
    print("result_ok", fut5.result(), fut5.done())
    print("exception_ok", fut5.exception())

    # After an exception is set, exception() returns it; result() re-raises it.
    fut6: asyncio.Future[int] = loop.create_future()
    fut6.set_exception(RuntimeError("boom"))
    got = fut6.exception()
    print("exception_set", type(got).__name__, str(got), fut6.done())
    try:
        fut6.result()
    except RuntimeError as exc:
        print("result_reraise", type(exc).__name__, str(exc))


asyncio.run(main())
