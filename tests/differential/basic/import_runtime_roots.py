"""Purpose: the runtime imports its roots (copyreg, datetime, asyncio.futures,
_warnings, importlib.machinery) by typed identity, with CPython results."""

import sys
import copy
import pickle
import email.utils
import asyncio
import warnings


class Point:
    def __init__(self, x):
        self.x = x


p = Point(3)
print("reduce_ex", type(p.__reduce_ex__(2)[0]).__name__)
print("copy", copy.copy(p).x, pickle.loads(pickle.dumps(p)).x)
print("parsedate", email.utils.parsedate_to_datetime("Tue, 01 Jan 2030 10:00:00 +0000").year)


async def main():
    loop = asyncio.get_running_loop()
    fut = loop.create_future()
    loop.call_soon(fut.set_result, 7)
    return await fut


print("future", asyncio.run(main()))
saved = sys.meta_path[:]
sys.meta_path.clear()
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    try:
        import colorsys  # noqa: F401
    except ImportError as exc:
        print("empty meta_path", type(exc).__name__)
    print("warned", [w.category.__name__ for w in caught])
sys.meta_path[:] = saved
print("copyreg in modules", "copyreg" in sys.modules)

import importlib.machinery

print("machinery", importlib.machinery.SourceFileLoader.__name__, importlib.machinery.ModuleSpec.__name__)
