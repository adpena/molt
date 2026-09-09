import builtins


class Base:
    def value(self):
        return "base"


class Child(Base):
    def via_alias(self):
        alias = super
        return alias().value()

    def via_builtin(self):
        _ = __class__
        return builtins.super().value()

    def via_callback(self):
        return next(iter(super, None)).value()

    def rebound(self):
        self = None
        value = super()
        return value.__thisclass__.__name__, value.__self__, value.__self_class__

    def deleted(self):
        alias = super
        del self
        return alias()


child = Child()
print(child.via_alias())
print(child.via_builtin())
print(child.via_callback())
print(child.rebound())
try:
    child.deleted()
except BaseException as error:
    print(type(error).__name__, str(error))


class Proxy:
    def __init__(self):
        self.lookups = 0

    @property
    def __class__(self):
        self.lookups += 1
        return Child


proxy = Proxy()
value = super(Child, proxy)
print(value.value())
print(value.__thisclass__.__name__, value.__self_class__.__name__)
print(value.__self__ is proxy, proxy.lookups)
print(value.value(), proxy.lookups)
print(repr(value), proxy.lookups)
print(repr(super(Child)), repr(super(Child, None)))
print(repr(super(Child, Child)), repr(super(object, 3)))


class RejectInstanceCheck(type):
    def __instancecheck__(cls, obj):
        raise AssertionError("super must not call instancecheck")


class Checked(metaclass=RejectInstanceCheck):
    pass


for receiver in (Child, child):
    try:
        super(Checked, receiver)
    except BaseException as error:
        print(type(error).__name__, str(error))


class BrokenProxy:
    @property
    def __class__(self):
        raise ValueError("class trap")


try:
    super(Child, BrokenProxy())
except BaseException as error:
    print(type(error).__name__, str(error))


def bad_cell():
    __class__ = 42

    def function(self):
        alias = super
        return alias()

    return function


try:
    bad_cell()(object())
except BaseException as error:
    print(type(error).__name__, str(error))


class Expected:
    pass


class After:
    pass


class Before:
    def __getattribute__(self, name):
        if name == "__class__":
            object.__setattr__(self, "__class__", After)
            return int
        return object.__getattribute__(self, name)


receiver = Before()
try:
    super(Expected, receiver)
except BaseException as error:
    print(type(error).__name__, str(error))
print(type(receiver).__name__)
