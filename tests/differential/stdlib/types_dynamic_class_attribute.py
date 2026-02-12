"""Purpose: differential coverage for types.DynamicClassAttribute descriptor parity."""

import types


class Box:
    def __init__(self):
        self._value = 1

    @types.DynamicClassAttribute
    def value(self):
        return self._value

    @value.setter
    def value(self, new_value):
        self._value = new_value

    @value.deleter
    def value(self):
        self._value = -1


b = Box()
print("instance_get", b.value)
b.value = 9
print("instance_set", b._value)
del b.value
print("instance_del", b._value)
try:
    Box.value
except Exception as exc:
    print("class_access", type(exc).__name__)

descriptor = types.DynamicClassAttribute(lambda self: 5)
print("manual_get", descriptor.__get__(b))

abstract_descriptor = types.DynamicClassAttribute(lambda self: 7)
abstract_descriptor.__isabstractmethod__ = True
print(
    "abstract_class_access",
    abstract_descriptor.__get__(None, Box) is abstract_descriptor,
)

unreadable = types.DynamicClassAttribute()
try:
    unreadable.__get__(b)
except Exception as exc:
    print("unreadable", type(exc).__name__, str(exc))

write_only = types.DynamicClassAttribute(lambda self: 1)
try:
    write_only.__set__(b, 3)
except Exception as exc:
    print("cant_set", type(exc).__name__, str(exc))
try:
    write_only.__delete__(b)
except Exception as exc:
    print("cant_delete", type(exc).__name__, str(exc))

copy_from_getter = descriptor.getter(lambda self: 8)
print("copy_type", type(copy_from_getter).__name__)
print("copy_value", copy_from_getter.__get__(b))

for label, thunk in [
    ("get_missing_instance", lambda: descriptor.__get__()),
    (
        "get_unexpected_kw",
        lambda: descriptor.__get__(instance=None, unexpected=1),
    ),
    ("init_too_many", lambda: types.DynamicClassAttribute(1, 2, 3, 4, 5)),
    ("init_duplicate_fget", lambda: types.DynamicClassAttribute(1, fget=2)),
]:
    try:
        thunk()
    except Exception as exc:
        print(label, type(exc).__name__)
