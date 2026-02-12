"""Purpose: differential coverage for abc/_abc intrinsic-backed behavior."""

import abc


class Base(abc.ABC):
    @abc.abstractmethod
    def run(self):
        raise NotImplementedError


class Child(Base):
    pass


class Impl(Child):
    def run(self):
        return "ok"


def instantiate_error(cls):
    try:
        cls()
    except TypeError as exc:
        return type(exc).__name__
    return "ok"


print("base-inst", instantiate_error(Base))
print("child-inst", instantiate_error(Child))
print("impl-run-abstract", getattr(Impl.run, "__isabstractmethod__", False))
print("impl-abstracts", sorted(Impl.__abstractmethods__))
print("impl-inst", instantiate_error(Impl))
print("base-abstracts", sorted(Base.__abstractmethods__))
print("child-abstracts", sorted(Child.__abstractmethods__))
print("base-dict-has", "__abstractmethods__" in Base.__dict__)
print("base-abstracts-bool", bool(Base.__abstractmethods__))


class Proto(abc.ABC):
    pass


class Concrete:
    pass


Proto.register(Concrete)
print("issub", issubclass(Concrete, Proto))
print("isinst", isinstance(Concrete(), Proto))


class Hooked(abc.ABC):
    @classmethod
    def __subclasshook__(cls, candidate):
        if any("marker" in base.__dict__ for base in candidate.__mro__):
            return True
        return NotImplemented


class Marked:
    marker = True


print("hook-sub", issubclass(Marked, Hooked))
print("hook-inst", isinstance(Marked(), Hooked))
print("token-int", isinstance(abc.get_cache_token(), int))
