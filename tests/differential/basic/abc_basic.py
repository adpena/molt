"""Purpose: differential coverage for abc basics."""

import abc


class Base(metaclass=abc.ABCMeta):
    @abc.abstractmethod
    def run(self):
        raise NotImplementedError

    @property
    @abc.abstractmethod
    def value(self):
        raise NotImplementedError


class Impl(Base):
    def run(self):
        return "ok"

    @property
    def value(self):
        return 42


print(sorted(Base.__abstractmethods__))
print(sorted(Impl.__abstractmethods__))
inst = Impl()
print(inst.run(), inst.value)
print(isinstance(inst, Base))
print(issubclass(Impl, Base))
print(isinstance(abc.get_cache_token(), int))
