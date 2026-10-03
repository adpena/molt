"""Future admission follows the declared protocol, never class spelling."""
import asyncio
from asyncio import base_futures, futures


class Future:
    pass


class Declared:
    _asyncio_future_blocking = False
    _loop = None


class InstanceOnly:
    def __init__(self):
        self._asyncio_future_blocking = False


class Disabled(Declared):
    _asyncio_future_blocking = None


loop = asyncio.new_event_loop()
try:
    actual = loop.create_future()
    for label, value in (("actual", actual), ("same-name", Future()),
                         ("protocol", Declared()), ("instance-only", InstanceOnly()),
                         ("disabled", Disabled())):
        print(label, asyncio.isfuture(value), futures.isfuture(value), base_futures.isfuture(value))
    foreign = Declared()
    foreign._loop = loop
    print("loop-admission", asyncio.ensure_future(foreign, loop=loop) is foreign)
    print("admission", asyncio.ensure_future(foreign) is foreign,
          asyncio.wrap_future(foreign) is foreign)
finally:
    loop.close()


import types


@types.coroutine
def yield_once():
    yield


async def inner_loop():
    await yield_once()
    await asyncio.sleep(0)
    return 7


async def manually_resumed_outer():
    return asyncio.run(inner_loop())


outer = manually_resumed_outer()
try:
    outer.send(None)
except StopIteration as completed:
    assert completed.value == 7
    print("nested-loop-resume", completed.value)
else:
    raise AssertionError("nested event loop escaped through manual resume")


class ForeignFuture:
    _asyncio_future_blocking = False

    def __init__(self):
        self.future = asyncio.get_running_loop().create_future()
        self.events = []

    def get_loop(self):
        return self.future.get_loop()

    def done(self):
        return self.future.done()

    def result(self):
        return self.future.result()

    def cancelled(self):
        return self.future.cancelled()

    def cancel(self, msg=None):
        return self.future.cancel(msg)

    def add_done_callback(self, callback, *, context=None):
        self.future.add_done_callback(callback, context=context)

    def remove_done_callback(self, callback):
        return self.future.remove_done_callback(callback)

    def __await__(self):
        try:
            if not self.done():
                self._asyncio_future_blocking = True
                yield self
            return self.result()
        except asyncio.CancelledError:
            self.events.append("cancelled")
            raise
        finally:
            self.events.append("finally")


async def foreign_cancellation():
    foreign = ForeignFuture()

    async def wait():
        return await foreign

    task = asyncio.create_task(wait())
    await asyncio.sleep(0)
    task.cancel("foreign-cancel")
    try:
        await task
    except asyncio.CancelledError as error:
        assert error.args == ("foreign-cancel",)
        assert foreign.events == ["cancelled", "finally"]
        print("foreign-cancellation", error.args, foreign.events)
    else:
        raise AssertionError("foreign future cancellation lost")


asyncio.run(foreign_cancellation())
