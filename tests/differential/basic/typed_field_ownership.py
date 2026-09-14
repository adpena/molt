"""Typed fields preserve owners and publish before reentrant destruction."""

import math
import weakref


class Box:
    def __init__(self, value):
        self.field = value


def retained_initialization():
    value = [41]
    box = Box(value)
    del value
    box.field.append(42)
    return box


print("retained", retained_initialization().field)

lifetime_events = []


class LifetimeValue:
    def __init__(self, label):
        self.label = label

    def __del__(self):
        lifetime_events.append(self.label)


def local_field_lifetime():
    value = LifetimeValue("local-field")
    box = Box(value)
    del value
    return box.field.label


print("local-field-lifetime", local_field_lifetime(), lifetime_events)


def captured_lifetime():
    value = LifetimeValue("closure-capture")

    def read():
        return value.label

    return read


captured = captured_lifetime()
print("closure-capture-live", captured(), lifetime_events)
del captured
print("closure-capture-released", lifetime_events)

observed = []
holder = Box(None)
incoming = ["incoming"]


class Old:
    def __del__(self):
        observed.append(
            (holder.field is incoming, holder.__dict__["field"] is incoming)
        )
        holder.field = "reentrant"


holder.field = Old()
mirror = holder.__dict__
holder.field = incoming
print("released-after-publication", observed)
print("reentrant", holder.field, mirror["field"])

holder.field = incoming
print("mirror", mirror["field"] is incoming)
holder.field = incoming
print("same-owner", holder.field is incoming)
holder.field = None
print("incoming-alive", incoming)
holder.field = 1
print("scalar-mirror", holder.field, mirror["field"])
holder.field = 2
print("scalar-replacement", holder.field, mirror["field"])

for value in [-(1 << 63), -(1 << 46) - 1, 1 << 46, (1 << 63) - 1]:
    box = Box(value)
    print("wide", box.field == value)
    box.field = 0
    print("overwritten", box.field)


def field_or_missing(box):
    try:
        return box.field
    except AttributeError:
        return "missing"


# The NaN-boxed representation of +0.0 is all-zero bits. Empty fields must use
# a distinct state in constructors, slot descriptors, and backing transitions.
class ConditionalField:
    field = "class-default"

    def __init__(self, assign, value):
        if assign:
            self.field = value


class ZeroSlot:
    __slots__ = ("field", "__dict__")

    def __init__(self, assign, value):
        if assign:
            self.field = value


for factory in [ConditionalField, ZeroSlot]:
    empty = factory(False, None)
    print("zero-empty", factory.__name__, field_or_missing(empty), empty.__getstate__())
    for value in [0.0, -0.0]:
        zero = factory(True, value)
        print("zero-inline", factory.__name__, math.copysign(1.0, zero.field))
        zero_dict = zero.__dict__
        print("zero-materialized", math.copysign(1.0, zero.field), zero.__getstate__())
        del zero.field
        print("zero-deleted", field_or_missing(zero), zero.__getstate__())
        zero.field = value
        del zero.__dict__
        print("zero-dict-reset", field_or_missing(zero), zero.__getstate__())
        zero.field = value
        print("zero-recreated", math.copysign(1.0, zero.field), zero.__getstate__())


def show_field(label, box, mapping):
    print(
        label,
        field_or_missing(box),
        getattr(box, "field", "missing"),
        mapping.get("field", "missing"),
        sorted(mapping.items()),
    )


# Deletion before dictionary exposure must also revoke inline load admission.
scalar = Box(3)
del scalar.field
print("scalar-delete-inline", field_or_missing(scalar))
scalar.field = 4
print("scalar-recreate-inline", scalar.field)


class DefaultBox:
    field = "class-default"

    def __init__(self):
        self.field = "instance"


defaulted = DefaultBox()
del defaulted.field
print("inline-class-fallback", defaulted.field)
defaulted.field = "again"
default_mapping = defaulted.__dict__
del default_mapping["field"]
print("dict-class-fallback", defaulted.field, "field" in default_mapping)

# Once exposed, the dictionary is the only ordinary-attribute owner. Reading
# __dict__ repeatedly must not restore removed entries or omit a None value.
entry = Box(None)
mapping = entry.__dict__
print("materialize-none", sorted(mapping.items()), mapping is entry.__dict__)
mapping["field"] = "dictionary"
print("direct-dict-read", entry.field)
show_field("dict-replacement", entry, mapping)
del mapping["field"]
show_field("dict-delete", entry, mapping)
print("repeat-after-delete", entry.__dict__ is mapping, sorted(entry.__dict__.items()))
entry.field = None
show_field("attribute-recreate-none", entry, mapping)
mapping.update({"field": "updated", "extra": 7})
show_field("dict-update", entry, mapping)
print("dict-pop", mapping.pop("field"))
show_field("after-pop", entry, mapping)
print("dict-setdefault", mapping.setdefault("field", "default"), entry.field)
mapping.clear()
show_field("dict-clear", entry, mapping)
print("repeat-after-clear", sorted(entry.__dict__.items()))
entry.field = "resurrected"
show_field("attribute-after-clear", entry, mapping)
del entry.field
show_field("attribute-delete", entry, mapping)

# getstate must describe Python storage, not expose a second typed-field owner.
state_box = Box(None)
state = object.__getstate__(state_box)
print("state-before-exposure", sorted(state.items()))
state_mapping = state_box.__dict__
state_mapping["field"] = "state-dictionary"
print("state-after-replace", sorted(object.__getstate__(state_box).items()))
state_mapping.clear()
print("state-after-clear", object.__getstate__(state_box))


class SlotBase:
    __slots__ = ("field",)

    def __init__(self):
        self.field = "base-slot"


class SlotChild(SlotBase):
    __slots__ = ("own", "__dict__")

    def __init__(self):
        super().__init__()
        self.own = None
        self.extra = "ordinary"


slots = SlotChild()
slot_mapping = slots.__dict__
print("slots-materialize", sorted(slot_mapping.items()), slots.field, slots.own)
slot_mapping["field"] = "dictionary-field"
slot_mapping["own"] = "dictionary-own"
print("slots-same-name", slots.field, slots.own, sorted(slot_mapping.items()))
slots.field = "new-base-slot"
slots.own = "new-own-slot"
print("slots-set-separate", slots.field, slots.own, sorted(slot_mapping.items()))
slot_state = object.__getstate__(slots)
print("slots-state", sorted(slot_state[0].items()), sorted(slot_state[1].items()))
del slots.field
del slots.own
print(
    "slots-delete-separate",
    hasattr(slots, "field"),
    hasattr(slots, "own"),
    sorted(slot_mapping.items()),
)
slots.field = "restored-slot"
slots.own = None
slot_mapping.clear()
print(
    "slots-survive-dict-clear", slots.field, slots.own, sorted(slots.__dict__.items())
)


class SlotDeclaration:
    __slots__ = ("field", "__dict__")

    def __init__(self):
        self.field = "sealed-slot"


sealed_slot = SlotDeclaration()
SlotDeclaration.__slots__ = ("__dict__",)
sealed_mapping = sealed_slot.__dict__
print("reassigned-slots-materialize", sorted(sealed_mapping.items()))
sealed_mapping["field"] = "dictionary-shadow"
print("reassigned-slots-separate", sealed_slot.field, sealed_mapping["field"])
SlotDeclaration.__slots__ = ()
try:
    print("reassigned-slots-dict-admission", sealed_slot.__dict__ is sealed_mapping)
except AttributeError:
    print("reassigned-slots-dict-denied")


class MutableSlotDeclaration:
    __slots__ = ["field", "__dict__"]

    def __init__(self):
        self.field = "list-declared-slot"


mutable_slot = MutableSlotDeclaration()
MutableSlotDeclaration.__slots__.remove("field")
mutable_mapping = mutable_slot.__dict__
mutable_mapping["field"] = "list-dictionary-shadow"
print("mutated-slots-separate", mutable_slot.field, mutable_mapping["field"])


class IterableSlotDeclaration:
    __slots__ = iter(("field", "__dict__", "__weakref__"))

    def __init__(self):
        self.field = "iterator-declared-slot"


iterable_slot = IterableSlotDeclaration()
IterableSlotDeclaration.__slots__ = ()
iterable_mapping = iterable_slot.__dict__
iterable_mapping["field"] = "iterator-dictionary-shadow"
print("iterator-slots-separate", iterable_slot.field, iterable_mapping["field"])

print("iterator-slots-weakref", weakref.ref(iterable_slot)() is iterable_slot)


class InheritedMutatedSlots(IterableSlotDeclaration):
    __slots__ = ()


inherited_mutated = InheritedMutatedSlots()
inherited_mutated.__dict__["field"] = "inherited-shadow"
print(
    "inherited-mutated-slots",
    inherited_mutated.field,
    inherited_mutated.__dict__["field"],
    weakref.ref(inherited_mutated)() is inherited_mutated,
)


class OriginallyUnslotted:
    pass


OriginallyUnslotted.__slots__ = ()
originally_unslotted = OriginallyUnslotted()
originally_unslotted.field = "ordinary-dictionary-field"
print(
    "late-slots-no-effect",
    originally_unslotted.__dict__["field"],
    weakref.ref(originally_unslotted)() is originally_unslotted,
)

# Whole-dictionary replacement and reset sever old aliases; they must never
# copy retired field values back into a newly installed or empty dictionary.
reset = Box("inline-before-reset")
old_mapping = reset.__dict__
replacement = {"field": "replacement", "extra": None}
reset.__dict__ = replacement
print(
    "dict-assignment",
    reset.__dict__ is replacement,
    reset.field,
    sorted(old_mapping.items()),
    sorted(reset.__dict__.items()),
)
old_mapping["field"] = "old-alias-only"
print("old-dict-alias", reset.field, old_mapping["field"])
reset.field = "new-owner"
print("replacement-alias", replacement["field"])
del reset.__dict__
print(
    "dict-reset-missing",
    field_or_missing(reset),
    sorted(reset.__dict__.items()),
    reset.__dict__ is replacement,
    replacement["field"],
)
reset.field = None
print("dict-reset-recreate", sorted(reset.__dict__.items()))
for invalid in [None, [], 3]:
    try:
        reset.__dict__ = invalid
    except TypeError:
        print("dict-assignment-reject", type(invalid).__name__)
    else:
        print("dict-assignment-accepted", type(invalid).__name__)

# Replacing a dictionary-only owner must release it immediately. Reentrant
# attribute reads observe the committed mapping and writes stay in that same
# dictionary, including after clear/delete/pop detach the original value.
reverse_events = []
reverse_target = Box(None)
reverse_mapping = reverse_target.__dict__


class ReverseOld:
    def __init__(self, label):
        self.label = label

    def __del__(self):
        reverse_events.append(
            (
                self.label,
                field_or_missing(reverse_target) == "committed",
                "field" in reverse_mapping,
                reverse_mapping.get("field", "missing") == "committed",
            )
        )
        reverse_target.field = "callback-" + self.label


for operation in ["replace", "update", "delete", "clear", "pop"]:
    reverse_mapping["field"] = ReverseOld(operation)
    if operation == "replace":
        reverse_mapping["field"] = "committed"
    elif operation == "update":
        reverse_mapping.update({"field": "committed"})
    elif operation == "delete":
        del reverse_mapping["field"]
    elif operation == "clear":
        reverse_mapping.clear()
    else:
        reverse_mapping.pop("field")
    print(
        "reverse-finalizer",
        operation,
        reverse_events[-1] if reverse_events else "not-released",
        field_or_missing(reverse_target) == "callback-" + operation,
        reverse_mapping.get("field", "missing") == "callback-" + operation,
    )


# Class replacement preserves compatible physical fields and fails atomically
# for a different layout. Tuple subclasses never share the exact empty singleton.
class SlotShapeA:
    __slots__ = ("field",)


class SlotShapeB:
    __slots__ = ("field",)


class SlotShapeOther:
    __slots__ = ("other",)


shape = SlotShapeA()
shape.field = -0.0
shape.__class__ = SlotShapeB
print("class-replacement", type(shape) is SlotShapeB, math.copysign(1.0, shape.field))
try:
    shape.__class__ = SlotShapeOther
except TypeError:
    print(
        "class-replacement-reject",
        type(shape) is SlotShapeB,
        math.copysign(1.0, shape.field),
    )


class TupleChild(tuple):
    pass


empty_child = TupleChild()
other_empty_child = TupleChild(())
filled_child = TupleChild((1, 2))
copied_child = TupleChild(filled_child)
print(
    "tuple-subclass-empty",
    type(empty_child) is TupleChild,
    type(other_empty_child) is TupleChild,
    empty_child is other_empty_child,
    type(()) is tuple,
)
print(
    "tuple-subclass-copy",
    type(copied_child) is TupleChild,
    copied_child == filled_child,
    copied_child is filled_child,
)


class IteratedTuple(tuple):
    def __iter__(self):
        print("tuple-subclass-iterated")
        return iter((7, 8))


print("tuple-subclass-iteration", TupleChild(IteratedTuple((1, 2))))


# Class assignment is still attribute mutation: custom setters take precedence,
# and explicit object.__setattr__ still honors a data descriptor.
class_assignment_events = []


class CustomClassSetter:
    def __setattr__(self, name, value):
        class_assignment_events.append(("custom", name, value is SlotShapeB))


class DescriptorClassSetter:
    @property
    def __class__(self):
        return type(self)

    @__class__.setter
    def __class__(self, value):
        class_assignment_events.append(("descriptor", value is SlotShapeB))


custom_class_setter = CustomClassSetter()
custom_class_setter.__class__ = SlotShapeB
descriptor_class_setter = DescriptorClassSetter()
descriptor_class_setter.__class__ = SlotShapeB
object.__setattr__(descriptor_class_setter, "__class__", SlotShapeB)
print(
    "class-assignment-precedence",
    class_assignment_events,
    type(custom_class_setter) is CustomClassSetter,
    type(descriptor_class_setter) is DescriptorClassSetter,
)

invalid_tuple_events = []


class InvalidTupleIterable:
    def __iter__(self):
        invalid_tuple_events.append("iterated")
        return iter((1,))


try:
    tuple.__new__(list, InvalidTupleIterable())
except TypeError:
    print("tuple-invalid-class-before-iteration", invalid_tuple_events)
