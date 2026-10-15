"""Purpose: method calls whose arguments suspend by yield or async comprehension stay exact (HF-216).

A `yield` or `yield from` argument suspends a generator, and an async list
comprehension argument suspends a coroutine, between the method load and the
call. The receiver must still be bound when the frame resumes, for plain
method loads and for zero-argument `super()` method loads.
"""

import asyncio


class Sink:
    def __init__(self):
        self.seen = []
        self.tag = "p"

    def put(self, value):
        self.seen.append(value)

    def tagged(self, value):
        return (self.tag, value)


def collector(sink):
    while True:
        sink.put((yield))


sink = Sink()
gen = collector(sink)
next(gen)
gen.send(1)
gen.send(2)
print(sink.seen)


def delegate(sink):
    sink.put((yield from range(2)))
    sink.put((yield from inner()))


def inner():
    yield "inner"
    return "returned"


sink = Sink()
print(list(delegate(sink)), sink.seen)


async def agen():
    for i in range(3):
        await asyncio.sleep(0)
        yield i


async def main(sink):
    return sink.tagged([x async for x in agen()])


print(asyncio.run(main(Sink())))


async def main_set_and_dict(sink):
    values = sink.tagged({x async for x in agen()})
    mapping = sink.tagged({x: x * x async for x in agen()})
    return values, mapping


print(asyncio.run(main_set_and_dict(Sink())))


class Base:
    def m(self, value):
        return ("Base", value)


class Child(Base):
    def m(self, value):
        return super().m(value)

    def gen(self):
        result = super().m((yield "ready"))
        yield result


child = Child()
steps = child.gen()
print(next(steps))
print(steps.send("sent"))
