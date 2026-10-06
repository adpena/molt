"""Actual type, inherited __class__, and custom descriptor precedence."""

import types
from dataclasses import dataclass


# Independently observed with CPython 3.12: generic object lookup reads a
# heap class's own namespace, but neither inherited nor static type namespaces.
# Type descriptors remain visible, including __dict__ and __name__.
class GenericParent:
    inherited = 17

    def inherited_method(self):
        return 17


class GenericChild(GenericParent):
    own = 23

    def own_method(self):
        return 23


for phase in ("before normal lookup", "after normal lookup"):
    print(
        "heap own",
        phase,
        object.__getattribute__(GenericParent, "inherited") == 17,
        object.__getattribute__(GenericChild, "own") == 23,
        object.__getattribute__(GenericParent, "inherited_method")
        is GenericParent.inherited_method,
        object.__getattribute__(GenericChild, "own_method") is GenericChild.own_method,
    )
    for owner, name in (
        (GenericChild, "inherited"),
        (GenericChild, "inherited_method"),
        (int, "bit_length"),
        (bool, "bit_length"),
    ):
        try:
            object.__getattribute__(owner, name)
        except AttributeError:
            print("generic namespace missing", phase, owner.__name__, name, True)
        else:
            print("generic namespace missing", phase, owner.__name__, name, False)
    for owner in (GenericParent, GenericChild, int, bool):
        print(
            "type descriptors",
            phase,
            owner.__name__,
            isinstance(
                object.__getattribute__(owner, "__dict__"), types.MappingProxyType
            ),
            object.__getattribute__(owner, "__name__") == owner.__name__,
        )
    print("normal inherited", GenericChild.inherited == 17)
    print("normal builtin", int.bit_length is bool.bit_length)


# Metaclass writes belong to each class object's dictionary. A compiler's
# inferred instance-field facts must never authorize loads from type metadata.
class GenericFieldMeta(type):
    def __init__(self, name, bases, namespace):
        self.meta_present = 29


class GenericFieldOwner(metaclass=GenericFieldMeta):
    own_present = 31


for phase in ("cold", "warm"):
    print(
        "metaclass generic fields",
        phase,
        object.__getattribute__(GenericFieldOwner, "meta_present"),
        object.__getattribute__(GenericFieldOwner, "own_present"),
    )


# All type getsets remain visible through generic object lookup; the own
# namespace, descriptor binding and constructor flag are independent concerns.
for owner in (object, type, int, bool, list, GenericParent, GenericChild):
    print(
        "type metadata getsets",
        owner.__name__,
        object.__getattribute__(owner, "__doc__") == owner.__doc__,
        object.__getattribute__(owner, "__text_signature__")
        == owner.__text_signature__,
    )
    try:
        object.__getattribute__(owner, "__abstractmethods__")
    except AttributeError:
        print("abstract methods absent", owner.__name__, True)
    else:
        print("abstract methods absent", owner.__name__, False)


class Documentation:
    def __get__(self, instance, owner):
        return (instance is None, owner.__name__)


class MetadataParent:
    __doc__ = Documentation()
    __text_signature__ = "not a native signature"
    __abstractmethods__ = ("from body",)


class MetadataChild(MetadataParent):
    pass


print(
    "own metadata",
    MetadataParent.__doc__,
    MetadataChild.__doc__ is None,
    MetadataParent.__text_signature__ is None,
    type(MetadataParent()).__name__ == "MetadataParent",
)
try:
    MetadataChild.__abstractmethods__
except AttributeError:
    print("abstract methods not inherited", True)
else:
    print("abstract methods not inherited", False)
MetadataParent.__doc__ = 29
print("doc write", object.__getattribute__(MetadataParent, "__doc__") == 29)
for name in ("__doc__", "__text_signature__"):
    try:
        delattr(MetadataParent, name)
    except (TypeError, AttributeError) as error:
        print("metadata delete rejected", name, type(error).__name__)

abstract_values = ["work"]
MetadataParent.__abstractmethods__ = abstract_values
abstract_values.clear()
try:
    MetadataParent()
except TypeError:
    print("abstract truth latched", True)
else:
    print("abstract truth latched", False)
print("abstract child remains concrete", type(MetadataChild()).__name__)


class FailingTruth:
    def __bool__(self):
        raise ValueError("abstract truth failure")


try:
    MetadataParent.__abstractmethods__ = FailingTruth()
except ValueError as error:
    print(
        "abstract truth failure",
        str(error),
        MetadataParent.__abstractmethods__ is abstract_values,
    )
del MetadataParent.__abstractmethods__
print("abstract delete enables construction", type(MetadataParent()).__name__)
try:
    del MetadataParent.__abstractmethods__
except AttributeError:
    print("abstract repeated delete rejected", True)
MetadataParent.__abstractmethods__ = ()
print("abstract false enables construction", type(MetadataParent()).__name__)


def generator():
    yield 1


async def coroutine():
    return 2


async def async_generator():
    yield 3


@types.coroutine
def iterable_coroutine():
    if False:
        yield
    return 4


class Awaitable:
    def __await__(self):
        if False:
            yield
        return 5


def make_cell():
    value = 6
    return (lambda: value).__closure__[0]


def check(label, value):
    actual = type(value)
    sentinel = object()
    print(
        label,
        value.__class__ is actual,
        getattr(value, "__class__") is actual,
        getattr(value, "__class__", sentinel) is actual,
        object.__getattribute__(value, "__class__") is actual,
        hasattr(value, "__class__"),
        isinstance(value, actual),
        callable(object.__getattribute__(value, "__repr__")),
    )


g = generator()
c = coroutine()
a = async_generator()
w = c.__await__()
i = iterable_coroutine()
custom = Awaitable()
custom_iterator = custom.__await__()
send = a.asend(None)
throw = a.athrow(ValueError("unused"))
close = a.aclose()
for label, value in [
    ("generator", g),
    ("coroutine", c),
    ("async generator", a),
    ("coroutine wrapper", w),
    ("iterable coroutine", i),
    ("custom awaitable", custom),
    ("custom await iterator", custom_iterator),
    ("async generator send", send),
    ("async generator throw", throw),
    ("async generator close", close),
    ("plain object", object()),
    ("class object", Awaitable),
    ("cell", make_cell()),
    ("dictionary", {}),
]:
    check(label, value)
print(
    "canonical suspension types",
    type(g) is types.GeneratorType,
    type(c) is types.CoroutineType,
    type(a) is types.AsyncGeneratorType,
)
print("class metadata", object.__getattribute__(Awaitable, "__name__"))
print("dictionary method", {}.get("absent", 7))
g.close()
c.close()
i.close()
custom_iterator.close()
# No generator body was entered, so the async generator owns no live resources.
del send, throw, close, a, w

events = []


class NonDataClass:
    def __get__(self, instance, owner):
        events.append("nondata")
        return str


class NonDataOwner:
    __class__ = NonDataClass()


n = NonDataOwner()
n.__dict__["__class__"] = int
print(
    "shadow", n.__class__ is int, object.__getattribute__(n, "__class__") is int, events
)
print("real type", type(n) is NonDataOwner)
del n.__dict__["__class__"]
print(
    "nondata",
    n.__class__ is str,
    object.__getattribute__(n, "__class__") is str,
    events,
)


class DataOwner:
    @property
    def __class__(self):
        events.append("data")
        return str


d = DataOwner()
d.__dict__["__class__"] = int
events.clear()
print(
    "data", d.__class__ is str, object.__getattribute__(d, "__class__") is str, events
)
print("data real type", type(d) is DataOwner, isinstance(d, str))


class FailingOwner:
    def __init__(self, error):
        self.error = error

    @property
    def __class__(self):
        events.append("failure")
        raise self.error("denied")


def ordinary(value):
    return value.__class__


def explicit(value):
    return object.__getattribute__(value, "__class__")


for error in [AttributeError, RuntimeError]:
    failing = FailingOwner(error)
    for reader in [ordinary, explicit]:
        events.clear()
        try:
            reader(failing)
        except Exception as exc:
            print("failure", error.__name__, type(exc).__name__, str(exc), events)
    print("failure real type", type(failing) is FailingOwner)
events.clear()
missing = FailingOwner(AttributeError)
print(
    "optional",
    getattr(missing, "__class__", "missing"),
    hasattr(missing, "__class__"),
    events,
)


class FallbackOwner(FailingOwner):
    def __getattr__(self, name):
        events.append("fallback")
        return str


fallback = FallbackOwner(AttributeError)
events.clear()
print("fallback", fallback.__class__ is str, events)
events.clear()
try:
    object.__getattribute__(fallback, "__class__")
except AttributeError as exc:
    print("explicit failure", str(exc), events)


class OverrideOwner:
    def __getattribute__(self, name):
        if name == "__class__":
            return int
        return object.__getattribute__(self, name)


override = OverrideOwner()
print(
    "override",
    override.__class__ is int,
    object.__getattribute__(override, "__class__") is OverrideOwner,
    type(override) is OverrideOwner,
)


plain_generator = generator()
print("generator await", hasattr(plain_generator, "__await__"))
plain_generator.close()

native_coroutine = coroutine()
for name in ("cr_running", "cr_frame", "cr_code", "cr_await"):
    direct = getattr(native_coroutine, name)
    explicit_member = object.__getattribute__(native_coroutine, name)
    if name == "cr_frame":
        print(
            "coroutine member",
            name,
            direct.f_code is explicit_member.f_code is coroutine.__code__,
            direct.f_lasti == explicit_member.f_lasti,
        )
    else:
        print("coroutine member", name, direct is explicit_member)
native_coroutine.close()
print("closed frame", object.__getattribute__(native_coroutine, "cr_frame") is None)


class LookupMeta(type):
    def __new__(mcls, name, bases, namespace, /, **kwargs):
        return super().__new__(mcls, name, bases, namespace, **kwargs)


class LookupClass(metaclass=LookupMeta):
    pass


print("super metaclass", type(LookupClass) is LookupMeta)


class LookupBase:
    __self__ = "delegated-self"

    def __repr__(self):
        return "delegated-repr"

    def __setattr__(self, name, value):
        super().__setattr__(name, value)


class LookupChild(LookupBase):
    def late_constructor(self):
        return super().__new__()


lookup_receiver = LookupChild()
lookup_receiver.value = 23
lookup_proxy = super(LookupChild, lookup_receiver)
print("super setter", lookup_receiver.value)
print("super repr", lookup_proxy.__repr__())
print("super own class", lookup_proxy.__class__ is super)
print("super delegated member", lookup_proxy.__self__)
print(
    "super generic member",
    object.__getattribute__(lookup_proxy, "__self__") is lookup_receiver,
)
print("super fallback", super(LookupBase, lookup_receiver).__self__ is lookup_receiver)
print("super unbound", super(LookupChild).__thisclass__ is LookupChild)


def late_new(self):
    return self.value


LookupBase.__new__ = late_new
print("super late new", lookup_proxy.__new__(), lookup_receiver.late_constructor())


class LookupDict(dict):
    def __init__(self, values):
        super().__init__(values)


print("super builtin", LookupDict([(1, 2)]))


class SignatureBirth:
    "SignatureBirth(a, b)\n--\n\nOriginal."


print("birth signature", SignatureBirth.__text_signature__)
SignatureBirth.__doc__ = "SignatureBirth(changed)\n--\n\nReplacement."
print("signature after doc", SignatureBirth.__text_signature__)
SignatureBirth.__name__ = "Renamed"
print("signature after name", SignatureBirth.__text_signature__)
SignatureBirth.__name__ = "SignatureBirth"
print("signature after restore", SignatureBirth.__text_signature__)


class AbstractAllocation:
    pass


def allocate_abstract_candidate():
    return AbstractAllocation()


for _ in range(4):
    allocate_abstract_candidate()
AbstractAllocation.__abstractmethods__ = ("z", "a")
for construct in (
    allocate_abstract_candidate,
    lambda: object.__new__(AbstractAllocation),
):
    try:
        construct()
    except TypeError as error:
        print("abstract allocator", str(error))
AbstractAllocation.__abstractmethods__ = ()
print("abstract cache reset", type(allocate_abstract_candidate()).__name__)


class AbstractEscape:
    def __new__(cls, **kwargs):
        return 42


class AbstractInt(int):
    pass


class AbstractException(Exception):
    pass


for owner in (AbstractEscape, AbstractInt, AbstractException):
    owner.__abstractmethods__ = ("f",)
print("abstract custom new", AbstractEscape(field=1))
print("abstract int new", AbstractInt(3))
print("abstract exception new", str(AbstractException("value")))

# Dataclass construction reaches the same object allocator even when its
# fields use a specialized runtime representation.


@dataclass
class AbstractRecord:
    field: int


AbstractRecord.__abstractmethods__ = ("f",)
try:
    AbstractRecord(1)
except TypeError as error:
    print("abstract dataclass new", str(error))


doc_releases = []


class BirthDocString(str):
    def __del__(self):
        doc_releases.append("released")


class BirthDocLifetime:
    __doc__ = BirthDocString("BirthDocLifetime(arg)\n--\n\nOriginal.\0ignored")


BirthDocLifetime.__doc__ = "replacement"
print("birth doc releases original", doc_releases, BirthDocLifetime.__text_signature__)
TruncatedSignature = type(
    "TruncatedSignature", (), {"__doc__": "TruncatedSignature(arg)\0\n--\n\nDoc."}
)
print("birth doc nul boundary", TruncatedSignature.__text_signature__)
