"""Saved state follows visible slots, not hidden physical base storage."""


class Base:
    __slots__ = ("x",)


class Child(Base):
    __slots__ = ("x", "y")


obj = Child()
Base.x.__set__(obj, "hidden")
print("hidden-only", object.__getstate__(obj))
obj.x = "visible"
obj.y = "second"
print("visible-state", object.__getstate__(obj))
print("both-owners", Base.x.__get__(obj), Child.x.__get__(obj))
del obj.x
print("visible-missing", object.__getstate__(obj))
print("base-retained", Base.x.__get__(obj))

events = []


class Observed(Base):
    __slots__ = ("x",)

    def __getattribute__(self, name):
        if name == "x":
            events.append("read-x")
            return len(events)
        return object.__getattribute__(self, name)


print("duplicate-slot-reads", object.__getstate__(Observed()), events)


class Shadow(Base):
    __slots__ = ()
    x = "class-value"


print("class-shadow", object.__getstate__(Shadow()))


class Missing(Base):
    __slots__ = ()

    def __getattribute__(self, name):
        if name == "x":
            raise AttributeError("absent")
        return object.__getattribute__(self, name)


print("missing-getter", object.__getstate__(Missing()))


class Broken(Base):
    __slots__ = ()

    def __getattribute__(self, name):
        if name == "x":
            raise RuntimeError("slot getter failed")
        return object.__getattribute__(self, name)


try:
    object.__getstate__(Broken())
except RuntimeError as error:
    print("getter-error", type(error).__name__, str(error))
