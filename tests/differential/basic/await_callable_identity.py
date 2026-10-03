"""Await preserves callable lookup, argument order, and async protocol identity."""
import builtins
from await_callable_identity_pkg import completed as anext

events = []


def finish(coroutine):
    try:
        coroutine.send(None)
    except builtins.StopIteration as error:
        return error.value
    raise AssertionError("fixture unexpectedly suspended")


async def imported():
    return await anext(value="imported", extra=2)


print("imported", finish(imported()))


async def replacement(*args, **kwargs):
    events.append("replacement")
    return args, kwargs


async def local(anext):
    return await anext(value="local", extra=3)


print("local", finish(local(replacement)))


async def shadow():
    anext = replacement
    return await anext(*("star",), **{"value": "shadow"})


print("shadow", finish(shadow()))


async def original(value):
    events.append("original")
    return value


anext = original


def rebind():
    global anext
    events.append("argument")
    anext = replacement
    return "captured"


async def rebound():
    first = await anext(rebind())
    second = await anext(value="later")
    return first, second


print("rebound", finish(rebound()))
print("order", events)


class Callable:
    def __call__(self, **kwargs):
        events.append("type-call")
        return replacement(**kwargs)


callable_value = Callable()
callable_value.__call__ = lambda **kwargs: "wrong-instance-call"
print("callable", finish(local(callable_value)))
try:
    finish(local(42))
except builtins.TypeError:
    print("noncallable", True)


class RealStop(builtins.StopAsyncIteration):
    pass


class FakeStop(builtins.Exception):
    pass


FakeStop.__name__ = "StopAsyncIteration"
FakeStop.__qualname__ = "StopAsyncIteration"


class Values:
    def __init__(self, stop):
        self.stop = stop

    def __aiter__(self):
        return self

    async def __anext__(self):
        raise self.stop("end")


async def consume(values):
    async for value in values:
        raise AssertionError("unexpected value")
    return "complete"


class ImmediateValues(Values):
    def __anext__(self):
        raise self.stop("end")


for factory in (Values, ImmediateValues):
    print("real-stop", finish(consume(factory(RealStop))))
    try:
        finish(consume(factory(FakeStop)))
    except FakeStop:
        print("fake-stop", "propagated")


async def comprehension(values):
    return [value async for value in values]


async def literal_list():
    async for value in [1]:
        pass


async def literal_iterator():
    async for value in iter([1]):
        pass


async def literal_comprehension():
    return [value async for value in [1]]


for operation in (literal_list, literal_iterator, literal_comprehension):
    try:
        finish(operation())
    except builtins.TypeError:
        print("sync-rejected", True)


try:
    aiter([1])
except builtins.TypeError:
    print("sync-acquisition-rejected", True)


class Acquisitions:
    def __aiter__(self):
        events.append("aiter")
        return Values(RealStop)


first = builtins.aiter(Acquisitions())
print("protocol", builtins.aiter(first) is first)


def local_handler(ValueError, error):
    try:
        raise error
    except ValueError:
        return "caught"


print("handler", local_handler(builtins.TypeError, builtins.TypeError("local")))
ValueError = builtins.TypeError
try:
    raise builtins.TypeError("module")
except ValueError:
    print("module-handler", "caught")


class HandlerMeta(type):
    def __instancecheck__(cls, instance):
        raise AssertionError("exception matching invoked __instancecheck__")


class Handler(builtins.Exception, metaclass=HandlerMeta):
    pass


print("handler-meta", local_handler(Handler, Handler("value")))
for target in (
    int, 42, (builtins.TypeError, int), (int, builtins.TypeError),
    ((builtins.TypeError,),),
):
    try:
        local_handler(target, builtins.TypeError("value"))
    except builtins.TypeError as error:
        print("invalid-handler", str(error))
