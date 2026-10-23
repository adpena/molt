"""sys.exception() inside a resumed activation finds the caller's innermost
handled exception, as CPython walks the exc_info chain past levels that
handle nothing (a nested try, an async with, a handler's cleanup level)."""

import asyncio
import sys


def name() -> str:
    return type(sys.exception()).__name__


class HandlerManager:
    async def __aenter__(self):
        return self

    async def __aexit__(self, kind, value, traceback):
        print("aexit before await", name())
        await asyncio.sleep(0)
        print("aexit after await", name())


async def child():
    print("child before await", name())
    await asyncio.sleep(0)
    print("child after await", name())


async def own_handler():
    try:
        raise ValueError("own")
    except ValueError:
        await asyncio.sleep(0)
        print("own handler after await", name())
    print("own handler done", name())


async def task_body():
    print("task", name())
    await asyncio.sleep(0)
    print("task resumed", name())


async def in_handler():
    try:
        raise LookupError("outer")
    except LookupError:
        await child()
        async with HandlerManager():
            pass
        try:
            await child()
        finally:
            print("finally in handler", name())
        await own_handler()
        await asyncio.create_task(task_body())
        print("handler end", name())
    print("after handler", name())


def gen_child():
    print("gen child", name())
    yield 1
    print("gen child resumed", name())
    try:
        raise IndexError("inner")
    except IndexError:
        yield 2
        print("gen own handler resumed", name())
    yield 3


def gen_in_handler():
    try:
        raise KeyError("k")
    except KeyError:
        g = gen_child()
        next(g)
        try:
            pass
        finally:
            next(g)
        next(g)
        print("caller between resumes", name())
        print("rest", list(g))
    print("caller after handler", name())


async def main_task():
    print("loop task", name())
    await asyncio.sleep(0)
    print("loop task resumed", name())


asyncio.run(in_handler())
gen_in_handler()
try:
    raise OSError("around the loop")
except OSError:
    asyncio.run(main_task())
print("end", name())
