import weakref


class SlotDict(dict):
    __slots__ = ("__weakref__",)


d = SlotDict()
d["a"] = 1
w = weakref.ref(d)
print(isinstance(w, weakref.ReferenceType))
print(w() is d)
print(type(w).__name__)
print("ref-type-identity", weakref.ref is weakref.ReferenceType)
print("exact-cache", weakref.ref(d) is w)


def named_function():
    pass


class NamedInstance:
    __name__ = "instance_name"


class NonStringName:
    __name__ = 42


def repr_shape(value, expected_type, expected_name):
    text = repr(weakref.ref(value))
    print(
        "repr-shape",
        expected_type in text,
        (f"({expected_name})" in text) if expected_name is not None else " (" not in text,
    )


repr_shape(named_function, "'function'", "named_function")
repr_shape(NamedInstance, "'type'", "NamedInstance")
named_instance = NamedInstance()
repr_shape(named_instance, "'NamedInstance'", "instance_name")
non_string_name = NonStringName()
repr_shape(non_string_name, "'NonStringName'", None)


class Box:
    pass


original = Box()
other = Box()
try:
    weakref.ref(original, 42)
except TypeError as exc:
    print("callback-validation", type(exc).__name__)
else:
    print("callback-validation", "missing")


class RefSub(weakref.ReferenceType):
    pass


subref = RefSub(original)
subref.__init__(other, None)
print("subtype-init", type(subref) is RefSub, subref() is original)

one_shot = weakref.ref(original)
cached_hash = hash(one_shot)
one_shot.__init__(other, lambda _ref: None)
print(
    "direct-reinit",
    one_shot() is original,
    one_shot() is other,
    one_shot.__callback__ is None,
    hash(one_shot) == cached_hash,
)

forged_rejected = True
try:
    from _intrinsics import require_intrinsic
except ImportError:
    pass
else:
    try:
        register = require_intrinsic("molt_weakref_register")
    except RuntimeError:
        pass
    else:
        ForgedReferenceType = type("ReferenceType", (), {})
        try:
            register(ForgedReferenceType(), original, None)
        except TypeError:
            pass
        else:
            forged_rejected = False
print("forged-reference-type-rejected", forged_rejected)
