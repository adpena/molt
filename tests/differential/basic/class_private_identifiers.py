"""Class-private lexical identities, metadata, closures and suspension."""
from __future__ import annotations


class Base:
    __slots__ = ("__value",)

    def __init__(self, value):
        self.__value = value

    def __read(self, __increment=0):
        return self.__value + __increment

    def read(self):
        return self.__read(2)

    def parameters(self):
        try:
            self.__read(__increment=1)
        except TypeError:
            print("private parameter rejects raw keyword")
        return self.__read(_Base__increment=3)

    def closure(self):
        __local = self.__value

        def __nested():
            nonlocal __local
            __local += 1
            return __local

        return __nested

    def __generate(self):
        yield self.__value

    def generated(self):
        return self.__generate()

    async def __async(self):
        return self.__value

    def coroutine(self):
        return self.__async()

    def annotated(self, __parameter: __Type) -> __Type:
        return __parameter


class Derived(Base):
    __slots__ = ("__value",)

    def __init__(self):
        Base.__init__(self, 7)
        self.__value = 11

    def own(self):
        return self.__value


value = Derived()
print("distinct private owners", value.read(), value.own())
print("private parameter", value.parameters())
closed = value.closure()
print("private closure", closed(), closed(), closed.__name__, closed.__qualname__)
print("private method metadata", Base._Base__read.__name__, Base._Base__read.__qualname__)
print("private annotations", sorted(Base.annotated.__annotations__.items()))
print("private generator", next(value.generated()))
coroutine = value.coroutine()
try:
    coroutine.send(None)
except StopIteration as stopped:
    print("private coroutine", stopped.value)


class Outer:
    class __Inner:
        __marker = 19

        def read(self):
            return self.__marker


inner = Outer._Outer__Inner()
print("private nested class", type(inner).__name__, type(inner).__qualname__, inner.read())


class GlobalOwner:
    global __published

    def __published():
        def inner():
            return "global closure"
        return inner


print("private global", _GlobalOwner__published.__name__, _GlobalOwner__published.__qualname__)
published_inner = _GlobalOwner__published()
print("private global child", published_inner.__qualname__, published_inner())


class ___:
    def __method(self, __value):
        return __value


print("underscore class", ___().__method(23))
