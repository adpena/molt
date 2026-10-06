"""Async protocol lookup bypasses instances and binds descriptors once."""
import sys


def finish(coroutine):
    try:
        coroutine.send(None)
    except StopIteration as error:
        return error.value
    raise AssertionError("unexpected suspension")


async def completed(value):
    return value


class Values:
    def __aiter__(self):
        return self

    def __anext__(self):
        return completed("type")


values = Values()
values.__aiter__ = lambda: "instance"
values.__anext__ = lambda: completed("instance")
print("instance-ignored", aiter(values) is values, finish(anext(values)))


class InstanceOnly:
    pass


instance = InstanceOnly()
instance.__aiter__ = lambda: values
instance.__anext__ = lambda: completed("instance")
for operation in (aiter, anext):
    try:
        operation(instance)
    except TypeError:
        print("instance-only-rejected", True)


class RaisingDescriptor:
    def __init__(self, error):
        self.error = error

    def __get__(self, obj, owner):
        raise self.error


for exception_type in (ValueError, AttributeError):
    failure = exception_type("descriptor")

    class BrokenAiter:
        __aiter__ = RaisingDescriptor(failure)

    class BrokenAnext:
        def __aiter__(self):
            return self
        __anext__ = RaisingDescriptor(failure)

    iterator = BrokenAnext()
    print("admission-does-not-bind", aiter(iterator) is iterator)
    for operation, value in ((aiter, BrokenAiter()), (anext, iterator)):
        try:
            operation(value)
        except Exception as caught:
            if sys.version_info >= (3, 14):
                assert caught is failure
            else:
                assert type(caught) is AttributeError
                assert str(caught) == "object " + type(value).__name__ + " does not have __" + operation.__name__ + "__ method"
                assert caught.__context__ is None
            print("descriptor-contract", True)


calls = []


class NextDescriptor:
    def __get__(self, obj, owner):
        calls.append("next")
        return lambda: completed("bound")


class Described:
    def __aiter__(self):
        return self
    __anext__ = NextDescriptor()


value = Described()
print("no-eager-bind", aiter(value) is value, calls)
print("single-bind", finish(anext(value)), calls)


class BadResult:
    def __aiter__(self):
        return object()


try:
    aiter(BadResult())
except TypeError:
    print("bad-result-rejected", True)


class CustomLookup(Values):
    def __getattribute__(self, name):
        if name in ("__aiter__", "__anext__"):
            raise AssertionError("ordinary instance lookup")
        return object.__getattribute__(self, name)


custom = CustomLookup()
print("lookup-bypassed", aiter(custom) is custom, finish(anext(custom)))
