"""LOAD_BUILD_CLASS uses captured builtins, including restored canonical bindings."""

import builtins


class Proved:
    marker = 1


print(Proved.marker)
original = builtins.__build_class__


def replacement(*args, **kwargs):
    return 17


# A global or class-local binding does not shadow LOAD_BUILD_CLASS.
__build_class__ = replacement


class Shadowed:
    __build_class__ = replacement

    class Inner:
        marker = 2


print(Shadowed.Inner.marker)
builtins.__build_class__ = replacement
builtins.__build_class__ = original


class Restored:
    marker = 3


print(Restored.marker)


def changing_base():
    builtins.__build_class__ = replacement
    return object


try:
    class Captured(changing_base()):
        marker = 4

    print(Captured.marker)
finally:
    builtins.__build_class__ = original

try:
    del builtins.__build_class__
    try:
        class Missing:
            pass
    except NameError as error:
        print(type(error).__name__, str(error))
    else:
        print("missing hook was ignored")
finally:
    builtins.__build_class__ = original


class AfterMissing:
    marker = 5


print(AfterMissing.marker)


import types


def template():
    class Mapped:
        marker = 6

    return Mapped.marker


class BuiltinsMapping:
    def __getitem__(self, name):
        if name == "__build_class__":
            print("mapping builder lookup")
            return original
        return getattr(builtins, name)


mapped = types.FunctionType(
    template.__code__, {"__builtins__": BuiltinsMapping(), "__name__": "mapped"}
)
print(mapped())
