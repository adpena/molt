"""Purpose: differential coverage for types advanced helpers."""

import asyncio
import sys
import types


def foo(x):
    return x + 1


def gen():
    yield 1


async def coro():
    return 1


async def agen():
    yield 1


print(isinstance(foo, types.FunctionType))
print(isinstance(foo.__code__, types.CodeType))
print(isinstance(sys._getframe(), types.FrameType))

mod = types.ModuleType("m")
mod.answer = 42
print(mod.__name__, mod.answer)

try:
    1 / 0
except Exception as exc:
    tb = exc.__traceback__
    print(isinstance(tb, types.TracebackType))

co = coro()
print(isinstance(co, types.CoroutineType))
co.close()

ag = agen()
print(isinstance(ag, types.AsyncGeneratorType))
asyncio.run(ag.aclose())

ns = types.SimpleNamespace(a=1)
print(ns.a)

def demo(self):
    return self.x

class C:
    def __init__(self):
        self.x = 5

obj = C()
method = types.MethodType(demo, obj)
print(method())

@types.coroutine
def legacy():
    yield 1

it = legacy()
print(isinstance(it, types.GeneratorType))
print(next(it))
