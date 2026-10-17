"""Purpose: method calls through properties, __getattr__ and __getattribute__ match CPython (HF-216).

Plain functions and native method descriptors selected through generic lookup
load with a separate receiver. Data descriptors, instance values and custom
attribute hooks retain the actual callable value they return.
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



def report_native(label, call):
    try:
        print(label, call())
    except TypeError as error:
        print(label, type(error).__name__, str(error))


class NativeIntercept(int):
    def __getattribute__(self, name):
        print("native_getattribute", name)
        return object.__getattribute__(self, name)


class NativeFallback(int):
    def __getattr__(self, name):
        print("native_getattr", name)
        return lambda: name


report_native("native_custom", lambda: NativeIntercept(7).bit_length(1))
report_native("native_fallback_slot", lambda: NativeFallback(7).bit_length(1))
print("native_missing", NativeFallback(7).missing())


class NativeShadow(int):
    pass


shadow = NativeShadow(7)
shadow.bit_length = shadow.bit_length
report_native("native_instance_bound", lambda: shadow.bit_length(1))
shadow.bit_length = lambda value: ("shadow", value)
print("native_instance_callable", shadow.bit_length(1))
del shadow.bit_length
report_native("native_unshadowed", lambda: shadow.bit_length(1))

# Selection precedes argument evaluation, even when that argument shadows it.
def change_selected():
    shadow.bit_length = lambda value: ("replacement", value)
    return 1


report_native("native_selected_before_arg", lambda: shadow.bit_length(change_selected()))
print("native_replacement_after_arg", shadow.bit_length(2))


class WrongNativeReceiver:
    method = int.bit_length


def evaluated_argument():
    print("native_bad_receiver_arg_evaluated")
    return 1


report_native("native_receiver_admission_at_call", lambda: WrongNativeReceiver().method(evaluated_argument()))
