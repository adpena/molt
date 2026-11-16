"""Purpose: zero-argument super() in generator, coroutine and async-generator
frames reads the frame's first parameter and `__class__` cell as they stand,
across suspensions, as CPython's super_init_without_args reads localsplus[0]."""

import asyncio


class Base:
    def name(self):
        return "base:" + type(self).__name__


class Child(Base):
    def gen(self):
        yield super().name()
        yield super().name()

    def gen_rebinds(self, other):
        yield super().name()
        self = other
        yield super().name()

    def gen_deletes(self):
        yield "before"
        del self
        try:
            super()
        except RuntimeError as error:
            yield "deleted: " + str(error)

    def gen_captures(self):
        def peek():
            return self

        yield super().name()
        self = Other()
        yield super().name() + " " + type(peek()).__name__

    async def coro(self):
        first = super().name()
        await asyncio.sleep(0)
        return first + " " + super().name()

    async def agen(self):
        yield super().name()
        await asyncio.sleep(0)
        yield super().name()

    def genexpr(self):
        return list(super().name() for _ in range(2))


class Other(Child):
    pass


def no_arguments():
    yield super()


def main():
    child = Child()
    print(list(child.gen()))
    print(list(child.gen_rebinds(Other())))
    print(list(child.gen_deletes()))
    print(list(child.gen_captures()))
    print(asyncio.run(child.coro()))

    async def drain():
        return [item async for item in child.agen()]

    print(asyncio.run(drain()))
    try:
        print(child.genexpr())
    except TypeError as error:
        print("genexpr", type(error).__name__)
    try:
        next(no_arguments())
    except RuntimeError as error:
        print("no arguments", error)


main()
