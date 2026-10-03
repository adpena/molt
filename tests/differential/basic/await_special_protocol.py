"""Await slot acquisition, native code flags, and ABC inspection stay distinct."""
import collections.abc
import inspect
import types

events = []


def finish(coroutine):
    try:
        coroutine.send(None)
    except StopIteration as error:
        return error.value
    raise AssertionError("fixture unexpectedly suspended")


async def use(value):
    return await value


def result():
    if False:
        yield None
    return 42


class Descriptor:
    def __get__(self, instance, owner):
        events.append("bind")
        return result


class Awaitable:
    __await__ = Descriptor()

    def __getattribute__(self, name):
        if name == "__await__":
            raise AssertionError("instance hook was called")
        return object.__getattribute__(self, name)


value = Awaitable()
value.__await__ = lambda: iter(())
print("inspect-slot", inspect.isawaitable(value), events)
print("await-slot", finish(use(value)), events)


class InstanceOnly:
    pass


instance = InstanceOnly()
instance.__await__ = result
print("inspect-instance", inspect.isawaitable(instance))
try:
    finish(use(instance))
except TypeError:
    print("await-instance", "rejected")


class SpoofedCode:
    @property
    def gi_code(self):
        raise AssertionError("gi_code inspected on a non-generator")


print("inspect-code", inspect.isawaitable(SpoofedCode()))


class MissingSlot:
    __await__ = None


print("inspect-none", inspect.isawaitable(MissingSlot()))


class Virtual:
    pass


collections.abc.Awaitable.register(Virtual)
print("inspect-registered", inspect.isawaitable(Virtual()))
try:
    finish(use(Virtual()))
except TypeError:
    print("await-registered", "rejected")


plain = result()
print("inspect-generator", inspect.isawaitable(plain))
try:
    finish(use(plain))
except TypeError:
    print("await-generator", "rejected")
plain.close()


@types.coroutine
def flagged():
    if False:
        yield None
    return 43


flagged_value = flagged()
print("inspect-flagged", inspect.isawaitable(flagged_value))
print("await-flagged", finish(use(flagged_value)))


async def native():
    return 44


native_value = native()
print("inspect-native", inspect.isawaitable(native_value))
print("await-native", finish(use(native_value)))


class Returned:
    def __init__(self, value):
        self.value = value

    def __await__(self):
        return self.value


for bad in ([], 42, native(), flagged()):
    try:
        finish(use(Returned(bad)))
    except TypeError as error:
        print("bad-result", str(error))
    finally:
        if hasattr(bad, "close"):
            bad.close()


class FailingDescriptor:
    def __get__(self, instance, owner):
        raise ValueError("descriptor")


class Broken:
    __await__ = FailingDescriptor()


print("inspect-broken", inspect.isawaitable(Broken()))
try:
    finish(use(Broken()))
except (AttributeError, ValueError) as error:
    print("descriptor-error", type(error).__name__, str(error), error.__context__)


class Steps:
    def __await__(self):
        try:
            received = yield "pause"
            return received
        finally:
            events.append("step-finally")


coroutine = use(Steps())
print("send-yield", coroutine.send(None))
try:
    coroutine.send(19)
except StopIteration as error:
    print("send-result", error.value, events[-1])


class ThrowSteps:
    def __await__(self):
        try:
            yield "throw-pause"
        except ValueError as error:
            return error
        finally:
            events.append("throw-finally")


failure = ValueError("thrown")
coroutine = use(ThrowSteps())
print("throw-yield", coroutine.send(None))
try:
    coroutine.throw(failure)
except StopIteration as error:
    print("throw-result", error.value is failure, events[-1])


coroutine = use(Steps())
print("close-yield", coroutine.send(None))
coroutine.close()
print("closed", events[-1], coroutine.cr_frame is None)
try:
    coroutine.send(None)
except RuntimeError as error:
    print("reuse-rejected", str(error))


async def wrapper_value():
    return 23


wrapped = wrapper_value()
iterator = wrapped.__await__()
print("wrapper-iterator", iter(iterator) is iterator)
try:
    next(iterator)
except StopIteration as error:
    print("wrapper-result", error.value)


import sys


def nested_callback():
    active = sys.exception()
    try:
        raise LookupError("callback")
    except LookupError as nested:
        assert sys.exception() is nested
    assert sys.exception() is active
    return "callback-restored"


async def exception_context():
    original = KeyError("outer")
    try:
        raise original
    except KeyError:
        before = sys.exception() is original
        await Steps()
        after = sys.exception() is original
        callback = nested_callback()
        return before, after, callback


context_coroutine = exception_context()
print("context-yield", context_coroutine.send(None), sys.exception() is None)
try:
    context_coroutine.send(None)
except StopIteration as completed:
    print("context-result", completed.value)
print("caller-restored", sys.exception() is None)


inner = use(Steps())
print("reentry-inner-yield", inner.send(None))


class ReentrantSend:
    def __get__(self, instance, owner):
        try:
            inner.send(31)
        except StopIteration as result:
            nested_result = result.value
        return lambda received: instance.complete(received, nested_result)


class ReentrantIterator:
    def __iter__(self):
        return self

    def __next__(self):
        return "outer-pause"

    def complete(self, received, nested):
        raise StopIteration((received, nested))

    send = ReentrantSend()


reentrant = use(Returned(ReentrantIterator()))
print("reentry-outer-yield", reentrant.send(None))
try:
    reentrant.send(17)
except StopIteration as result:
    print("reentry-result", result.value)


invalid_throw = use(Steps())
try:
    invalid_throw.throw(42)
except TypeError:
    print("invalid-throw-unstarted", inspect.getcoroutinestate(invalid_throw))
print("after-invalid-throw", invalid_throw.send(None))
try:
    invalid_throw.throw(42)
except TypeError:
    print("invalid-throw-suspended", inspect.getcoroutinestate(invalid_throw))
try:
    invalid_throw.send(29)
except RuntimeError as error:
    print("after-invalid-reuse", str(error))


async def catch_invalid_throw():
    try:
        await Steps()
    except TypeError:
        return "caught-invalid"


caught = catch_invalid_throw()
print("caught-invalid-yield", caught.send(None))
try:
    caught.throw(42)
except StopIteration as result:
    print("caught-invalid-result", result.value)


class Captured:
    def __init__(self, label):
        self.label = label

    def __del__(self):
        events.append(self.label)


async def never_started(capture):
    raise AssertionError("closed coroutine entered its body")


created = never_started(Captured("closed-capture"))
code = created.cr_code
created.close()
print("closed-created", events[-1], created.cr_frame is None, created.cr_code is code)

created = never_started(Captured("thrown-capture"))
code = created.cr_code
failure = LookupError("before-start")
try:
    created.throw(failure)
except LookupError as error:
    print("thrown-created", error is failure, events[-1], created.cr_frame is None, created.cr_code is code)


async def return_from_close():
    try:
        await Steps()
    except GeneratorExit:
        return 53


closing = return_from_close()
closing.send(None)
closed_result = closing.close()
assert closed_result == (53 if sys.version_info >= (3, 13) else None)
assert closing.close() is None
print("coroutine-close-return", closed_result)

failure = LookupError("manual-call")


async def fail_after_resume():
    await Steps()
    raise failure


failed = fail_after_resume()
failed.send(None)
try:
    failed.send(None)
except LookupError as error:
    assert error is failure
    assert inspect.getcoroutinestate(failed) == inspect.CORO_CLOSED
    print("manual-error-identity", True)
else:
    raise AssertionError("manual resume lost the original exception")
