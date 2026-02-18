import weakref


class SlotDict(dict):
    __slots__ = ("__weakref__",)


d = SlotDict()
d["a"] = 1
w = weakref.ref(d)
print(isinstance(w, weakref.ReferenceType))
print(w() is d)
print(type(w).__name__)
