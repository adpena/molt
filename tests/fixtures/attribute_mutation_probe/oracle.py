"""Run against the probe built with the selected CPython's headers and library."""

import functools
import json
import pickle
import sys

import attribute_mutation_probe as probe


def rejected(action, kind, message):
    try:
        action()
    except kind as error:
        assert str(error) == message, (type(error).__name__, str(error), message)
    else:
        raise AssertionError(f"expected {kind.__name__}: {message}")


class Native(probe.Probe):
    def __reduce_ex__(self, protocol):
        return type(self), (), ({"dictionary_field": 11}, {"slot_field": 12})


native = Native()
setattr(native, "field", 1)
delattr(native, "field")
assert probe.counts(native) == (1, 1)
for delete in (False, True):
    method = object.__delattr__ if delete else object.__setattr__
    args = (native, 4) if delete else (native, 4, 2)
    rejected(
        lambda: method(*args),
        TypeError,
        f"can't apply this {method.__name__} to Native object",
    )
assert probe.counts(native) == (1, 1)
probe.raw_set(native, "field", 0.0)
assert native.field == 0.0
probe.raw_delete(native, "field")
assert probe.counts(native) == (1, 1)
restored = pickle.loads(pickle.dumps(native, protocol=4))
assert (restored.dictionary_field, restored.slot_field) == (11, 12)
assert probe.counts(restored) == (1, 0)


def wrapped():
    pass


wrapped.label = "wrapped"
functools.update_wrapper(native, wrapped, assigned=("label",), updated=())
assert probe.counts(native) == (3, 1)
assert native.label == "wrapped" and native.__wrapped__ is wrapped
native._asyncio_future_blocking = False
assert not native._asyncio_future_blocking
assert probe.counts(native) == (4, 1)
probe.set_failure(native, True)
rejected(
    lambda: functools.update_wrapper(native, wrapped, assigned=("label",), updated=()),
    ValueError,
    "native mutation sentinel",
)
assert probe.counts(native) == (5, 1)
rejected(
    lambda: setattr(native, "_asyncio_future_blocking", False),
    ValueError,
    "native mutation sentinel",
)
assert probe.counts(native) == (6, 1)

events = []


class Meta(type):
    def __setattr__(cls, name, value):
        events.append(("set", name, value))

    def __delattr__(cls, name):
        events.append(("delete", name))


class Ordinary:
    pass


class WithMeta(metaclass=Meta):
    pass


for receiver, label in ((Ordinary, "type"), (WithMeta, "Meta")):
    for delete in (False, True):
        method = object.__delattr__ if delete else object.__setattr__
        args = (receiver, "field") if delete else (receiver, "field", 1)
        rejected(
            lambda: method(*args),
            TypeError,
            f"can't apply this {method.__name__} to {label} object",
        )
    before = len(events)
    probe.raw_set(receiver, "field", 0.0)
    assert vars(receiver)["field"] == 0.0
    probe.raw_delete(receiver, "field")
    assert "field" not in vars(receiver)
    assert len(events) == before
setattr(WithMeta, "field", 3)
delattr(WithMeta, "field")
assert events == [("set", "field", 3), ("delete", "field")]


# Physical generic dictionary ownership is intentionally distinct from the
# semantic namespace for static classes (CPython 3.12 oracle).
raw_name = "_molt_raw_class_oracle"
for receiver in (int, bool, object, type):
    probe.raw_set(receiver, raw_name, 137)
    assert raw_name not in vars(receiver)
    assert object.__getattribute__(receiver, raw_name) == 137
    assert not hasattr(receiver, raw_name)
    probe.raw_delete(receiver, raw_name)
    assert raw_name not in vars(receiver)
    rejected(
        lambda: object.__getattribute__(receiver, raw_name),
        AttributeError,
        f"'type' object has no attribute '{raw_name}'",
    )


class CachedHeap:
    pass


probe.raw_set(CachedHeap, raw_name, 137)
assert getattr(CachedHeap, raw_name) == 137
probe.raw_delete(CachedHeap, raw_name)
assert raw_name not in vars(CachedHeap)
rejected(
    lambda: object.__getattribute__(CachedHeap, raw_name),
    AttributeError,
    f"'type' object has no attribute '{raw_name}'",
)
# The generic dictionary and namespace observations above are the contract.
# Do not depend on a borrowed value left in CPython's private type cache
# after raw deletion. A default type write publishes the current value.
type.__setattr__(CachedHeap, raw_name, 138)
assert getattr(CachedHeap, raw_name) == 138
type.__delattr__(CachedHeap, raw_name)
assert not hasattr(CachedHeap, raw_name)


class Descriptor:
    def __set__(self, receiver, value):
        events.append(("descriptor set", value))

    def __delete__(self, receiver):
        events.append(("descriptor delete",))


class Managed(list):
    field = Descriptor()

    def __setattr__(self, name, value):
        events.append(("override set", name, value))

    def __delattr__(self, name):
        events.append(("override delete", name))


managed = Managed()
events.clear()
object.__setattr__(managed, "field", 9)
object.__delattr__(managed, "field")
probe.raw_set(managed, "field", 0.0)
probe.raw_delete(managed, "field")
setattr(managed, "field", 10)
delattr(managed, "field")
assert events == [
    ("descriptor set", 9),
    ("descriptor delete",),
    ("descriptor set", 0.0),
    ("descriptor delete",),
    ("override set", "field", 10),
    ("override delete", "field"),
]
# The comparison happens inside the native namespace dictionary commit. A
# precomputed slot plan would miss the sibling __getattr__ added by __eq__.
CollisionTarget = probe.slot_type()
original_getattribute = CollisionTarget.__getattribute__
probe.raw_delete(CollisionTarget, "__getattribute__")
collision_events = []
collision_armed = False


class CollisionName(str):
    __hash__ = str.__hash__

    def __eq__(self, other):
        global collision_armed
        if collision_armed:
            collision_armed = False
            collision_events.append("comparison")
            type.__setattr__(CollisionTarget, "__getattr__", lambda self, name: 37)
        return str.__eq__(self, other)


probe.raw_set(CollisionTarget, CollisionName("__getattribute__"), original_getattribute)
collision_armed = True
type.__setattr__(CollisionTarget, "__getattribute__", original_getattribute)
assert collision_events == ["comparison"]
assert CollisionTarget().missing_after_comparison == 37


# A managed class with an existing native view must publish successful type
# descriptor and metadata writes to C watchers before displaced values retire.
class WatchedMetadata:
    pass


metadata_watch_events = []
probe.watch_start(WatchedMetadata)
try:
    for direct in (False, True):
        for field, value in (
            ("__doc__", "new doc"),
            ("__name__", "WatchedMetadata"),
            ("__qualname__", "WatchedMetadata"),
            ("__annotations__", {"field": int}),
            ("__abstractmethods__", ("work",)),
        ):
            probe.watch_arm(WatchedMetadata)
            if direct:
                vars(type)[field].__set__(WatchedMetadata, value)
            else:
                setattr(WatchedMetadata, field, value)
            calls, version = probe.watch_state(WatchedMetadata)
            invalidates = not direct or field not in ("__name__", "__qualname__")
            assert calls == int(invalidates)
            assert (version == 0) == invalidates
            metadata_watch_events.append((direct, field, calls))
        probe.watch_arm(WatchedMetadata)
        try:
            if direct:
                vars(type)["__name__"].__set__(WatchedMetadata, 42)
            else:
                WatchedMetadata.__name__ = 42
        except TypeError:
            pass
        else:
            raise AssertionError("invalid name admitted")
        calls, version = probe.watch_state(WatchedMetadata)
        assert calls == 0 and version != 0

    retired_metadata = []

    class RetiredDoc:
        def __del__(self):
            retired_metadata.append(
                (WatchedMetadata.__doc__, probe.watch_state(WatchedMetadata))
            )
            WatchedMetadata.__doc__ = "reentered"

    WatchedMetadata.__doc__ = RetiredDoc()
    probe.watch_arm(WatchedMetadata)
    WatchedMetadata.__doc__ = "committed"
    assert retired_metadata == [("committed", (1, 0))]
    assert WatchedMetadata.__doc__ == "reentered"

    identity_retirements = []

    class RetiredIdentity(str):
        def __del__(self):
            calls, version = probe.watch_state(WatchedMetadata)
            identity_retirements.append((calls, version != 0))

    for direct in (False, True):
        for field in ("__name__", "__qualname__"):
            descriptor = vars(type)[field]
            descriptor.__set__(WatchedMetadata, RetiredIdentity("OldIdentity"))
            probe.watch_arm(WatchedMetadata)
            if direct:
                descriptor.__set__(WatchedMetadata, "WatchedMetadata")
            else:
                setattr(WatchedMetadata, field, "WatchedMetadata")
            assert identity_retirements[-1] == (0, True)
            calls, version = probe.watch_state(WatchedMetadata)
            assert calls == int(not direct)
            assert (version == 0) == (not direct)

    metadata_retirements = []

    class RetiredMetadata:
        def __bool__(self):
            return True

        def __del__(self):
            calls, version = probe.watch_state(WatchedMetadata)
            metadata_retirements.append(
                (calls, version != 0, probe.is_abstract(WatchedMetadata))
            )

    for direct in (False, True):
        for field in ("__annotations__", "__abstractmethods__"):
            descriptor = vars(type)[field]
            vars(type)["__abstractmethods__"].__set__(WatchedMetadata, ())
            descriptor.__set__(WatchedMetadata, RetiredMetadata())
            probe.watch_arm(WatchedMetadata)
            replacement = () if field == "__abstractmethods__" else {}
            if direct:
                descriptor.__set__(WatchedMetadata, replacement)
            else:
                setattr(WatchedMetadata, field, replacement)
            assert metadata_retirements[-1] == (0, True, field == "__abstractmethods__")
            assert probe.watch_state(WatchedMetadata) == (1, 0)
            assert not probe.is_abstract(WatchedMetadata)
finally:
    probe.watch_stop(WatchedMetadata)


class WatchDescriptor:
    def __set__(self, owner, value):
        if value == -1:
            raise ValueError("descriptor sentinel")
        probe.raw_set(owner, "payload", value)

    def __delete__(self, owner):
        probe.raw_delete(owner, "payload")


class WatchMeta(type):
    field = WatchDescriptor()


class WatchedDescriptor(metaclass=WatchMeta):
    pass


probe.watch_start(WatchedDescriptor)
try:
    for raw in (False, True):
        for delete in (False, True):
            probe.watch_arm(WatchedDescriptor)
            if raw:
                if delete:
                    probe.raw_delete(WatchedDescriptor, "field")
                else:
                    probe.raw_set(WatchedDescriptor, "field", 71)
            elif delete:
                del WatchedDescriptor.field
            else:
                WatchedDescriptor.field = 71
            calls, version = probe.watch_state(WatchedDescriptor)
            assert calls == int(not raw)
            assert (version == 0) == (not raw)
    probe.watch_arm(WatchedDescriptor)
    try:
        WatchedDescriptor.field = -1
    except ValueError as error:
        assert str(error) == "descriptor sentinel"
    else:
        raise AssertionError("descriptor failure lost")
    calls, version = probe.watch_state(WatchedDescriptor)
    assert calls == 0 and version != 0
finally:
    probe.watch_stop(WatchedDescriptor)

print(
    json.dumps(
        {
            "python": sys.version,
            "implementation": sys.implementation.name,
            "result": "passed",
            "native_mutation_counts": probe.counts(native),
            "pickle_mutation_counts": probe.counts(restored),
            "managed_events": events,
            "native_slot_comparison_events": collision_events,
            "metadata_watch_events": metadata_watch_events,
        },
        sort_keys=True,
    )
)
