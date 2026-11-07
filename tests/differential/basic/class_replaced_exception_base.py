"""Builtin base identities must follow the binding read by the class statement."""

import builtins

prepared = []


class Meta(type):
    @classmethod
    def __prepare__(mcls, name, bases):
        prepared.append(name)
        return {}


class Replacement(metaclass=Meta):
    def __init__(self, value):
        self.value = value


original_error = builtins.ValueError
original_list = builtins.list
try:
    builtins.ValueError = Replacement
    builtins.list = Replacement

    class ErrorChild(ValueError):
        pass

    class ListChild(list):
        pass

    for child in (ErrorChild, ListChild):
        instance = child(37)
        print(child.__bases__ == (Replacement,))
        print(isinstance(instance, Replacement), instance.value)
        print(isinstance(instance, BaseException))
finally:
    builtins.ValueError = original_error
    builtins.list = original_list


class RealError(ValueError):
    pass


print(isinstance(RealError("real"), original_error))


def shadowed():
    ValueError = Replacement

    class Local(ValueError):
        pass

    print(Local(41).value)


shadowed()
print(prepared)
