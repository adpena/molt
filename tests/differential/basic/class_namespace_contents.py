# A class namespace holds what CPython's type_new and PyType_Ready put there:
# `__module__`, `__doc__` (None by default), `__dict__`/`__weakref__` getsets on
# the class that adds them, and the body's names. `__bases__`, `__mro__` and
# `__text_signature__` live on the type object and are never namespace content.
import weakref


def dunders(cls):
    return sorted(name for name in vars(cls) if name.startswith("__"))


class Plain:
    pass


class Documented:
    """documented"""


class Meta(type):
    pass


class WithMeta(metaclass=Meta):
    pass


class Slotted:
    __slots__ = ("a",)


class WeakSlotted:
    __slots__ = ("a", "__weakref__")


class Error(Exception):
    pass


class IntSub(int):
    pass


class DictSub(dict):
    pass


class Child(Plain):
    pass


Built = Meta("Built", (Plain,), {"x": 1})

for cls in (Plain, Documented, WithMeta, Slotted, WeakSlotted, Error, IntSub):
    print(cls.__name__, dunders(cls))
for cls in (DictSub, Child):
    print(cls.__name__, dunders(cls))
print("doc", Plain.__doc__, Documented.__doc__, WithMeta.__doc__, Built.__doc__)


class Body:
    seen = "__doc__" in locals()


print("doc during body", Body.seen, "__doc__" in vars(Body))

# Hierarchy metadata comes from `type`'s descriptors. Reassigning `__bases__`
# after layout is fixed is outside the verified subset.
print("mro", Child.__mro__ == (Child, Plain, object), Child.__bases__ == (Plain,))



# `__weakref__` is the class's getset: the first weak reference, or None.
item = Plain()
print("weakref none", item.__weakref__)
ref = weakref.ref(item)
print("weakref head", item.__weakref__ is ref)
print("descriptor", repr(vars(Plain)["__weakref__"]))
print("unbound", vars(Plain)["__weakref__"].__get__(None, Plain) is vars(Plain)["__weakref__"])
for action in ("set", "delete"):
    try:
        if action == "set":
            item.__weakref__ = 1
        else:
            del item.__weakref__
    except AttributeError as error:
        print(action, error)
print("slotted weakref", WeakSlotted().__weakref__)
print("child weakref", Child().__weakref__, "__weakref__" in vars(Child))


def no_weakref(label, value):
    try:
        value.__weakref__
    except AttributeError as error:
        print(label, "AttributeError", error)


no_weakref("function", dunders)
no_weakref("set", set())
no_weakref("type", int)
no_weakref("object", object())
no_weakref("exception", ValueError())
print("class access", repr(WithMeta.__weakref__))

# Text signatures come from each type's own declaration; nothing inherits.
for cls in (object, property, memoryview, enumerate, reversed, complex, float):
    print(cls.__name__, cls.__text_signature__)
for cls in (list, tuple, weakref.ref, int, str, dict, type, ValueError):
    print(cls.__name__, cls.__text_signature__)
# 3.13 adds these signatures; 3.14 extends map; 3.15 renames reversed's.
for cls in (bool, set, frozenset, staticmethod, classmethod, zip, map, filter):
    print(cls.__name__, cls.__text_signature__)
print("heap", Plain.__text_signature__, IntSub.__text_signature__)


class Signed:
    """Signed(a, b)
--

Body."""


print("signed", Signed.__text_signature__)
for cls in (object, int, Plain, Error):
    print(cls.__name__, "__text_signature__" in vars(cls), "__mro__" in vars(cls))
