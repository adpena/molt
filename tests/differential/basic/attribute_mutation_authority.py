import functools
import pickle
import types

events = []


class Meta(type):
    def __setattr__(cls, name, value):
        events.append(("meta set", name, value))
        type.__setattr__(cls, name, value)

    def __delattr__(cls, name):
        events.append(("meta delete", name))
        type.__delattr__(cls, name)


class Class(metaclass=Meta):
    pass


setattr(Class, "field", 1)
print(Class.field, events)
delattr(Class, "field")
print(hasattr(Class, "field"), events)
for delete in (False, True):
    try:
        if delete:
            object.__delattr__(Class, "field")
        else:
            object.__setattr__(Class, "field", 1)
    except TypeError as error:
        print(type(error).__name__, str(error))


class Descriptor:
    def __set__(self, receiver, value):
        events.append(("descriptor set", value))

    def __delete__(self, receiver):
        events.append(("descriptor delete",))


class Managed(list):
    field = Descriptor()

    def __setattr__(self, name, value):
        events.append(("instance set", name, value))
        object.__setattr__(self, name, value)

    def __delattr__(self, name):
        events.append(("instance delete", name))
        object.__delattr__(self, name)


events.clear()
managed = Managed()
setattr(managed, "field", 1)
delattr(managed, "field")
object.__setattr__(managed, "field", 2)
object.__delattr__(managed, "field")
print(events)


class Module(types.ModuleType):
    def __setattr__(self, name, value):
        events.append(("module set", name, value))
        types.ModuleType.__setattr__(self, name, value)

    def __delattr__(self, name):
        events.append(("module delete", name))
        types.ModuleType.__delattr__(self, name)


events.clear()
module = Module("probe")
module.field = 3
del module.field
print(events, hasattr(module, "field"))


class Namespace(types.SimpleNamespace):
    field = Descriptor()

    def __setattr__(self, name, value):
        events.append(("namespace set", name, value))


events.clear()
namespace = Namespace(field=5)
print(namespace.__dict__, events)


class Restored:
    def __reduce_ex__(self, protocol):
        return type(self), (), ({"dictionary_field": 7}, {"slot_field": 8})

    def __setattr__(self, name, value):
        events.append(("build set", name, value))
        object.__setattr__(self, name, value)


events.clear()
restored = pickle.loads(pickle.dumps(Restored(), protocol=4))
print(restored.dictionary_field, restored.slot_field, events)


class Wrapper:
    def __setattr__(self, name, value):
        events.append(("wrapper set", name))
        if name == "label":
            raise ValueError("wrapper mutation sentinel")
        object.__setattr__(self, name, value)


def wrapped():
    pass


wrapped.label = 4
events.clear()
try:
    functools.update_wrapper(Wrapper(), wrapped, assigned=("label",), updated=())
except ValueError as error:
    print(type(error).__name__, str(error), events)


class Name(str):
    def __hash__(self):
        raise AssertionError("type default must canonicalize the name")


class PlainType:
    pass


name = Name("canonical_field")
for explicit in (False, True):
    if explicit:
        type.__setattr__(PlainType, name, 61)
    else:
        setattr(PlainType, name, 61)
    stored = next(key for key in PlainType.__dict__ if key == "canonical_field")
    print("type name", explicit, type(stored) is str, PlainType.canonical_field)
    if explicit:
        type.__delattr__(PlainType, name)
    else:
        delattr(PlainType, name)
    print("type delete", hasattr(PlainType, "canonical_field"))


class IdentityMeta(type):
    def __setattr__(cls, key, value):
        print("meta name", key is name)
        type.__setattr__(cls, key, value)

    def __delattr__(cls, key):
        print("meta delete name", key is name)
        type.__delattr__(cls, key)


class IdentityType(metaclass=IdentityMeta):
    pass


setattr(IdentityType, name, 71)
print("meta canonical", IdentityType.canonical_field)
delattr(IdentityType, name)


for setter, deleter in (
    (object.__setattr__, object.__delattr__),
    (type.__setattr__, type.__delattr__),
    (object.__setattr__, type.__delattr__),
    (type.__setattr__, object.__delattr__),
):
    meta = type("DefaultMeta", (type,), {"__setattr__": setter, "__delattr__": deleter})
    owner = meta("DefaultType", (), {"field": 0})
    label = setter.__objclass__.__name__ + "/" + deleter.__objclass__.__name__
    for delete in (False, True):
        try:
            if delete:
                delattr(owner, "field")
            else:
                setattr(owner, "field", 1)
        except TypeError as error:
            print("meta defaults", label, delete, type(error).__name__, str(error))
        else:
            print("meta defaults", label, delete, owner.__dict__.get("field"))
