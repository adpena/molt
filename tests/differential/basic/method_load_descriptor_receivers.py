"""Purpose: method calls through properties, __getattr__ and __getattribute__ match CPython (HF-216).

Only a plain function found on the type loads without a bound method; every
other attribute takes the generic lookup and is called as an ordinary callable.
"""


class WithProperty:
    @property
    def handler(self):
        return lambda value: ("property", value)


print(WithProperty().handler(1))


class Fallback:
    def real(self, value):
        return ("real", value)

    def __getattr__(self, name):
        return lambda value: ("fallback", name, value)


fallback = Fallback()
print(fallback.real(2))
print(fallback.virtual(3))


class Intercepting:
    def __init__(self):
        self.calls = []

    def target(self, value):
        return ("target", value)

    def __getattribute__(self, name):
        if name != "calls":
            object.__getattribute__(self, "calls").append(name)
        return object.__getattribute__(self, name)


intercepting = Intercepting()
print(intercepting.target(4))
print(intercepting.target(5))
print(intercepting.calls)


class Descriptor:
    def __get__(self, instance, owner):
        return lambda value: ("descriptor", instance is not None, value)


class WithDescriptor:
    act = Descriptor()


print(WithDescriptor().act(6))


class DataDescriptor:
    def __get__(self, instance, owner):
        return lambda value: ("data", value)

    def __set__(self, instance, value):
        raise AttributeError("read-only")


class WithDataDescriptor:
    act = DataDescriptor()


print(WithDataDescriptor().act(7))


class CallableValue:
    def __call__(self, value):
        return ("callable", value)


class WithCallable:
    run = CallableValue()


print(WithCallable().run(8))
