"""Constructor lifecycle and native subtype storage through public consumers."""

import gc
import math
import weakref


def raised(label, callback):
    try:
        callback()
    except Exception as error:
        print(label, type(error).__name__)
    else:
        print(label, "accepted")


families = [
    (list, [1, 2]), (dict, {"a": 1}), (set, (1, 2)),
    (frozenset, (1, 2)), (tuple, (1, 2)), (str, "ab"),
    (bytes, b"ab"), (bytearray, b"ab"), (int, 7),
    (float, 2.5), (complex, 2 + 3j),
]
events = []
for base, source in families:
    def initialize(self, value):
        events.append("init")
        if base in (list, dict, set, bytearray):
            base.__init__(self, value)
        self.marker = 7

    subtype = type("NativeChild", (base,), {"__init__": initialize})
    events.clear()
    instance = subtype(source)
    print("family", base.__name__, type(instance) is subtype, instance == base(source),
          instance.marker, events)
    events.clear()
    fresh = base.__new__(subtype, source)
    print("explicit-new", base.__name__, type(fresh) is subtype, events)
    raised("unsafe-new-" + base.__name__, lambda: object.__new__(subtype))


class Foreign(str):
    def __new__(cls, value):
        return 5

    def __init__(self, value):
        raise AssertionError("foreign result must skip init")


print("foreign-result", Foreign("x"))
events.clear()


class Parent(tuple):
    def __new__(cls, value):
        return tuple.__new__(Returned, value)

    def __init__(self, value):
        events.append("parent")


class Returned(Parent):
    def __init__(self, value):
        events.append("returned")


returned = Parent((4, 5))
print("actual-result-init", type(returned) is Returned, events)


class InitDescriptor:
    def __get__(self, instance, owner):
        def initialize(value):
            instance.marker = value
        return initialize


class DescriptorString(str):
    __init__ = InitDescriptor()


print("init-descriptor", DescriptorString("xy").marker)


class TupleChild(tuple):
    pass


class StringChild(str):
    pass


class BytesChild(bytes):
    pass


class FrozenChild(frozenset):
    pass


class ComplexChild(complex):
    pass


for base, subtype, value in [(tuple, TupleChild, ()), (str, StringChild, "ab"),
                             (bytes, BytesChild, b"ab"), (frozenset, FrozenChild, (1,)),
                             (complex, ComplexChild, 2 + 3j)]:
    exact = base(value)
    child = subtype(value)
    print("identity", base.__name__, base(exact) is exact, type(base(child)) is base,
          subtype(child) is child, type(exact) is base)

string = StringChild("ab")
string.marker = 9
string += "cd"
print("string-inplace", type(string) is str, string)


class AddString(str):
    def __add__(self, other):
        return "override:" + other


string = AddString("ab")
string += "cd"
print("string-inplace-override", string)


class Fields:
    def write(self):
        self.x = 1


class MixedString(Fields, str):
    pass


class MixedTuple(Fields, tuple):
    pass


for cls, source in [(MixedString, "x"), (MixedTuple, (1,))]:
    value = cls(source)
    value.write()
    print("inferred-dict", cls.__name__, vars(value), value.x)


class SlottedString(str):
    __slots__ = ("field", "__dict__")


class SiblingString(str):
    __slots__ = ("field", "__dict__")


for length in (0, 1, 4, 7, 8, 9):
    value = SlottedString("x" * length)
    value.field = -0.0
    value.extra = length
    value.__class__ = SiblingString
    print("aligned-slots", length, type(value) is SiblingString, len(value),
          math.copysign(1.0, value.field), vars(value))
    del value.field
    raised("deleted-slot", lambda: value.field)


class Plain:
    pass


plain = Plain()
raised("plain-to-native", lambda: setattr(plain, "__class__", StringChild))
print("plain-preserved", type(plain) is Plain)

finalized = []


def native_cycle(base, source):
    label = base.__name__

    def finish(self):
        finalized.append(label)

    cls = type("CyclicNative", (base,), {"__del__": finish})
    value = cls(source)
    value.me = value
    try:
        reference = weakref.ref(value)
    except TypeError:
        reference = None
    return reference


for base, source in families:
    reference = native_cycle(base, source)
    gc.collect()
    gc.collect()
    print("cycle", base.__name__, finalized.count(base.__name__),
          reference is None or reference() is None)


class MutableBytes(bytearray):
    pass


mutable = MutableBytes(b"old")
export = memoryview(mutable)
raised("export-reinit", lambda: mutable.__init__(b"new"))
print("export-preserved", bytes(mutable))
export.release()


def failing_items():
    yield 65
    raise RuntimeError("conversion failed")


raised("partial-bytearray", lambda: mutable.__init__(failing_items()))
print("partial-bytearray-value", bytes(mutable))
mutable.__init__(mutable)
print("self-bytearray", bytes(mutable))


class ReenterIndex:
    def __index__(self):
        mutable.append(66)
        return 0


mutable.__init__(ReenterIndex())
print("zero-index-reentry", bytes(mutable))
raised("range-bytearray", lambda: mutable.__init__([1 << 100]))
print("range-bytearray-length", len(mutable))


class MutableSet(set):
    pass


mutable_set = MutableSet((2,))
raised("partial-set", lambda: mutable_set.__init__(failing_items()))
print("partial-set-value", sorted(mutable_set))
mutable_set.__init__(mutable_set)
print("self-set", sorted(mutable_set))

events.clear()


class BytesAndIndex:
    def __bytes__(self):
        events.append("bytes")
        return BytesChild(b"ok")

    def __index__(self):
        events.append("index")
        return 4


converted = bytes(BytesAndIndex())
print("bytes-protocol", type(converted) is BytesChild, converted, events)


class HintFailure:
    def __iter__(self):
        events.append("iter")
        return iter((65,))

    def __length_hint__(self):
        events.append("hint")
        raise RuntimeError("hint failed")


events.clear()
raised("bytes-hint", lambda: bytes(HintFailure()))
print("bytes-hint-order", events)


class Keyword(str):
    pass


key = Keyword("key")
dictionary = dict(**{key: 9})
print("keyword-owner", next(iter(dictionary)) is key, dictionary[key])

for decode in (lambda: str(b"\xff", "utf-8"), lambda: b"\xff".decode(),
               lambda: bytearray(b"\xff").decode()):
    try:
        decode()
    except UnicodeDecodeError as error:
        print("decode-fields", error.encoding, error.object, error.start, error.end)


class Numeric:
    def __float__(self):
        events.append("float")
        return -0.0


events.clear()
raised("complex-second-admission", lambda: complex(Numeric(), object()))
print("complex-admission-order", events)
number = Numeric()
number.__complex__ = lambda: 9j
converted = complex(number)
print("complex-slot-lookup", converted.real, converted.imag, events)
for real, imag in [(-0.0, -0.0), (0.0, -0.0), (-0.0, 0.0)]:
    converted = complex(real, imag)
    print("complex-zero", math.copysign(1.0, converted.real), math.copysign(1.0, converted.imag))

raised("int-positional-only", lambda: int(x=3))
raised("int-bool-new", lambda: int.__new__(bool, 3))


# Representation changes must not bypass Python special-method dispatch.
def returns(label):
    def method(self, other):
        return label
    return method


for base, source in [(tuple, (1,)), (str, "a"), (bytes, b"a"), (bytearray, b"a")]:
    child = type("SequenceOperatorChild", (base,), {
        "__add__": returns("add"), "__radd__": returns("radd"),
        "__mul__": returns("mul"), "__rmul__": returns("rmul"),
    })(source)
    print("sequence-overrides", base.__name__, child + base(source), base(source) + child,
          child * 2, 2 * child)
    print("sequence-base-slots", base.__name__, base.__add__(child, base(source)) == base(source) + base(source),
          type(base.__mul__(child, 1)) is base, base.__mul__(child, 1) == base(source))


class ReflectedSequenceNumber:
    def __radd__(self, other):
        return "reflected-add"

    def __rmul__(self, other):
        return "reflected-mul"


for base, source in [(tuple, (1,)), (str, "a"), (bytes, b"a"), (bytearray, b"a")]:
    child = type("InheritedSequenceChild", (base,), {})(source)
    print("sequence-inherited-reflected", base.__name__, child + ReflectedSequenceNumber(),
          child * ReflectedSequenceNumber())


class BytearrayOperators(bytearray):
    __iadd__ = returns("iadd")
    __imul__ = returns("imul")


value = BytearrayOperators(b"x")
value += b"y"
other = BytearrayOperators(b"x")
other *= 2
print("bytearray-inplace-overrides", value, other)
value = BytearrayOperators(b"x")
print("bytearray-inplace-base", bytearray.__iadd__(value, b"y") is value,
      bytearray.__imul__(value, 2) is value, bytes(value))


for base in (set, frozenset):
    methods = {name: returns(name) for name in (
        "__or__", "__ror__", "__and__", "__rand__", "__sub__", "__rsub__", "__xor__", "__rxor__",
    )}
    child = type("SetOperatorChild", (base,), methods)((1, 2))
    other = base((2, 3))
    print("set-overrides", base.__name__, child | other, other | child, child & other,
          other & child, child - other, other - child, child ^ other, other ^ child)
    print("set-base-slots", base.__name__, sorted(base.__or__(child, other)),
          sorted(base.__and__(child, other)), sorted(base.__sub__(child, other)),
          sorted(base.__xor__(child, other)))


class SetInplace(set):
    __ior__ = returns("ior")
    __iand__ = returns("iand")
    __isub__ = returns("isub")
    __ixor__ = returns("ixor")


value = SetInplace((1,))
value |= {2}
print("set-ior", value)
value = SetInplace((1,))
value &= {1}
print("set-iand", value)
value = SetInplace((1,))
value -= {1}
print("set-isub", value)
value = SetInplace((1,))
value ^= {1}
print("set-ixor", value)


class InheritedBytearray(bytearray):
    pass


value = InheritedBytearray(b"a")
value.__iadd__ = returns("wrong-instance")
value += b"b"
print("inplace-ignores-instance", bytes(value))


class ComplexOperators(complex):
    __add__ = returns("add")
    __radd__ = returns("radd")
    __sub__ = returns("sub")
    __rsub__ = returns("rsub")
    __mul__ = returns("mul")
    __rmul__ = returns("rmul")
    __truediv__ = returns("div")
    __rtruediv__ = returns("rdiv")
    __pow__ = returns("pow")
    __rpow__ = returns("rpow")

    def __neg__(self):
        return "neg"

    def __pos__(self):
        return "pos"

    def __abs__(self):
        return "abs"


value = ComplexOperators(2 + 3j)
print("complex-overrides", value + 1j, 1j + value, value - 1j, 1j - value,
      value * 1j, 1j * value, value / 1j, 1j / value, value ** 2, 2 ** value,
      -value, +value, abs(value))
print("complex-base-slots", complex.__add__(value, 1j), complex.__sub__(value, 1j),
      complex.__mul__(value, 1j), complex.__truediv__(value, 1j),
      complex.__pow__(value, 2), complex.__neg__(value), complex.__pos__(value))


class ModString(str):
    __mod__ = returns("mod")


print("string-mod-override", ModString("%s") % "value", str.__mod__(ModString("%s"), "value"))


def always_false(self):
    return False


for base, source in families:
    child = type("FalseNativeChild", (base,), {"__bool__": always_false})(source)
    print("truth-override", base.__name__, bool(child))
for base in (float, complex):
    child = type("InheritedNumericTruth", (base,), {})
    print("truth-inherited", base.__name__, bool(child(0)), bool(child(2)))


def typed_str_len(value: str):
    return len(value)


def typed_tuple_len(value: tuple):
    return len(value)


def typed_dict_len(value: dict):
    return len(value)


def typed_set_len(value: set):
    return len(value)


def typed_frozen_len(value: frozenset):
    return len(value)


def typed_bytes_len(value: bytes):
    return len(value)


def typed_bytearray_len(value: bytearray):
    return len(value)


def overridden_length(self):
    return 37


for base, source, length in [(str, "abc", typed_str_len), (tuple, (1,), typed_tuple_len),
                              (dict, {"a": 1}, typed_dict_len), (set, (1,), typed_set_len),
                              (frozenset, (1,), typed_frozen_len), (bytes, b"a", typed_bytes_len),
                              (bytearray, b"a", typed_bytearray_len)]:
    child = type("LengthNativeChild", (base,), {"__len__": overridden_length})(source)
    print("typed-length", base.__name__, length(child), base.__len__(child))


# __len__ shares one result protocol across ordinary len, truth and bisect.
import bisect
import sys

class IndexLength:
    def __index__(self):
        return 3

class ProtocolLength(list):
    def __len__(self):
        return IndexLength()

value = ProtocolLength([1, 2, 4])
print("index-length", len(value), bool(value), bisect.bisect_left(value, 3))
for invalid in (-1, -(1 << 100), sys.maxsize + 1, 1 << 100, 1.5):
    class InvalidLength(list):
        def __len__(self):
            return invalid
    value = InvalidLength([1])
    for consumer in (len, bool):
        try:
            consumer(value)
        except Exception as error:
            print("invalid-length", consumer.__name__, type(error).__name__, str(error))

for constructor, source in ((str, b"x"), (bytes, "x"), (bytearray, "x")):
    for keyword in ("encoding", "errors"):
        for name in ("x\ud800\ud801", "x\0", "\0\ud800"):
            try:
                constructor(source, **{keyword: name})
            except Exception as error:
                print("codec-argument", constructor.__name__, keyword,
                      type(error).__name__, str(error))
