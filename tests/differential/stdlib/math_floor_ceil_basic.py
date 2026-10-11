"""Purpose: differential coverage for math floor/ceil/trunc."""

import math
import warnings

print(math.floor(1.9))
print(math.ceil(1.1))
print(math.trunc(-1.9))


# CPython Modules/mathmodule.c calls a present rounding special method directly.
# A missing method and a descriptor/callback failure are different outcomes.
events = []
failure = RuntimeError("rounding callback failure")


class FloatOnly:
    def __float__(self):
        events.append("float")
        return 3.5


class NonCallable(FloatOnly):
    __floor__ = None
    __ceil__ = None
    __trunc__ = None


class FailingDescriptor:
    def __get__(self, instance, owner):
        events.append("descriptor")
        raise failure


class DescriptorFailure(FloatOnly):
    __floor__ = FailingDescriptor()
    __ceil__ = FailingDescriptor()
    __trunc__ = FailingDescriptor()


class CallbackFailure(FloatOnly):
    def fail(self):
        events.append("callback")
        raise failure

    __floor__ = fail
    __ceil__ = fail
    __trunc__ = fail


for operation in (math.floor, math.ceil, math.trunc):
    for value in (NonCallable(), DescriptorFailure(), CallbackFailure()):
        events.clear()
        try:
            operation(value)
        except Exception as exc:
            print(
                operation.__name__,
                type(value).__name__,
                type(exc).__name__,
                exc is failure,
                events,
            )
            assert "float" not in events
        else:
            raise AssertionError("rounding failure was ignored")

events.clear()
assert math.floor(FloatOnly()) == 3
assert math.ceil(FloatOnly()) == 4
assert events == ["float", "float"]
try:
    math.trunc(FloatOnly())
except TypeError as exc:
    print("trunc missing", str(exc), events)
else:
    raise AssertionError("trunc incorrectly used __float__")
assert events == ["float", "float"]

# A successful conversion must release its newly owned callback result.
# Suppressing the subclass warning keeps this a lifetime test, not a warning test.


class TemporaryFloat(float):
    def __del__(self):
        events.append("float result retired")


class FloatResult:
    def __float__(self):
        return TemporaryFloat(4.0)


with warnings.catch_warnings():
    warnings.simplefilter("ignore", DeprecationWarning)
    for operation in (math.sqrt, math.log):
        events.clear()
        answer = operation(FloatResult())
        assert math.isclose(answer, 2.0 if operation is math.sqrt else math.log(4.0))
        assert events == ["float result retired"]
        print(operation.__name__, events)

# Storage is not exact type admission: numeric subclasses can override these
# methods and may return any object without int normalization.
marker = object()


class IntegerRounding(int):
    def rounding(self):
        events.append("int rounding")
        return marker

    __floor__ = rounding
    __ceil__ = rounding
    __trunc__ = rounding


class FloatRounding(float):
    def rounding(self):
        events.append("float rounding")
        return marker

    __floor__ = rounding
    __ceil__ = rounding
    __trunc__ = rounding


for operation in (math.floor, math.ceil, math.trunc):
    for value in (IntegerRounding(3), FloatRounding(3.5)):
        events.clear()
        assert operation(value) is marker
        assert len(events) == 1
        print(operation.__name__, type(value).__name__, events)
    for value in (float("nan"), float("inf"), float("-inf")):
        try:
            operation(value)
        except (ValueError, OverflowError) as exc:
            expected = (
                "cannot convert float NaN to integer"
                if math.isnan(value) else "cannot convert float infinity to integer"
            )
            assert str(exc) == expected
            print(operation.__name__, type(exc).__name__, str(exc))
        else:
            raise AssertionError("nonfinite rounding did not fail")
