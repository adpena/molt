"""Purpose: differential coverage for the async generator awaitable classes.

agen.__anext__() and agen.asend() return async_generator_asend objects;
agen.athrow() and agen.aclose() return async_generator_athrow objects. Each is
an awaitable iterator with send, throw and close, so inspect, the
collections.abc protocols and asyncio accept it. asyncio.run must close an
async generator that the program leaves open.
"""

import asyncio
import collections.abc
import inspect

PROTOCOL = ("__await__", "__iter__", "__next__", "send", "throw", "close")


class Tick:
    """An awaitable that yields its label once to whoever drives it."""

    def __init__(self, label):
        self.label = label

    def __await__(self):
        received = yield self.label
        return (self.label, received)


def outcome(step):
    try:
        value = step()
    except StopIteration as exc:
        return ("stop", exc.value)
    except StopAsyncIteration:
        return ("stop-async",)
    except (RuntimeError, TypeError, ValueError, KeyError) as exc:
        return (type(exc).__name__, str(exc))
    return ("yield", value)


async def numbers():
    yield 1
    yield 2


def describe(label, awaitable):
    kind = type(awaitable)
    print(label, kind.__name__, kind.__qualname__, kind.__module__, repr(kind))
    print(
        "  await",
        hasattr(awaitable, "__await__"),
        awaitable.__await__() is awaitable,
        iter(awaitable) is awaitable,
    )
    print(
        "  abc",
        inspect.isawaitable(awaitable),
        isinstance(awaitable, collections.abc.Awaitable),
        isinstance(awaitable, collections.abc.Coroutine),
        isinstance(awaitable, collections.abc.Generator),
        isinstance(awaitable, collections.abc.Iterator),
    )
    print("  inspect", inspect.iscoroutine(awaitable), inspect.isgenerator(awaitable))
    print("  methods", [name for name in PROTOCOL if name in vars(kind)])
    try:
        kind()
    except TypeError as exc:
        print("  new", exc)
    try:

        class Derived(kind):
            pass

    except TypeError as exc:
        print("  subclass", exc)


# Each awaitable comes from a fresh generator and is driven to its end.
first = numbers()
anext_step = first.__anext__()
describe("anext", anext_step)
print("  run", outcome(lambda: anext_step.send(None)))
asend_step = numbers().asend(None)
describe("asend", asend_step)
print("  run", outcome(lambda: asend_step.send(None)))
athrow_step = numbers().athrow(KeyError("thrown"))
describe("athrow", athrow_step)
print("  run", outcome(lambda: athrow_step.send(None)))
aclose_step = numbers().aclose()
describe("aclose", aclose_step)
print("  run", outcome(lambda: aclose_step.send(None)))
print("same class", type(first.asend(None)) is type(anext_step))
print("same class", type(first.aclose()) is type(athrow_step))


async def ticking():
    try:
        got = await Tick("t1")
        print("  body got", got)
        sent = yield "first"
        print("  body sent", sent)
        try:
            await Tick("t2")
        except ValueError as exc:
            print("  body caught", exc)
            yield "recovered"
        yield "last"
    finally:
        print("  body finally")


# Manual driving: an await-level yield comes back from send(); an async-level
# yield ends the step with StopIteration.
gen = ticking()
step = gen.asend(None)
print("send1", outcome(lambda: step.send(None)))
print("send2", outcome(lambda: step.send("x")))
print("send3", outcome(lambda: step.send(None)))
step = gen.asend("hello")
print("asend1", outcome(lambda: step.send(None)))
print("asend2", outcome(lambda: step.throw(ValueError("boom"))))
print("asend3", outcome(lambda: step.send(None)))
step = gen.__anext__()
print("next1", outcome(lambda: next(step)))
print("next-close", step.close())
closer = gen.aclose()
print("aclose1", outcome(lambda: closer.send(None)))
print("aclose2", outcome(lambda: closer.send(None)))
print("closed", outcome(lambda: gen.__anext__().send(None)))

# First-step rules.
print("nonnone", outcome(lambda: numbers().asend(5).send(None)))
gen = numbers()
step = gen.__anext__()
print("throw-first", outcome(lambda: step.throw(KeyError("k"))))
print("throw-reuse", outcome(lambda: step.send(None)))
print("throw-closed", outcome(lambda: gen.__anext__().send(None)))
print("athrow-first", outcome(lambda: numbers().athrow(KeyError("k")).send(None)))


async def catching():
    try:
        yield 1
    except ValueError:
        yield "caught"


gen = catching()
print("catch1", outcome(lambda: gen.__anext__().send(None)))
thrower = gen.athrow(ValueError("v"))
print("athrow", outcome(lambda: thrower.send(None)))
print("athrow-reuse", outcome(lambda: thrower.send(None)))
print("athrow-ended", outcome(lambda: gen.athrow(ValueError("w")).send(None)))


async def returning():
    try:
        yield 1
    except ValueError:
        return


gen = returning()
print("return1", outcome(lambda: gen.__anext__().send(None)))
print("athrow-return", outcome(lambda: gen.athrow(ValueError).send(None)))


async def reuse():
    gen = numbers()
    step = gen.__anext__()
    print("await", await step)
    try:
        await step
    except RuntimeError as exc:
        print("await-reuse", exc)
    print("asend", await gen.asend(None))
    try:
        await gen.athrow(ValueError("end"))
    except ValueError as exc:
        print("athrow raised", exc)
    print("aclose", await gen.aclose())


asyncio.run(reuse())


async def scheduled():
    gens = [numbers(), numbers()]
    for item in gens:
        await item.__anext__()
    print("gather", await asyncio.gather(*(item.aclose() for item in gens)))
    print("ensure_future", await asyncio.ensure_future(numbers().aclose()))
    print("wait_for", await asyncio.wait_for(numbers().__anext__(), 1))


asyncio.run(scheduled())


async def ticker(log):
    try:
        count = 0
        while True:
            yield count
            count += 1
            await asyncio.sleep(0)
    finally:
        log.append("ticker closed")


async def leave_open(log, holder):
    gen = ticker(log)
    print("ticker", await gen.__anext__(), await gen.__anext__())
    # The generator stays open; asyncio.run's shutdown_asyncgens closes it.
    holder.append(gen)


log = []
holder = []
asyncio.run(leave_open(log, holder))
print("teardown", log)
