"""Purpose: differential coverage for inspect.isgenerator.

Only a generator object is a generator. A generator function, a coroutine, an
async generator, its awaitables, an iterator and an object that only has the
generator methods are not.
"""

import inspect
import types


def numbers():
    yield 1


async def job():
    return 1


async def ticks():
    yield 1


class LooksLikeGenerator:
    def __iter__(self):
        return self

    def __next__(self):
        raise StopIteration

    def send(self, value):
        raise StopIteration

    def throw(self, *args):
        raise StopIteration

    def close(self):
        pass


gen = numbers()
coro = job()
agen = ticks()
asend = agen.__anext__()
cases = [
    ("generator", gen),
    ("generator-expression", (x for x in range(2))),
    ("generator-function", numbers),
    ("coroutine", coro),
    ("async-generator", agen),
    ("asend", asend),
    ("list-iterator", iter([1])),
    ("duck", LooksLikeGenerator()),
    ("none", None),
]
for label, value in cases:
    print(label, inspect.isgenerator(value), isinstance(value, types.GeneratorType))

next(gen)
print("running-done", inspect.isgenerator(gen))
gen.close()
print("closed", inspect.isgenerator(gen))
print("exported", "isgenerator" in inspect.__all__)
asend.close()
coro.close()
