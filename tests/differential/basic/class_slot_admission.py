"""Slot admission and sealed capability policy; compare directly with CPython."""
import functools
import itertools
import io
import types
import weakref


def result(label, factory):
    try:
        value = factory()
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))
        return None
    print(label, "ok")
    return value


def dynamic(base, declaration):
    return type("AdmissionClass", (base,), {"__slots__": declaration})


def statement(base, declaration):
    class AdmissionClass(base):
        __slots__ = declaration
    return AdmissionClass


def capabilities(label, instance):
    try:
        instance.extra = 17
        dictionary = instance.__dict__["extra"] == 17
    except (AttributeError, TypeError):
        dictionary = False
    try:
        reference = weakref.ref(instance)
        weak = reference() is instance
    except TypeError:
        weak = False
    print(label, dictionary, weak)


for label, instance in (
    ("builtin-function", len),
    ("builtin-method", [].append),
    ("module", types.ModuleType("admission")),
    ("namespace", types.SimpleNamespace()),
    ("partial", functools.partial(len, ())),
    ("repeat", itertools.repeat(None)),
    ("tee", itertools.tee(())[0]),
    ("classmethod", classmethod(len)),
    ("staticmethod", staticmethod(len)),
    ("io", io.BytesIO()),
    ("exception", ValueError("admission")),
):
    capabilities("native:" + label, instance)


for make_label, make in (("type", dynamic), ("statement", statement)):
    for label, declaration in (
        ("empty", ()),
        ("empty-name", ("",)),
        ("digit", ("7slot",)),
        ("punctuation", ("bad-slot",)),
        ("space", ("two words",)),
        ("dot", ("a.b",)),
        ("surrogate", ("\ud800",)),
        ("integer", (7,)),
        ("mixed", ("valid", None)),
        ("keyword", ("class",)),
        ("unicode", ("π", "e\u0301")),
        ("duplicate", ("x", "x")),
        ("duplicate-dict", ("__dict__", "__dict__")),
        ("duplicate-weak", ("__weakref__", "__weakref__")),
    ):
        cls = result(make_label + ":" + label, lambda: make(object, declaration))
        if cls is not None and label in ("keyword", "unicode", "duplicate"):
            instance = cls()
            for slot in declaration:
                setattr(instance, slot, 23)
                print(make_label, slot, getattr(instance, slot))

    for base in (int, bytes, tuple, type):
        for label, declaration in (
            ("empty", ()),
            ("field", ("x",)),
            ("invalid-item", (7,)),
            ("invalid-name", ("bad-name",)),
            ("dict", ("__dict__",)),
            ("weak", ("__weakref__",)),
        ):
            result(make_label + ":" + base.__name__ + ":" + label,
                   lambda: make(base, declaration))

    for base in (int, bytes, tuple):
        alias = type("VariableAlias", (base,), {"__slots__": ()})
        alias.__name__ = "RenamedVariable"
        result(make_label + ":renamed:" + base.__name__, lambda: make(alias, ("x",)))
        empty = make(base, ())
        absent = type("UnslottedVariable", (base,), {})
        capabilities(make_label + ":empty:" + base.__name__, empty())
        capabilities(make_label + ":absent:" + base.__name__, absent())


class StaticPrivate:
    __slots__ = ("__secret", "class", "π", "value", "value")


private = StaticPrivate()
private._StaticPrivate__secret = 29
setattr(private, "class", 31)
private.π = 37
private.value = 41
print("private-and-duplicate", private._StaticPrivate__secret,
      getattr(private, "class"), private.π, private.value)
StaticPrivate.__slots__ = ("__dict__", "__weakref__")
capabilities("rebound-still-slotted", private)
result("private-conflict", lambda: type(
    "Private", (), {"__slots__": ("__x",), "_Private__x": 1}))
result("qualname-consumed", lambda: type(
    "QualnameSlot", (), {"__slots__": ("__qualname__",), "__qualname__": "QualnameSlot"}))


class DictionaryOnly:
    __slots__ = ("__dict__",)


class WeakOnly:
    __slots__ = ("__weakref__",)


class Empty:
    __slots__ = ()


class Ordinary:
    pass


for make_label, make in (("type", dynamic), ("statement", statement)):
    for label, base in (
        ("ordinary", Ordinary),
        ("dict", DictionaryOnly),
        ("weak", WeakOnly),
        ("module", types.ModuleType),
        ("set", set),
        ("exception", Exception),
        ("namespace", types.SimpleNamespace),
        ("partial", functools.partial),
        ("repeat", itertools.repeat),
        ("classmethod", classmethod),
        ("staticmethod", staticmethod),
    ):
        for special in ("__dict__", "__weakref__"):
            result(make_label + ":inherited:" + label + ":" + special,
                   lambda: make(base, (special,)))

for label, bases in (
    ("secondary-dict", (Empty, DictionaryOnly)),
    ("secondary-weak", (Empty, WeakOnly)),
    ("secondary-both", (DictionaryOnly, WeakOnly)),
    ("secondary-variable-weak", (int, WeakOnly)),
):
    cls = result(label, lambda: type("Secondary", bases, {"__slots__": ()}))
    if cls is not None:
        capabilities(label + ":instance", cls())


events = []


class SlotIterator:
    def __init__(self, names, hint_error=False):
        self.names = names
        self.index = 0
        self.hint_error = hint_error

    def __iter__(self):
        events.append("iter")
        return self

    def __length_hint__(self):
        events.append("hint")
        if self.hint_error:
            raise LookupError("hint failed")
        return len(self.names)

    def __next__(self):
        events.append("next:" + str(self.index))
        if self.index == len(self.names):
            raise StopIteration
        value = self.names[self.index]
        self.index += 1
        return value


for make_label, make in (("type", dynamic), ("statement", statement)):
    for label, base, names, hint_error in (
        ("valid", object, ("x",), False),
        ("invalid-item", object, (7, "x"), False),
        ("duplicate-dict", object, ("__dict__", "__dict__", "x"), False),
        ("variable-before-item", int, (7,), False),
        ("hint-before-variable", int, ("x",), True),
    ):
        events.clear()
        result(make_label + ":ordering:" + label,
               lambda: make(base, SlotIterator(names, hint_error)))
        print(make_label + ":events:" + label, events)


class MutatingDeclaration:
    def __iter__(self):
        original["x"] = 71
        return iter(("x",))


original = {"__slots__": MutatingDeclaration()}
snapshot = result("namespace-copy", lambda: type("Snapshot", (), original))
if snapshot is not None:
    obj = snapshot()
    obj.x = 73
    print("namespace-copy-values", original["x"], obj.x)


class WatchedMeta(type):
    def __del__(cls):
        events.append("class-finalizer")


class WatchedDescriptor:
    def __set_name__(self, owner, name):
        events.append("set-name:" + name)


class WatchedBase:
    def __init_subclass__(cls):
        events.append("init-subclass")


events.clear()
result("conflict-before-allocation", lambda: WatchedMeta(
    "RejectedConflict", (WatchedBase,),
    {"__slots__": ("x",), "x": 1, "observer": WatchedDescriptor()}))
print("conflict-hooks", events)
events.clear()
result("invalid-before-allocation", lambda: WatchedMeta(
    "RejectedIdentifier", (WatchedBase,),
    {"__slots__": ("bad-name",), "observer": WatchedDescriptor()}))
print("invalid-hooks", events)

class TransferA:
    __slots__ = ()


class TransferB:
    __slots__ = ()


class TransferWeak:
    __slots__ = ("__weakref__",)


transferred = TransferA()
result("transfer-same-policy", lambda: setattr(transferred, "__class__", TransferB))
print("transfer-class", type(transferred) is TransferB)
result("transfer-different-policy", lambda: setattr(transferred, "__class__", TransferWeak))
capabilities("transfer-policy-preserved", transferred)

# Validation precedes mangling; type names need not be identifiers.
OddName = type("my-class", (), {"__slots__": ("__x",)})
odd = OddName()
setattr(odd, "_my-class__x", 79)
print("nonidentifier-class-private-slot", getattr(odd, "_my-class__x"))


class SealedObserver:
    def __set_name__(self, owner, name):
        descriptor = owner.__dict__["value"]
        instance = owner()
        descriptor.__set__(instance, 83)
        events.append((name, descriptor.__get__(instance, owner)))
        # Rebinding is visible but cannot change the physical slot declaration.
        owner.__slots__ = ("changed",)


events.clear()


class SealedBeforeHooks:
    __slots__ = ("value",)
    observer = SealedObserver()


print("static-sealed-before-hooks", events)
events.clear()
type("DynamicSealedBeforeHooks", (), {
    "__slots__": ("value",), "observer": SealedObserver(),
})
print("dynamic-sealed-before-hooks", events)
