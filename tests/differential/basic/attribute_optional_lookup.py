"""Optional lookup preserves hook, descriptor, type and exception semantics."""

import sys
import types


class Plain:
    value = 0.0


class Slotted:
    __slots__ = ("value",)


for receiver in (Plain(), Plain, Slotted(), 1, None):
    print("missing", getattr(receiver, "absent", "default"), hasattr(receiver, "absent"))
print("present", getattr(Plain(), "value", "default"), hasattr(Plain(), "value"))
slotted = Slotted()
print("unset-slot", getattr(slotted, "value", "default"), hasattr(slotted, "value"))
slotted.value = 0.0
print("set-slot", getattr(slotted, "value", "default"), hasattr(slotted, "value"))
try:
    vars(slotted)
except TypeError as error:
    print("vars-miss", str(error))

base = object()
print("bases-probe", types.resolve_bases((base,))[0] is base)


events = []


class Fallback:
    @property
    def value(self):
        events.append("property")
        raise AttributeError("getter")

    def __getattr__(self, name):
        events.append(("fallback", name))
        if name == "error":
            raise RuntimeError("fallback error")
        if name == "absent":
            raise AttributeError("fallback miss")
        return 0.0


fallback = Fallback()
print("fallback", getattr(fallback, "value", "default"), hasattr(fallback, "value"))
print("fallback-miss", getattr(fallback, "absent", "default"), hasattr(fallback, "absent"))
for operation in (getattr, hasattr):
    try:
        if operation is getattr:
            operation(fallback, "error", "default")
        else:
            operation(fallback, "error")
    except RuntimeError as error:
        print("fallback-error", str(error))
print(events)


class Custom:
    def __getattribute__(self, name):
        if name == "error":
            raise ValueError("custom error")
        raise AttributeError("custom miss")


custom = Custom()
print("custom-miss", getattr(custom, "absent", "default"), hasattr(custom, "absent"))
try:
    getattr(custom, "absent")
except AttributeError as error:
    print("required-custom", str(error))
for operation in (getattr, hasattr):
    try:
        if operation is getattr:
            operation(custom, "error", "default")
        else:
            operation(custom, "error")
    except ValueError as error:
        print("custom-error", str(error))


class CustomFallback(Custom):
    def __getattr__(self, name):
        return ("custom-fallback", name)


print(getattr(CustomFallback(), "absent", "default"))


module = types.ModuleType("optional_lookup")
print("module-miss", getattr(module, "absent", "default"), hasattr(module, "absent"))


def module_fallback(name):
    if name == "absent":
        raise AttributeError("module miss")
    if name == "error":
        raise RuntimeError("module error")
    return ("module-fallback", name)


module.__getattr__ = module_fallback
print(getattr(module, "value", "default"))
print("module-hook-miss", getattr(module, "absent", "default"), hasattr(module, "absent"))
try:
    getattr(module, "error", "default")
except RuntimeError as error:
    print(str(error))


class Meta(type):
    def __instancecheck__(cls, value):
        return value == 42

    def __subclasscheck__(cls, value):
        return value is Plain


class Checked(metaclass=Meta):
    pass


print("type-default", isinstance(Plain(), Plain), isinstance(Plain(), str))
print("subtype-default", issubclass(Slotted, object), issubclass(Slotted, Plain))
print("type-hook", isinstance(42, Checked), isinstance(43, Checked))
print("subtype-hook", issubclass(Plain, Checked), issubclass(Slotted, Checked))


class PlainError:
    @property
    def value(self):
        raise ValueError("plain descriptor")


def capture_descriptor_error():
    try:
        getattr(PlainError(), "value", "default")
    except ValueError as error:
        frames = []
        trace = error.__traceback__
        while trace is not None:
            frames.append(trace.tb_frame.f_code.co_name)
            trace = trace.tb_next
        return str(error), frames


print("descriptor-trace", capture_descriptor_error())


# Neither a miss nor a suppressed descriptor failure may consume the surrounding
# handled exception. A genuine failure still chains to that same exception.
try:
    raise LookupError("outer")
except LookupError as outer:
    print("active-before", sys.exception() is outer)
    print(getattr(Plain(), "absent", "default"))
    print(getattr(fallback, "value", "default"))
    print("active-after", sys.exception() is outer)
    try:
        getattr(custom, "error", "default")
    except ValueError as inner:
        print("context", inner.__context__ is outer)
    print("active-restored", sys.exception() is outer)


try:
    getattr(Plain(), "absent")
except AttributeError as error:
    print("required-miss", error.name, isinstance(error.obj, Plain))
