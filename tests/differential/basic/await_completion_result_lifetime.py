"""Completion transfers its result and preserves cleanup that suspends."""


class Result:
    def __init__(self, label):
        self.label = label

    def __del__(self):
        print("released", self.label)


async def ready():
    return Result("coroutine")


async def ready_error():
    raise StopIteration("raised in ready coroutine")


async def generator():
    yield Result("async generator operation")


class Tick:
    def __await__(self):
        yield "suspended in cleanup"
        yield "still suspended in cleanup"


async def direct_cleanup(label):
    try:
        return Result(label)
    finally:
        await Tick()
        print("direct cleanup resumed")


class Worker:
    async def ready(self):
        try:
            return Result("async method")
        finally:
            await Tick()
            print("cleanup resumed")


async def consume_once(operand):
    result = await operand
    print("returned", result.label)
    del result
    print("before reuse")
    try:
        await operand
    except RuntimeError as error:
        print(type(error).__name__, str(error))


async def main():
    await consume_once(ready())
    source = generator()
    await consume_once(source.__anext__())
    await source.aclose()
    await consume_once(Worker().ready())
    for label in ("first direct coroutine", "second direct coroutine"):
        result = await ready()
        print("returned directly", result.label)
        del result
        try:
            await ready_error()
        except RuntimeError as error:
            print(str(error), type(error.__cause__).__name__, str(error.__cause__))
        result = await direct_cleanup(label)
        print("returned", result.label)
        del result
        print("after direct release")


task = main()
try:
    while True:
        print(task.send(None))
except StopIteration:
    pass
