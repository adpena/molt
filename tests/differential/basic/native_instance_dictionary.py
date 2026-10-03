"""Native dictionary ownership, descriptor precedence and real cyclic release."""
import gc
import io
import weakref


def outcome(label, operation):
    try:
        operation()
    except Exception as error:
        print(label, type(error).__name__)
    else:
        print(label, "ok")


outcome("builtin-set", lambda: setattr(len, "extra", 1))
outcome("builtin-delete", lambda: delattr(len, "extra"))
outcome("builtin-dictionary", lambda: len.__dict__)
original_module = len.__module__
len.__module__ = "native-test"
print("builtin-module", len.__module__)
del len.__module__
print("builtin-module-deleted", len.__module__)
len.__module__ = original_module


def managed():
    pass


managed.extra = 12
namespace = managed.__dict__
namespace["second"] = 13
print("managed", managed.extra, managed.second, namespace is managed.__dict__)
del managed.extra
print("managed-delete", "extra" not in namespace)


events = []


class NativeTuple(tuple):
    def __setattr__(self, name, value):
        events.append("set:" + name)
        object.__setattr__(self, name, value)

    def __delattr__(self, name):
        events.append("del:" + name)
        object.__delattr__(self, name)


class SealedTuple(tuple):
    __slots__ = ()


original = (1, 2, 3)
value = NativeTuple(original)
value.extra = 17
namespace = value.__dict__
namespace["second"] = 18
print("tuple", tuple(value), original, value.extra, value.second, "extra" in dir(value))
del value.extra
value.__dict__ = {"third": 19}
print("tuple-replace", value.third, namespace == {"second": 18}, tuple(value))
del value.__dict__
print("tuple-delete-dictionary", value.__dict__, events)
outcome("exact-tuple-dict", lambda: original.__dict__)
outcome("sealed-tuple-dict", lambda: SealedTuple().__dict__)
outcome("sealed-tuple-set", lambda: setattr(SealedTuple(), "extra", 1))


class NativeBytes(bytes):
    pass


class SealedBytes(bytes):
    __slots__ = ()


byte_value = NativeBytes()
byte_value.extra = 20
print("bytes-dictionary", byte_value.__dict__["extra"], "extra" in dir(byte_value))
outcome("sealed-bytes-dict", lambda: SealedBytes().__dict__)
outcome("sealed-bytes-set", lambda: setattr(SealedBytes(), "extra", 1))


for label, stream in (("bytesio", io.BytesIO(b"abc")), ("stringio", io.StringIO("abc"))):
    stream.extra = 21
    namespace = stream.__dict__
    namespace["second"] = 22
    namespace["closed"] = "shadow"
    print(label, stream.extra, stream.second, stream.closed, namespace is stream.__dict__, "extra" in dir(stream))
    outcome(label + "-readonly", lambda: setattr(stream, "closed", True))
    outcome(label + "-dict-replace", lambda: setattr(stream, "__dict__", {}))
    outcome(label + "-dict-delete", lambda: delattr(stream, "__dict__"))
    object.__setattr__(stream, "third", 23)
    object.__delattr__(stream, "extra")
    print(label + "-explicit", stream.third, "extra" not in namespace, stream.getvalue())
    stream.close()
    print(label + "-closed", stream.closed, stream.second)


released = []


class CyclicTuple(tuple):
    def __del__(self):
        released.append("tuple")


def tuple_cycle():
    cycle = CyclicTuple((4, 5))
    cycle.self = cycle


def io_cycle():
    stream = io.BytesIO()
    stream.self = stream
    return weakref.ref(stream)


tuple_cycle()
reference = io_cycle()
gc.collect()
print("cycles", released == ["tuple"], reference() is None)

for label, obj in (("list", []), ("dict", {}), ("set", set())):
    try:
        setattr(obj, "extra", 1)
    except AttributeError as error:
        print("label", label, "'" + label + "'" in str(error))
