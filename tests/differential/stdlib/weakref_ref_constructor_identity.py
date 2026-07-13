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


class Box:
    pass


original = Box()
other = Box()
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
