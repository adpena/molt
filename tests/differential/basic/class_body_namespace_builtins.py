"""Class namespace reflection observes one live mapping, independent of globals."""

module_only = 100


class Plain:
    """A class docstring is a namespace binding too."""
    marker = 42
    view = locals()
    same = vars() is view
    keys = dir()
    view["written"] = 43
    copied = written
    later = 44
    del marker


print(Plain.same, Plain.copied, Plain.view["later"])
print("marker" in Plain.view, "module_only" in Plain.view)
print("marker" in Plain.keys, "module_only" in Plain.keys)
print("__module__" in Plain.view, "__qualname__" in Plain.view)
print(Plain.view["__doc__"] == Plain.__doc__)


def decorator(cls):
    print(cls.view["value"], "module_only" in cls.view)
    return cls


@decorator
class Decorated:
    value = 51
    view = vars()


class Outer:
    outer_only = 61

    class Inner:
        inner_only = 62
        view = locals()

    view = locals()


print(Outer.view["outer_only"], Outer.Inner.view["inner_only"])
print("outer_only" in Outer.Inner.view, "Inner" in Outer.view)


def enclosing():
    enclosing_only = 70

    class Local:
        value = 71
        view = locals()
        global_view = globals()

    print(Local.view["value"], "enclosing_only" in Local.view)
    print(Local.global_view["module_only"])
    return enclosing_only


print(enclosing())


prepared_prefix = []


class PreparedMapping:
    def __init__(self):
        self.data = {}

    def __getitem__(self, name):
        return self.data[name]

    def __setitem__(self, name, value):
        if name == "__doc__":
            import sys

            frame = sys._getframe(1)
            prepared_prefix.append(
                ("__annotations__" in self.data, frame.f_code.co_name, frame.f_lineno)
            )
        self.data[name] = value

    def keys(self):
        return self.data.keys()


class PreparedMeta(type):
    @classmethod
    def __prepare__(mcls, name, bases):
        return PreparedMapping()

    def __new__(mcls, name, bases, namespace):
        print(namespace["view"] is namespace, namespace["same"])
        print("mark" in namespace["names"], "module_only" in namespace["names"])
        return super().__new__(mcls, name, bases, dict(namespace))


class Prepared(metaclass=PreparedMeta):
    """Prepared namespace docstring."""
    mark: int = 91
    view = locals()
    same = vars() is view
    names = dir()
    view["written"] = 92
    copied = written


print(Prepared.copied)
print(prepared_prefix)


class MappedSlots:
    namespace = locals()
    namespace.update({"__slots__": ("value",)})

    def __init__(self):
        self.value = 93


slotted = MappedSlots()
print(slotted.value, hasattr(slotted, "__dict__"))


class RichBoolean:
    def __gt__(self, other):
        return False

    def __lt__(self, other):
        return True


boxed_false = RichBoolean() > 2
boxed_true = RichBoolean() < 2
print(boxed_false is False, False is boxed_false, boxed_false is not True)
print(boxed_true is True, True is boxed_true, boxed_true is not False)


class DeferredNamespace:
    marker = 94
    type Alias = locals()


print("marker" in DeferredNamespace.Alias.__value__)


class Annotations:
    before = __annotations__
    value: int = 1
    after = __annotations__


print(Annotations.before is Annotations.after, Annotations.before["value"] is int)


class CellBase:
    def identity(self):
        return 95


class CellClass(CellBase):
    namespace = locals()

    def identity(self):
        return super().identity(), __class__ is CellClass


print(CellClass().identity())
