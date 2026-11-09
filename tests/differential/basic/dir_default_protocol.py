# The default object.__dir__ / type.__dir__ protocol: generic lookups of
# __dict__, __class__ and __bases__ (descriptors run), merged as CPython's
# _dir_object, _dir_type and merge_class_dict merge them.


def show(label, names):
    # Class-namespace dunder contents are tracked by HF-815; this file pins the
    # merge protocol itself, so it compares the non-dunder names and the order.
    names = list(names)
    own = [name for name in names if not (name.startswith("__") and name.endswith("__"))]
    print(label, own, names == sorted(names))


class Base:
    base_attr = 1

    def base_method(self):
        return 1


class Left(Base):
    left_attr = 2


class Right(Base):
    right_attr = 3


class Diamond(Left, Right):
    def __init__(self):
        self.a = 1
        self.b = 2


item = Diamond()
show("instance", dir(item))
item.c = 3
del item.a
show("mutated", dir(item))
show("type", dir(Diamond))


class Meta(type):
    meta_attr = 4


class WithMeta(metaclass=Meta):
    own = 5


show("metaclass excluded", dir(WithMeta))
print("meta_attr" in dir(WithMeta), "meta_attr" in dir(Meta))


class NotADict:
    x = 1

    @property
    def __dict__(self):
        return [("hidden", 1)]


show("non-dict __dict__", dir(NotADict()))


class Other:
    other_attr = 6


class Masked:
    masked_attr = 7

    @property
    def __class__(self):
        return Other


show("__class__ property", dir(Masked()))


class BasesMeta(type):
    @property
    def __bases__(cls):
        return [Other]


class ListBases(metaclass=BasesMeta):
    listed = 8


show("list __bases__", dir(ListBases))


class Slotted:
    __slots__ = ("p", "q")

    def __init__(self):
        self.p = 1


show("slots", dir(Slotted()))


class Raising:
    @property
    def __dict__(self):
        raise ValueError("no namespace")


try:
    dir(Raising())
except ValueError as error:
    print("raised", type(error).__name__, error)
