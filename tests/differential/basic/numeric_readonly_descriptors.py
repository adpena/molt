"""Numeric members use builtin descriptors, including inherited payload reads."""

import math


class IntegerChild(int):
    def __int__(self):
        raise RuntimeError("unexpected __int__")

    def __index__(self):
        raise RuntimeError("unexpected __index__")


class FloatChild(float):
    def __float__(self):
        raise RuntimeError("unexpected __float__")


class ComplexChild(complex):
    def __complex__(self):
        raise RuntimeError("unexpected __complex__")


for owner, members, values in (
    (int, ("real", "imag", "numerator", "denominator"),
     (3, True, 10**50, IntegerChild(37), IntegerChild(10**50))),
    (float, ("real", "imag"), (1.25, -0.0, float("nan"), FloatChild(-0.0))),
    (complex, ("real", "imag"), (complex(-0.0, 2.5), ComplexChild(3, -0.0))),
):
    for member in members:
        descriptor = vars(owner)[member]
        assert descriptor.__objclass__ is owner
        assert descriptor.__get__(None, owner) is descriptor
        print("descriptor", owner.__name__, member, type(descriptor).__name__)
        for value in values:
            result = getattr(value, member)
            direct = descriptor.__get__(value, type(value))
            explicit = object.__getattribute__(value, member)
            if isinstance(result, float) and math.isnan(result):
                assert math.isnan(direct) and math.isnan(explicit)
                trace = "nan"
            else:
                assert result == direct == explicit
                trace = repr(result)
            assert type(result) is (int if owner is int else float)
            print("read", type(value).__name__, member, type(result).__name__, trace)
            if type(value) is owner and member in ("real", "numerator") and owner is not complex:
                assert result is value
            for operation in ("set", "delete"):
                try:
                    if operation == "set":
                        descriptor.__set__(value, 99)
                    else:
                        descriptor.__delete__(value)
                except AttributeError:
                    print("readonly", owner.__name__, member, operation)
                else:
                    raise AssertionError("numeric descriptor accepted mutation")
        try:
            descriptor.__get__(object(), object)
        except TypeError:
            print("wrong-receiver", owner.__name__, member)
        else:
            raise AssertionError("numeric descriptor admitted unrelated receiver")


class ShadowInteger(IntegerChild):
    real = "integer-shadow"


class ShadowFloat(FloatChild):
    real = "float-shadow"


class ShadowComplex(ComplexChild):
    real = "complex-shadow"


for owner, child, value, expected in (
    (int, ShadowInteger, 37, 37),
    (float, ShadowFloat, -0.0, -0.0),
    (complex, ShadowComplex, 3 + 4j, 3.0),
):
    instance = child(value)
    assert instance.real == child.real
    assert owner.real.__get__(instance, child) == expected
    print("shadow", owner.__name__, instance.real, owner.real.__get__(instance, child))

# An inherited data descriptor also wins against an instance dictionary entry.
for instance in (IntegerChild(9), FloatChild(2.5), ComplexChild(3, 4)):
    instance.__dict__["real"] = "instance-shadow"
    assert instance.real != "instance-shadow"
    print("data-precedence", type(instance).__name__, instance.real)

assert math.copysign(1.0, FloatChild(-0.0).real) == -1.0
assert math.copysign(1.0, FloatChild(-0.0).imag) == 1.0
assert math.copysign(1.0, ComplexChild(1, -0.0).imag) == -1.0
print("signed-zero", "preserved")
