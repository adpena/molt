"""Purpose: differential coverage for object dunder builtins."""


def show(label, value):
    print(label, value)


class OverrideGet:
    def __init__(self) -> None:
        self.x = 1

    def __getattribute__(self, name):
        return f"override:{name}"

    def __getattr__(self, name):
        return f"fallback:{name}"


a = OverrideGet()
show("override_attr", a.x)
show("object_getattribute_x", object.__getattribute__(a, "x"))
try:
    object.__getattribute__(a, "missing")
except AttributeError as exc:
    show("object_getattribute_missing", type(exc).__name__)


class OverrideSet:
    def __setattr__(self, name, value):
        raise RuntimeError("blocked")


s = OverrideSet()
object.__setattr__(s, "x", 5)
show("object_setattr_x", s.x)
try:
    object.__setattr__(s, 1, 2)
except TypeError as exc:
    show("object_setattr_name_error", str(exc))


class OverrideDel:
    def __init__(self) -> None:
        self.x = 9

    def __delattr__(self, name):
        raise RuntimeError("blocked")


d = OverrideDel()
object.__delattr__(d, "x")
show("object_delattr_has", hasattr(d, "x"))
try:
    object.__delattr__(d, 1)
except TypeError as exc:
    show("object_delattr_name_error", str(exc))


class Desc:
    def __get__(self, obj, objtype=None):
        return obj._x

    def __set__(self, obj, val) -> None:
        obj._x = val + 1

    def __delete__(self, obj) -> None:
        obj._x = -1


class Prop:
    x = Desc()

    def __init__(self) -> None:
        self._x = 0


p = Prop()
object.__setattr__(p, "x", 4)
show("object_setattr_prop", p._x)
show("object_getattribute_prop", object.__getattribute__(p, "x"))
object.__delattr__(p, "x")
show("object_delattr_prop", p._x)


class Demo:
    y = 3


show("object_getattribute_class", object.__getattribute__(Demo, "y"))
try:
    object.__setattr__(Demo, "z", 1)
except TypeError as exc:
    show("object_setattr_type_error", type(exc).__name__)
try:
    object.__delattr__(Demo, "y")
except TypeError as exc:
    show("object_delattr_type_error", type(exc).__name__)
