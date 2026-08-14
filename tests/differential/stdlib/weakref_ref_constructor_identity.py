import gc
import inspect
import sys
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
callback_descriptor = weakref.ReferenceType.__dict__["__callback__"]
print(
    "callback-descriptor-published",
    weakref.ReferenceType.__callback__ is callback_descriptor,
    "__callback__" in dir(weakref.ReferenceType),
)


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
callback_events = []
old_unraisablehook = sys.unraisablehook
sys.unraisablehook = callback_events.append
callback_target = Box()
if sys.version_info >= (3, 15):
    try:
        weakref.ref(callback_target, 42)
    except TypeError as exc:
        print("noncallable-callback-rejected", type(exc).__name__, str(exc))
else:
    noncallable_ref = weakref.ref(callback_target, 42)
    print("noncallable-callback-retained", noncallable_ref.__callback__ == 42)
    del callback_target
    gc.collect()
sys.unraisablehook = old_unraisablehook
if callback_events:
    print(
        "noncallable-callback-unraisable",
        len(callback_events),
        type(callback_events[0].exc_value).__name__,
        callback_events[0].object if sys.version_info < (3, 14) else None,
        callback_events[0].err_msg,
    )


class RefSub(weakref.ReferenceType):
    pass


subref = RefSub(original)
subref.__init__(other, None)
print("subtype-init", type(subref) is RefSub, subref() is original)

one_shot = weakref.ref(original)
cached_hash = hash(one_shot)
print("exact-class-edge", one_shot.__class__ is weakref.ReferenceType)
print(
    "exact-cache-excludes-subtypes",
    weakref.ref(original) is one_shot,
    subref is not one_shot,
    RefSub(original) is not one_shot,
)
print("exact-ref-no-dict", not hasattr(one_shot, "__dict__"))
try:
    one_shot._hash = cached_hash
except AttributeError as exc:
    print("exact-ref-no-shadow-hash", type(exc).__name__)
one_shot.__init__(other, lambda _ref: None)
print(
    "direct-reinit",
    one_shot() is original,
    one_shot() is other,
    one_shot.__callback__ is None,
    hash(one_shot) == cached_hash,
)


def keyed_callback(_ref):
    return None


keyed_target = Box()
keyed_key = object()
keyed = weakref.KeyedRef(
    ob=keyed_target, callback=keyed_callback, key=keyed_key
)
direct_keyed = weakref.KeyedRef.__new__(
    type=weakref.KeyedRef,
    ob=keyed_target,
    callback=keyed_callback,
    key=keyed_key,
)
print(
    "keyed-ref-surface",
    "KeyedRef" not in weakref.__all__,
    type(keyed) is weakref.KeyedRef,
    keyed() is keyed_target,
    keyed.key is keyed_key,
    keyed.__callback__ is keyed_callback,
    not hasattr(keyed, "__dict__"),
    not hasattr(keyed, "_hash"),
)
try:
    keyed._hash = 1
except AttributeError as exc:
    print("keyed-ref-no-shadow-hash", type(exc).__name__)
print(
    "keyed-ref-direct-new",
    direct_keyed() is keyed_target,
    direct_keyed.key is keyed_key,
    direct_keyed.__callback__ is keyed_callback,
    str(inspect.signature(weakref.KeyedRef.__new__)),
    str(inspect.signature(weakref.KeyedRef.__init__)),
)
for operation in ("set", "delete"):
    try:
        if operation == "set":
            keyed.__callback__ = None
        else:
            del keyed.__callback__
    except AttributeError as exc:
        print("keyed-ref-callback-readonly", operation, type(exc).__name__)


class OverrideRef(weakref.ReferenceType):
    def __call__(self):
        return "override-call"

    def __hash__(self):
        return 919


class EqOnlyRef(weakref.ReferenceType):
    def __eq__(self, other):
        return self is other


class SlotsOverrideRef(weakref.ReferenceType):
    __slots__ = ()

    def __getattribute__(self, name):
        if name in {"__class__", "__dict__", "__weakref__", "__callback__"}:
            return "override-" + name
        return object.__getattribute__(self, name)


class DescriptorShadowRef(weakref.ReferenceType):
    __slots__ = ()

    __dict__ = property(lambda self: "descriptor-dict")
    __weakref__ = property(lambda self: "descriptor-weakref")
    __callback__ = property(lambda self: "descriptor-callback")


class ProtocolRef(weakref.ReferenceType):
    def __bool__(self):
        return False

    def __format__(self, spec):
        return "formatted-" + spec

    def __getitem__(self, key):
        return "item-" + key

    def __setitem__(self, key, value):
        self.last_item = (key, value)


override = OverrideRef(original)
eq_only = EqOnlyRef(original)
slots_override = SlotsOverrideRef(original, keyed_callback)
descriptor_shadow = DescriptorShadowRef(original, keyed_callback)
protocol = ProtocolRef(original)
print("subtype-protocol-overrides", type(override) is OverrideRef, override(), hash(override))
print(
    "subtype-getattribute-overrides",
    slots_override.__class__,
    slots_override.__dict__,
    slots_override.__weakref__,
    slots_override.__callback__,
    object.__getattribute__(slots_override, "__callback__") is keyed_callback,
)
print(
    "subtype-descriptor-shadows",
    descriptor_shadow.__dict__,
    descriptor_shadow.__weakref__,
    descriptor_shadow.__callback__,
    object.__getattribute__(descriptor_shadow, "__dict__"),
    object.__getattribute__(descriptor_shadow, "__weakref__"),
    object.__getattribute__(descriptor_shadow, "__callback__"),
)
override.extra = "allowed"
print("subtype-dict", override.__dict__, override.extra)
print("subtype-managed-weakref-before", override.__weakref__)


def exercise_managed_weakref():
    override_watcher = weakref.ref(override, lambda _ref: None)
    print("subtype-managed-weakref-live", weakref.getweakrefcount(override) == 1)
    return override_watcher is not None


exercise_managed_weakref()
gc.collect()
print("subtype-managed-weakref-after", override.__weakref__)
protocol["key"] = "value"
print(
    "subtype-object-protocols",
    bool(protocol),
    format(protocol, "spec"),
    protocol["key"],
    protocol.last_item,
    object.__getstate__(protocol),
)
print(
    "weakref-richcompare",
    weakref.ReferenceType.__eq__(one_shot, object()) is NotImplemented,
    weakref.ReferenceType.__ne__(one_shot, object()) is NotImplemented,
)


class RichReferent:
    def __eq__(self, other):
        return "EQ"

    def __ne__(self, other):
        return "NE"


rich_left = RichReferent()
rich_right = RichReferent()
rich_left_ref = weakref.ref(rich_left)
rich_right_ref = weakref.ref(rich_right)
print(
    "weakref-richcompare-forwarding",
    weakref.ReferenceType.__eq__(rich_left_ref, rich_right_ref),
    weakref.ReferenceType.__ne__(rich_left_ref, rich_right_ref),
)


class SlotBaseRef(weakref.ReferenceType):
    __slots__ = ("a",)


class SlotLeafRef(SlotBaseRef):
    __slots__ = ("b",)


slot_state = SlotLeafRef(original)
slot_state.a = "A"
slot_state.b = "B"
print(
    "weakref-getstate",
    object.__getstate__(one_shot),
    object.__getstate__(RefSub(original)),
    sorted(object.__getstate__(keyed)[1]),
    sorted(object.__getstate__(slot_state)[1]),
)
for label, target, expected in (
    ("exact", one_shot, False),
    ("ordinary-subtype", override, True),
    ("slots-subtype", keyed, False),
):
    try:
        nested = weakref.ref(target)
    except TypeError:
        print("weakref-subtype-weakrefability", label, False, expected)
    else:
        print("weakref-subtype-weakrefability", label, nested() is target, expected)
try:
    hash(eq_only)
except TypeError as exc:
    print("subtype-eq-only-hash", type(exc).__name__)

for descriptor_name, args in (
    ("__call__", (object(),)),
    ("__hash__", (object(),)),
    ("__eq__", (object(), object())),
    ("__repr__", (object(),)),
    ("__init__", (object(), object(), None)),
):
    try:
        getattr(weakref.ReferenceType, descriptor_name)(*args)
    except TypeError as exc:
        print("descriptor-receiver", descriptor_name, type(exc).__name__, str(exc))

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

cycle_key = Box()
cycle_value = Box()
cycle_key_watcher = weakref.ref(cycle_key)
cycle_keyed = weakref.KeyedRef(cycle_value, None, cycle_key)
cycle_key.r = cycle_keyed
del cycle_key, cycle_keyed
gc.collect()
print("keyed-ref-inline-cycle-collected", cycle_key_watcher() is None)

dict_cycle = RefSub(original)
dict_cycle_watcher = weakref.ref(dict_cycle)
dict_cycle.extra = dict_cycle
del dict_cycle
gc.collect()
print("weakref-subtype-dict-cycle-collected", dict_cycle_watcher() is None)
