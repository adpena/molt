"""Purpose: class statements whose bases are plain classes (HF-846 class
facts). A subclass of a plain class keeps metaclass ``type`` and a dict
namespace, so its body runs no Python beyond its own statements, and its
constructor may fold to an inline allocation. Each case checks the order of
observable effects and the constructed object's class against CPython: base
hooks (``__init_subclass__``, a metaclass, ``__set_name__``), a base rebound
before the subclass is created, an inherited ``__init__`` and ``__new__``, and
``__class__`` assignment after construction."""

trace = []


class Base:
    def __init__(self, value):
        self.value = value

    def describe(self):
        return f"{type(self).__name__}({self.value})"


class Mid(Base):
    trace.append("Mid body")

    def describe(self):
        return "mid:" + super().describe()


class Leaf(Mid):
    trace.append("Leaf body")


def build(n):
    return [Leaf(i).describe() for i in range(n)]


print(trace, build(2))


class Hooked:
    def __init_subclass__(cls, **kwargs):
        trace.append(f"init_subclass {cls.__name__} {sorted(kwargs)}")
        super().__init_subclass__()


class Child(Hooked):
    pass


class Flagged(Hooked, flag=1):
    pass


class Meta(type):
    def __call__(cls, *args):
        trace.append(f"Meta call {cls.__name__}")
        return super().__call__(*args)


class WithMeta(metaclass=Meta):
    pass


class UnderMeta(WithMeta):
    pass


def make_under():
    return type(UnderMeta()).__name__


print(trace[-3:], make_under(), trace[-1])


class Named:
    def __set_name__(self, owner, name):
        trace.append(f"set_name {owner.__name__}.{name}")


class Holder(Base):
    attr = Named()


print(trace[-1])


class Original:
    tag = "original"


Replaced = Original


class Original:
    tag = "rebound"


class FromRebound(Replaced):
    pass


print(FromRebound.tag, FromRebound.__mro__[1].tag)


class Singleton:
    _instance = None

    def __new__(cls, *args):
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance


class SubSingleton(Singleton):
    def __init__(self, value):
        self.value = value


def make_singletons():
    first = SubSingleton(1)
    second = SubSingleton(2)
    return first is second, second.value


print("singleton", make_singletons())


class Shape:
    def name(self):
        return "shape"


class Circle(Shape):
    def name(self):
        return "circle"


class Square(Shape):
    def name(self):
        return "square"


def morph():
    shape = Circle()
    names = [shape.name()]
    shape.__class__ = Square
    names.append(shape.name())
    return names


print("morph", morph())


def decorate(cls):
    trace.append(f"decorate {cls.__name__}")
    return cls


@decorate
class Decorated(Base):
    pass


print(trace[-1], Decorated(3).describe())
