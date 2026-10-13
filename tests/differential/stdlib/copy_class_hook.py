"""Class-level copy dispatch, independent of default reduction support."""

import copy


# Class hooks precede reduction and instance attributes cannot replace them.
class Hook:
    def __copy__(self):
        return self.result

    def __reduce_ex__(self, protocol):
        raise AssertionError("__copy__ must precede reduction")


class InheritedHook(Hook):
    pass


hook = InheritedHook()
hook.result = object()
hook.__copy__ = lambda: "instance shadow"
print("inherited class hook", copy.copy(hook) is hook.result)
for result in (None, NotImplemented, False):
    hook.result = result
    print("hook result", copy.copy(hook) is result)


marker = object()
calls = []


def subclass_copy(self):
    calls.append(type(self).__name__)
    return marker


class ListHook(list):
    __copy__ = subclass_copy


class DictHook(dict):
    __copy__ = subclass_copy


class SetHook(set):
    __copy__ = subclass_copy


class TupleHook(tuple):
    __copy__ = subclass_copy


class IntHook(int):
    __copy__ = subclass_copy


class FloatHook(float):
    __copy__ = subclass_copy


class StrHook(str):
    __copy__ = subclass_copy


class BytesHook(bytes):
    __copy__ = subclass_copy


class BytearrayHook(bytearray):
    __copy__ = subclass_copy


class FrozensetHook(frozenset):
    __copy__ = subclass_copy


for value in (
    ListHook([1]),
    DictHook(a=1),
    SetHook([1]),
    TupleHook([1]),
    IntHook(1),
    FloatHook(1),
    StrHook("a"),
    BytesHook(b"a"),
    BytearrayHook(b"a"),
    FrozensetHook([1]),
):
    print("builtin subclass hook", copy.copy(value) is marker)
print("hook calls", calls)


class CopyDescriptor:
    def __get__(self, instance, owner):
        print("hook descriptor", instance is None, owner.__name__)
        return subclass_copy


class DescriptorHook:
    __copy__ = CopyDescriptor()


print("descriptor result", copy.copy(DescriptorHook()) is marker)


class MetaHook(type):
    def __getattribute__(cls, name):
        if name == "__copy__":
            return subclass_copy
        return type.__getattribute__(cls, name)


class MetaInstance(metaclass=MetaHook):
    pass


print("metaclass lookup", copy.copy(MetaInstance()) is marker)
print("class atomic", copy.copy(MetaInstance) is MetaInstance)


class BadHook:
    __copy__ = False


class RaisingHook:
    def __copy__(self):
        raise ValueError("hook failure")


class RaisingDescriptor:
    def __get__(self, instance, owner):
        raise LookupError("lookup failure")


class LookupFailure:
    __copy__ = RaisingDescriptor()


for value in (BadHook(), RaisingHook(), LookupFailure()):
    try:
        copy.copy(value)
    except Exception as error:
        print("hook failure", type(error).__name__)


shared = []
original = [shared]
copied = copy.copy(original)
print("exact list", copied is not original, copied[0] is shared)
original = {"item": shared}
copied = copy.copy(original)
print("exact dict", copied is not original, copied["item"] is shared)
original = (shared,)
print("exact tuple", copy.copy(original) is original)


def copy_temporary_text():
    text = "".join(["heap", "-copy"])
    return copy.copy(text)


print("heap atomic lifetime", copy_temporary_text())
