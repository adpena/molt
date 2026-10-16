"""Purpose: differential coverage for struct pack_into/unpack_from/iter_unpack."""

import gc
import operator
import struct
import sys

buf = bytearray(8)
struct.pack_into("ii", buf, 0, 1, 2)
print(struct.unpack_from("ii", buf, 0))
print(list(struct.iter_unpack("i", buf)))


# Public callback and buffer observations, independent of the runtime codec.
def resize_probe(data):
    try:
        data.append(255)
    except BufferError:
        return "pinned"
    data.pop()
    return "released"


class Index:
    def __init__(self, label, data, events, result=0, failure=None):
        self.label = label
        self.data = data
        self.events = events
        self.result = result
        self.failure = failure

    def __index__(self):
        self.events.append((self.label, bytes(self.data), resize_probe(self.data)))
        if self.failure is not None:
            raise self.failure
        return self.result


class Truth:
    def __init__(self, data, events, failure=None):
        self.data = data
        self.events = events
        self.failure = failure

    def __bool__(self):
        self.events.append(("bool", bytes(self.data), resize_probe(self.data)))
        if self.failure is not None:
            raise self.failure
        self.data[-1] = 77
        return True


class Float:
    def __init__(self, data, events, failure=None):
        self.data = data
        self.events = events
        self.failure = failure

    def __float__(self):
        self.events.append(("float", bytes(self.data), resize_probe(self.data)))
        if self.failure is not None:
            raise self.failure
        return 1.5


for method in (False, True):
    data = bytearray(b"abcdefgh")
    events = []
    args = (
        data,
        Index("offset", data, events),
        Index("value", data, events, 9),
        Truth(data, events),
        Float(data, events),
    )
    if method:
        struct.Struct("<B?f2x").pack_into(*args)
    else:
        struct.pack_into("<B?f2x", *args)
    print("ordered", method, events, bytes(data), resize_probe(data))

for role in ("offset", "index", "bool", "float"):
    data = bytearray(b"abcdefgh")
    events = []
    failure = ValueError(role)
    offset = Index(
        "offset", data, events, failure=failure if role == "offset" else None
    )
    values = (
        Index("value", data, events, 9, failure if role == "index" else None),
        Truth(data, events, failure if role == "bool" else None),
        Float(data, events, failure if role == "float" else None),
    )
    try:
        struct.pack_into("<B?f2x", data, offset, *values)
    except Exception as exc:
        print(
            "callback-failure",
            role,
            type(exc).__name__,
            type(exc) is struct.error,
            exc is failure,
            events,
            bytes(data),
            resize_probe(data),
        )

# OverflowError is deliberately translated by narrow/native integer converters,
# but a standard q/Q __index__ failure is preserved.
for fmt in ("B", "@q", ">q", ">Q", "P"):
    data = bytearray(struct.calcsize(fmt))
    events = []
    failure = OverflowError("retained index failure")
    try:
        struct.pack_into(fmt, data, 0, Index("index", data, events, failure=failure))
    except Exception as exc:
        print(
            "index-overflow",
            fmt,
            type(exc).__name__,
            type(exc) is struct.error,
            exc is failure,
            len(events),
            resize_probe(data),
        )

for operation in ("pack_into", "unpack_from"):
    for offset_value in (sys.maxsize + 1, -sys.maxsize - 2):
        data = bytearray(b"a")
        events = []
        offset = Index("offset", data, events, offset_value)
        try:
            if operation == "pack_into":
                struct.pack_into("B", data, offset, 1)
            else:
                struct.unpack_from("B", data, offset)
        except Exception as exc:
            print(
                "offset-overflow",
                operation,
                offset_value > 0,
                type(exc).__name__,
                len(events),
                bytes(data),
                resize_probe(data),
            )

# Count checks precede buffer admission and offset/value conversion.
data = bytearray(b"a")
events = []
try:
    struct.pack_into(
        "BB", data, Index("offset", data, events), Index("value", data, events, 1)
    )
except Exception as exc:
    print(
        "count-first",
        type(exc) is struct.error,
        events,
        bytes(data),
        resize_probe(data),
    )

for target in (b"ab", memoryview(bytearray(b"abcd"))[::2]):
    events = []
    data = bytearray(b"a")
    try:
        struct.pack_into("B", target, Index("offset", data, events), 1)
    except Exception as exc:
        print("write-admission", type(exc).__name__, events)
    if isinstance(target, memoryview):
        target.release()

# The zeroing is visible to an aliased s/p source; this cannot use prepacked bytes.
for fmt in ("4s", "4p"):
    data = bytearray(b"abcd")
    struct.pack_into(fmt, data, 0, data)
    print("alias", fmt, bytes(data), resize_probe(data))

# Each next reads current storage. The last yielded tuple still owns the export;
# only a subsequent exhaustion call (or destruction) releases it.
for method in (False, True):
    data = bytearray(b"\x01\x02")
    iterator = (
        struct.Struct("B").iter_unpack(data)
        if method
        else struct.iter_unpack("B", data)
    )
    print(
        "iterator-start",
        method,
        iter(iterator) is iterator,
        operator.length_hint(iterator),
        resize_probe(data),
    )
    print("iterator-first", next(iterator), operator.length_hint(iterator))
    data[1] = 9
    print(
        "iterator-mutation",
        next(iterator),
        operator.length_hint(iterator),
        resize_probe(data),
    )
    print(
        "iterator-exhaust",
        next(iterator, "end"),
        next(iterator, "end"),
        operator.length_hint(iterator),
        resize_probe(data),
    )

for empty in (False, True):
    data = bytearray() if empty else bytearray(b"a")
    iterator = struct.iter_unpack("B", data)
    print("iterator-delete-before", empty, resize_probe(data))
    del iterator
    gc.collect()
    print("iterator-delete-after", empty, resize_probe(data))

data = bytearray()
iterator = struct.iter_unpack("B", data)
print("empty-before", operator.length_hint(iterator), resize_probe(data))
print("empty-after", next(iterator, "end"), resize_probe(data))
for fmt, data in (("0s", bytearray()), ("H", bytearray(b"a"))):
    try:
        struct.iter_unpack(fmt, data)
    except Exception as exc:
        print("iterator-refusal", fmt, type(exc) is struct.error, resize_probe(data))


# Managed exporters expose the exact flags, callback order and cycle edge.
class Exporter:
    def __init__(self, data, events, failure=None):
        self.data = data
        self.events = events
        self.failure = failure
        self.iterator = None

    def __buffer__(self, flags):
        self.events.append(("acquire", int(flags)))
        if self.failure is not None:
            raise self.failure
        return memoryview(self.data)

    def __release_buffer__(self, view):
        self.events.append(("release", bytes(view)))


for operation in ("pack_into", "unpack", "unpack_from", "iter_unpack"):
    data = bytearray(b"ab")
    events = []
    exporter = Exporter(data, events)
    if operation == "pack_into":
        struct.pack_into("B", exporter, Index("offset", data, events), 7)
    elif operation == "unpack":
        print("export-unpack", struct.unpack("BB", exporter))
    elif operation == "unpack_from":
        print(
            "export-unpack-from",
            struct.unpack_from("B", exporter, Index("offset", data, events)),
        )
    else:
        iterator = struct.iter_unpack("B", exporter)
        print("export-iterator-first", next(iterator), list(events))
        del iterator
    print("export", operation, events, resize_probe(data))

for operation in ("pack_into", "unpack", "iter_unpack"):
    events = []
    failure = LookupError("export refusal")
    exporter = Exporter(bytearray(b"a"), events, failure)
    try:
        if operation == "pack_into":
            struct.pack_into("B", exporter, 0, 1)
        elif operation == "unpack":
            struct.unpack("B", exporter)
        else:
            struct.iter_unpack("B", exporter)
    except Exception as exc:
        print("export-failure", operation, type(exc).__name__, exc is failure, events)

cycle_events = []


class CycleExporter(Exporter):
    def __release_buffer__(self, view):
        # GC may already be clearing the instance dictionary. The observation
        # belongs to the external test log, not an edge inside the cycle.
        cycle_events.append(("release", bytes(view)))


events = cycle_events
data = bytearray(b"a")
exporter = CycleExporter(data, events)
iterator = struct.iter_unpack("B", exporter)
exporter.iterator = iterator
print("cycle-before", resize_probe(data), events)
del iterator
del exporter
gc.collect()
print("cycle-after", resize_probe(data), events)

# The concrete native iterator is immutable, non-subclassable and not publicly
# constructible. These are public type properties, not private declaration mocks.
iterator = struct.iter_unpack("B", b"a")
iterator_type = type(iterator)
try:
    iterator_type()
except Exception as exc:
    print("iterator-construction", type(exc).__name__)
try:
    iterator_type.marker = 1
except Exception as exc:
    print("iterator-type-write", type(exc).__name__)
try:

    class DerivedIterator(iterator_type):
        pass
except Exception as exc:
    print("iterator-subclass", type(exc).__name__)
del iterator


# Semantic class inquiry is separate from native layout admission. Neither
# resolve_bases nor prepare_class may reject a real non-subclassable class;
# the default metaclass rejects only after the body has run.
import types

native_bases = (iterator_type,)
print("iterator-resolve", types.resolve_bases(native_bases) is native_bases)
prepared_meta, prepared_namespace, prepared_keywords = types.prepare_class(
    "PreparedIterator", native_bases
)
print("iterator-prepare", prepared_meta is type, prepared_namespace, prepared_keywords)
construction_events = []
try:

    class OrderedIterator(iterator_type):
        construction_events.append("body")
except TypeError as exc:
    print("iterator-subclass-order", construction_events, str(exc))

construction_events = []
try:
    types.new_class(
        "FactoryIterator", native_bases,
        exec_body=lambda namespace: construction_events.append("body"),
    )
except TypeError as exc:
    print("iterator-new-class-order", construction_events, str(exc))

actual_type = type
try:
    actual_type("DirectIterator", native_bases, {})
except TypeError as exc:
    print("iterator-type-alias", str(exc))

sentinel = object()
construction_events = []


def native_base_factory(name, bases, namespace):
    construction_events.append(("factory", name, bases[0] is iterator_type, namespace["marker"]))
    return sentinel


class FactoryResult(iterator_type, metaclass=native_base_factory):
    construction_events.append("body")
    marker = 42


print("iterator-custom-metaclass", FactoryResult is sentinel, construction_events)


class ExistingFactoryType:
    marker = "unchanged"


def existing_type_factory(name, bases, namespace):
    return ExistingFactoryType


class ExistingFactoryResult(iterator_type, metaclass=existing_type_factory):
    marker = "discarded"


def null_factory(name, bases, namespace):
    return None


class NullFactoryResult(iterator_type, metaclass=null_factory):
    def method(self):
        return __class__


print("iterator-existing-metaclass", ExistingFactoryResult is ExistingFactoryType,
      ExistingFactoryType.marker)
print("iterator-null-metaclass-classcell", NullFactoryResult is None)


# get_original_bases consults the owned namespace, not an inherited marker,
# and evaluates __bases__ even when the namespace already contains the key.
class OriginalBase:
    __orig_bases__ = ("original-marker",)


class OriginalChild(OriginalBase):
    pass


print("iterator-original-bases", types.get_original_bases(iterator_type) == (object,))
print("original-bases-own", types.get_original_bases(OriginalBase))
print("original-bases-inherited", types.get_original_bases(OriginalChild) == (OriginalBase,))
original_events = []
original_failure = LookupError("original bases callback")


class OriginalNamespace:
    def __getattribute__(self, name):
        if name == "get":
            original_events.append("get-attribute")
        return object.__getattribute__(self, name)

    def get(self, name, default):
        original_events.append(("get", name, default))
        return ("namespace-marker",)


class OriginalMeta(type):
    def __getattribute__(cls, name):
        if name == "__dict__":
            original_events.append("namespace")
            return OriginalNamespace()
        if name == "__bases__":
            original_events.append("bases")
            if original_refuse:
                raise original_failure
            return ("base-default",)
        return super().__getattribute__(name)


class OriginalSubject(metaclass=OriginalMeta):
    pass


original_refuse = False
print("original-bases-protocol", types.get_original_bases(OriginalSubject), original_events)
original_events.clear()
original_refuse = True
try:
    types.get_original_bases(OriginalSubject)
except LookupError as exc:
    print("original-bases-error", exc is original_failure, original_events)


class NamespaceProtocol:
    @property
    def __dict__(self):
        return {"__orig_bases__": ("duck-marker",)}

    @property
    def __bases__(self):
        return ("duck-default",)


print("original-bases-duck", types.get_original_bases(NamespaceProtocol()))
original_attribute_error = AttributeError("original bases attribute")


class MissingNamespace:
    @property
    def __dict__(self):
        raise original_attribute_error


try:
    types.get_original_bases(MissingNamespace())
except TypeError as exc:
    print(
        "original-bases-attribute-error", str(exc),
        exc.__context__ is original_attribute_error, exc.__cause__, exc.__suppress_context__,
    )
