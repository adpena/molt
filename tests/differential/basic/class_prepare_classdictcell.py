"""From 3.14 a class body that owns an annotation scope (a def or an annotated
assignment, through compound statements but not nested scopes) stores
__classdictcell__ in its __prepare__ namespace, before __classcell__; type
creation pops it, so the class __dict__ never holds it."""

events = []


class LogDict(dict):
    def __setitem__(self, key, value):
        events.append(key)
        super().__setitem__(key, value)


class Meta(type):
    @classmethod
    def __prepare__(mcls, name, bases, **kwargs):
        return LogDict()


def tail(cls):
    names = [name for name in events if name.startswith("__") and name not in ("__module__", "__qualname__", "__firstlineno__")]
    print(cls.__name__, names, "__classdictcell__" in vars(cls), "__classcell__" in vars(cls))
    events.clear()


class Attr(metaclass=Meta):
    a = 1


tail(Attr)


class Method(metaclass=Meta):
    def f(self):
        return 1


tail(Method)


class Cell(metaclass=Meta):
    def f(self):
        return __class__


tail(Cell)


class Lambda(metaclass=Meta):
    f = lambda self: 1  # noqa: E731


tail(Lambda)


class Annotated(metaclass=Meta):
    x: int


tail(Annotated)


class Nested(metaclass=Meta):
    class Inner:
        def f(self):
            return 1


tail(Nested)


class Branch(metaclass=Meta):
    if len(events) >= 0:
        def f(self):
            return 1
    try:
        pass
    except Exception:
        async def g(self):
            return 1


tail(Branch)


class Comprehension(metaclass=Meta):
    xs = [i for i in range(2)]


tail(Comprehension)
print("cell value", Cell().f() is Cell)
