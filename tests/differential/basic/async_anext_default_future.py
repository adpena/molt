"""Purpose: differential coverage for async anext default future."""

import asyncio


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
