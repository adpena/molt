"""Purpose: differential coverage for async anext default future."""

import asyncio
import sys


class AsyncIter:
    def __init__(self) -> None:
        self.i = 0

    def __aiter__(self) -> "AsyncIter":
        return self

    async def __anext__(self) -> int:
        if self.i >= 1:
            raise StopAsyncIteration
        val = self.i
        self.i += 1
        return val


async def main() -> int:
    it = AsyncIter()
    fut = anext(it, 10)
    first = await fut
    second = await anext(it, 20)
    return first + second


print(asyncio.run(main()))


class DerivedStop(StopAsyncIteration):
    pass


class StopAsyncIteration(Exception):
    pass


class FailingIterator:
    def __init__(self, error):
        self.error = error

    def __aiter__(self):
        return self

    async def __anext__(self):
        await asyncio.sleep(0)
        raise self.error


async def termination_identity():
    actual_stop = DerivedStop("done")
    actual_stop.__class__.__name__ = "RenamedTermination"
    print("derived-stop", await anext(FailingIterator(actual_stop), 73))
    impostor = StopAsyncIteration("same spelling")
    try:
        await anext(FailingIterator(impostor), 91)
    except Exception as caught:
        print("same-name-propagates", caught is impostor, str(caught))
    else:
        print("same-name-propagates", False)

    # Iteration consumes the canonical termination subclass in its continuation,
    # before the surrounding exception handler observes any pending exception.
    try:
        values = [item async for item in FailingIterator(actual_stop)]
    except BaseException:
        print("comprehension-termination", False)
    else:
        print("comprehension-termination", values)
    try:
        values = [item async for item in FailingIterator(impostor)]
    except Exception as caught:
        print("comprehension-same-name", caught is impostor)
    else:
        print("comprehension-same-name", False)

    try:
        raise LookupError("outer")
    except LookupError as outer:
        async for item in FailingIterator(actual_stop):
            print("unreachable", item)
        else:
            print("async-for-else", True)
        try:
            raise
        except LookupError as restored:
            print("outer-exception-restored", restored is outer)


asyncio.run(termination_identity())


class ImmediateIterator:
    def __aiter__(self):
        return self

    def __init__(self, error):
        self.error = error

    def __anext__(self):
        raise self.error


async def anext_with_handled_owner():
    outer = RuntimeError("handled")
    try:
        raise outer
    except RuntimeError:
        # Both successful await and consumed termination preserve the handler.
        it = AsyncIter()
        print("handled-anext-value", await anext(it, 99), sys.exception() is outer)
        print(
            "handled-anext-default",
            await anext(FailingIterator(DerivedStop("done")), 99),
            sys.exception() is outer,
        )
        immediate = ValueError("immediate")
        try:
            await anext(ImmediateIterator(immediate), 88)
        except ValueError as caught:
            print(
                "handled-anext-immediate",
                caught is immediate,
                caught.__context__ is outer,
            )
        print("handled-anext-immediate-restored", sys.exception() is outer)
        immediate_stop = DerivedStop("immediate stop")
        try:
            await anext(ImmediateIterator(immediate_stop), 88)
        except DerivedStop as caught:
            print(
                "handled-anext-immediate-stop",
                caught is immediate_stop,
                caught.__context__ is outer,
            )
        else:
            print("handled-anext-immediate-stop", False)
        print("handled-anext-immediate-stop-restored", sys.exception() is outer)
        async for value in ImmediateIterator(DerivedStop("loop stop")):
            print("immediate-loop-unreachable", value)
        else:
            print("immediate-loop-consumed", sys.exception() is outer)
        values = [
            value
            async for value in ImmediateIterator(DerivedStop("comprehension stop"))
        ]
        print("immediate-comprehension-consumed", values, sys.exception() is outer)
        inner = ValueError("pending")
        try:
            await anext(FailingIterator(inner), 77)
        except ValueError as caught:
            print("handled-anext-error", caught is inner, caught.__context__ is outer)
        print("handled-anext-restored", sys.exception() is outer)


asyncio.run(anext_with_handled_owner())


# Acquisition happens even without awaiting; conversion stays deferred.
import builtins


class DeferredConversion:
    def __init__(self, events, error=None):
        self.events = events
        self.error = error

    def __await__(self):
        self.events.append("await")
        if self.error is not None:
            raise self.error
        return iter(())


class ReturningIterator:
    def __init__(self, events, result):
        self.events = events
        self.result = result

    def __anext__(self):
        self.events.append("anext")
        return self.result


async def acquisition_boundaries():
    for next_call in (anext, builtins.anext):
        events = []
        wrapped = next_call(ReturningIterator(events, DeferredConversion(events)), 91)
        print("anext-call-time", events)
        print("anext-converted-value", await wrapped, events)
        for error in (DerivedStop("conversion stop"), ValueError("conversion failure")):
            events = []
            wrapped = next_call(ReturningIterator(events, DeferredConversion(events, error)), 92)
            try:
                await wrapped
            except Exception as caught:
                print("anext-conversion-error", caught is error, events)
            else:
                print("anext-conversion-error", False)
        immediate = DerivedStop("call stop")
        try:
            next_call(ImmediateIterator(immediate), 93)
        except DerivedStop as caught:
            print("anext-call-error", caught is immediate)
        else:
            print("anext-call-error", False)
        invalid = next_call(ReturningIterator([], 42), 94)
        try:
            await invalid
        except TypeError:
            print("anext-invalid-deferred", True)


asyncio.run(acquisition_boundaries())
