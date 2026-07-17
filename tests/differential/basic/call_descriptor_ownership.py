"""Type-owned call descriptors preserve exceptions and release bound owners."""

import gc
import weakref


class RaisingCallDescriptor:
    def __get__(self, instance, owner):
        raise LookupError("call descriptor failed")


class RaisesDuringCallBinding:
    __call__ = RaisingCallDescriptor()


def ephemeral_metaclass_call():
    class Meta(type):
        def __call__(cls):
            return 7

    class Ephemeral(metaclass=Meta):
        pass

    class_ref = weakref.ref(Ephemeral)
    if Ephemeral() != 7:
        raise AssertionError("custom metaclass call result changed")
    return class_ref


try:
    RaisesDuringCallBinding()()
except LookupError as exc:
    print("descriptor-error", str(exc))

ephemeral_ref = ephemeral_metaclass_call()
gc.collect()
print("metaclass-call-released", ephemeral_ref() is None)
