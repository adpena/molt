"""Finalizer eligibility follows current class/MRO behavior, not sealing time."""

import gc
import weakref

events = []


class Base:
    pass


class Child(Base):
    pass


class Leaf(Child):
    pass


def base_finalizer(self):
    events.append(("base", self.name))


def own_finalizer(self):
    events.append(("own", self.name))


preexisting = Leaf()
preexisting.name = "preexisting"
Base.__del__ = base_finalizer
del preexisting
print("late-base", events)

removed = Leaf()
removed.name = "removed"
del Base.__del__
del removed
print("removed-base", events)

# Releasing the replaced namespace value can itself run Python. That callback
# destroys an existing descendant while the outer class mutation is active.
for remove in (False, True):

    class ReentrantBase:
        pass

    class ReentrantChild(ReentrantBase):
        pass

    def old_finalizer(self):
        events.append(("old-unexpected", self.name))

    ReentrantBase.__del__ = old_finalizer
    retained = [ReentrantChild()]
    retained[0].name = "during-remove" if remove else "during-replace"

    def release_instance(_weak):
        events.append(
            ("namespace-callback", remove, "__del__" in ReentrantBase.__dict__)
        )
        retained.clear()

    weak = weakref.ref(old_finalizer, release_instance)
    del old_finalizer
    if remove:
        del ReentrantBase.__del__
    else:
        ReentrantBase.__del__ = base_finalizer
    print("namespace-commit", remove, weak() is None, len(retained), events)

Base.__del__ = base_finalizer
Child.__del__ = own_finalizer
overridden = Leaf()
overridden.name = "overridden"
del overridden
del Child.__del__
inherited = Leaf()
inherited.name = "inherited-again"
del inherited
print("mro-selection", events)

cycle = Leaf()
cycle.name = "cycle"
cycle.peer = cycle
del cycle
gc.collect()
print("cycle", events)
del Base.__del__

# A class is itself an instance: metaclass MRO mutations govern its finalizer.
meta_events = []


class MetaBase(type):
    pass


class MetaChild(MetaBase):
    pass


def metaclass_finalizer(cls):
    meta_events.append(("base", cls.__name__))


def metaclass_override(cls):
    meta_events.append(("own", cls.__name__))


existing_type = MetaChild("ExistingType", (), {})
MetaBase.__del__ = metaclass_finalizer
del existing_type
gc.collect()
print("metaclass-late", meta_events)

removed_type = MetaChild("RemovedType", (), {})
del MetaBase.__del__
del removed_type
gc.collect()
print("metaclass-removed", meta_events)

MetaBase.__del__ = metaclass_finalizer
MetaChild.__del__ = metaclass_override
overridden_type = MetaChild("OverriddenType", (), {})
del overridden_type
gc.collect()
print("metaclass-override", meta_events)
del MetaChild.__del__


class FailingDescriptor:
    def __set_name__(self, owner, name):
        meta_events.append(("set-name", owner.__name__, type(owner) is MetaChild))
        raise RuntimeError("descriptor failure")


class DuplicateBase:
    pass


for label, bases, namespace in (
    ("qualname", (), {"__qualname__": 42}),
    ("slots", (), {"__slots__": (42,)}),
    ("nonbase", (42,), {}),
    ("duplicate", (DuplicateBase, DuplicateBase), {}),
    ("layout-conflict", (int, list), {}),
    ("descriptor", (), {"field": FailingDescriptor()}),
):
    meta_events.clear()
    try:
        MetaChild(label, bases, namespace)
    except Exception as error:
        meta_events.append(("error", type(error).__name__))
    gc.collect()
    print("metaclass-construction", label, meta_events)
del MetaBase.__del__


# Runtime builtin identities cannot be inferred from user-controlled names.
def named_finalizer(self):
    meta_events.append(("named", type(self).__name__))


for reserved_name in ("frame", "traceback"):
    meta_events.clear()
    OrdinaryNamed = type(reserved_name, (), {"__del__": named_finalizer})
    ordinary_named = OrdinaryNamed()
    del ordinary_named
    print("builtin-spelled-ordinary", reserved_name, meta_events)
    del OrdinaryNamed.__del__

    meta_events.clear()
    MetaNamed = type(reserved_name, (type,), {"__del__": named_finalizer})
    type_named = MetaNamed("NamedMetaclassInstance", (), {})
    del type_named
    gc.collect()
    print("builtin-spelled-metaclass", reserved_name, meta_events)
    del MetaNamed.__del__
