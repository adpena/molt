"""Constructor cells and callbacks share one native/WASM semantic authority."""

events = []


class Descriptor:
    def __set_name__(self, owner, name):
        events.append(("set", name, __class__.__name__))
        if name == "first":
            del owner.second


class Left:
    def __init_subclass__(cls, **kwargs):
        events.append(("left", cls.__name__, tuple(kwargs)))
        super().__init_subclass__()


class Right:
    def __init_subclass__(cls, **kwargs):
        events.append(("right", cls.__name__, tuple(kwargs)))
        super().__init_subclass__()


class Subject(Left, Right, alpha=1, beta=2):
    first = Descriptor()
    second = Descriptor()
    marker = int
    type Alias = marker

    def defining_class(self):
        return __class__


print("callbacks", events)
print("classcell", Subject().defining_class() is Subject)
Subject.marker = str
print("namespace", Subject.Alias.__value__ is str)
print("hidden", "__classcell__" in vars(Subject), "__classdictcell__" in vars(Subject))


class CopyMeta(type):
    def __new__(mcls, name, bases, namespace):
        copied = dict(namespace)
        namespace.pop("__classcell__", None)
        return super().__new__(mcls, name, bases, copied)


class Copied(metaclass=CopyMeta):
    def defining_class(self):
        return __class__


print("copied", Copied().defining_class() is Copied)


class MissingMeta(type):
    def __new__(mcls, name, bases, namespace):
        namespace.pop("__classcell__", None)
        return super().__new__(mcls, name, bases, namespace)


try:

    class Missing(metaclass=MissingMeta):
        def defining_class(self):
            return __class__
except Exception as error:
    print("missing", type(error).__name__)


class Other:
    pass


class WrongMeta(type):
    def __new__(mcls, name, bases, namespace):
        super().__new__(mcls, name, bases, namespace)
        return Other


try:

    class Wrong(metaclass=WrongMeta):
        def defining_class(self):
            return __class__
except Exception as error:
    print("wrong", type(error).__name__)


class ObjectMeta(type):
    def __new__(mcls, name, bases, namespace):
        return 42


class NonType(metaclass=ObjectMeta):
    def defining_class(self):
        return __class__


print("non-type", NonType)


def mark_decorator(cls):
    events.append(("decorated", cls().defining_class() is cls))
    return cls


@mark_decorator
class Decorated:
    def defining_class(self):
        return __class__


print("decorator", events[-1])


class InstanceOnly:
    def __init__(self):
        self.__set_name__ = lambda owner, name: events.append(
            ("wrong-instance-hook", name)
        )


before = len(events)


class IgnoresInstanceHook:
    value = InstanceOnly()


print("special-lookup", len(events) == before)


class FailingDescriptor:
    def __set_name__(self, owner, name):
        raise ValueError("descriptor failure")


class NeverCalled:
    def __set_name__(self, owner, name):
        events.append(("wrong-later-hook", name))


before = len(events)
try:

    class Fails:
        first = FailingDescriptor()
        second = NeverCalled()
except Exception as error:
    print("failure", type(error).__name__, str(error))
    print("notes", getattr(error, "__notes__", []))
print("stopped", len(events) == before)

for invalid_cell in (False, True):
    namespace = {"__qualname__": 1}
    if invalid_cell:
        namespace["__classcell__"] = 7
    try:
        type("InvalidQualname", (), namespace)
    except Exception as error:
        print("qualname-first", invalid_cell, type(error).__name__, str(error))
