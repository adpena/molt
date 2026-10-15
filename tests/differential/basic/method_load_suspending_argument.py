"""Purpose: a method call whose argument suspends keeps CPython's order (HF-216).

In a generator and a coroutine the method is loaded before the suspension, and
an argument that rebinds the method while suspended does not change the call.
"""

import asyncio


class Sink:
    def __init__(self):
        self.seen = []

    def put(self, *values):
        self.seen.append(values)
        return len(self.seen)


def generator(sink):
    count = sink.put((yield "first"), (yield "second"))
    yield count


sink = Sink()
gen = generator(sink)
print(next(gen))
print(gen.send("a"))
Sink.put = lambda self, *values: "rebound"
print(gen.send("b"))
print(sink.seen)
del Sink.put


class Recorder:
    def __init__(self):
        self.seen = []

    def record(self, value, other):
        self.seen.append((value, other))
        return value + other


async def produce(value):
    await asyncio.sleep(0)
    return value


async def main():
    recorder = Recorder()
    first = recorder.record(await produce(1), await produce(2))
    second = recorder.record(3, await produce(4))
    return first, second, recorder.seen


print(asyncio.run(main()))
