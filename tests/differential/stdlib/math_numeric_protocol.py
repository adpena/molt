"""Shared math conversion: storage, protocol dispatch, warnings, and lifetime."""

import math
import warnings


events = []
failure = RuntimeError("numeric descriptor failure")


class DescriptorFailure:
    def __get__(self, instance, owner):
        events.append("descriptor")
        raise failure


class FailingFloat:
    __float__ = DescriptorFailure()

    def __index__(self):
        raise AssertionError("failed __float__ must not fall through")


for operation in (math.sqrt, math.log, math.log2, math.log10, math.isfinite):
    events.clear()
    try:
        operation(FailingFloat())
    except RuntimeError as exc:
        assert exc is failure
        assert events == ["descriptor"]
        print(operation.__name__, "descriptor identity", events)
    else:
        raise AssertionError("descriptor error was lost")


class IntegerOverride(int):
    def __float__(self):
        events.append("integer float")
        return 4.0

    def __index__(self):
        raise AssertionError("actual integer storage must bypass __index__")


large = IntegerOverride(1 << 1200)
events.clear()
assert math.sqrt(large) == 2.0
assert events == ["integer float"]
events.clear()
assert math.log2(large) == 1200.0
assert math.isclose(math.log(large), 1200 * math.log(2.0))
assert math.isclose(math.log10(large), 1200 * math.log10(2.0))
assert events == []
print("actual integer log", math.log2(large), events)


class FloatOverride(float):
    def __float__(self):
        raise AssertionError("PyFloat_AsDouble reads float subtype storage")


assert math.sqrt(FloatOverride(4.0)) == 2.0


class FloatResult(float):
    def __del__(self):
        events.append("float result released")


class FloatProducer:
    def __float__(self):
        events.append("float called")
        return FloatResult(4.0)


class IndexResult(int):
    def __del__(self):
        events.append("index result released")


class IndexProducer:
    def __index__(self):
        events.append("index called")
        return IndexResult(4)


for producer, kind in ((FloatProducer, "float"), (IndexProducer, "index")):
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always", DeprecationWarning)
        events.clear()
        assert math.sqrt(producer()) == 2.0
        assert len(caught) == 1 and caught[0].category is DeprecationWarning
        assert events == [kind + " called", kind + " result released"]
        print(kind, "warning", events)
    with warnings.catch_warnings():
        warnings.simplefilter("error", DeprecationWarning)
        events.clear()
        try:
            math.sqrt(producer())
        except DeprecationWarning:
            assert events == [kind + " called", kind + " result released"]
            print(kind, "warning error", events)
        else:
            raise AssertionError("subtype warning was suppressed")


class HugeIndex:
    def __index__(self):
        return IndexResult(1 << 1200)


with warnings.catch_warnings():
    warnings.simplefilter("ignore", DeprecationWarning)
    for operation in (math.sqrt, math.log, math.log2, math.log10, math.isfinite):
        events.clear()
        try:
            operation(HugeIndex())
        except OverflowError as exc:
            assert str(exc) == "int too large to convert to float"
            assert events == ["index result released"]
            print(operation.__name__, "index overflow", events)
        else:
            raise AssertionError("protocol integer incorrectly used arbitrary-int log")


class Argument:
    def __init__(self, name, value):
        self.name = name
        self.value = value

    def __float__(self):
        events.append(self.name)
        return self.value


events.clear()
assert math.isclose(
    Argument("a", 1.0), Argument("b", 1.0),
    rel_tol=Argument("rel", 0.0), abs_tol=Argument("abs", 0.0),
)
assert events == ["a", "b", "rel", "abs"]
print("isclose order", events)


class Item:
    def __float__(self):
        events.append("item conversion")
        raise failure

    def __del__(self):
        events.append("item released")


class Items:
    def __iter__(self):
        return Cursor()


class Cursor:
    def __next__(self):
        return Item()

    def __del__(self):
        events.append("iterator released")


events.clear()
try:
    math.fsum(Items())
except RuntimeError as exc:
    assert exc is failure
# A traceback owns the failing conversion frame until it is cleared.
failure.__traceback__ = None
assert events == ["item conversion", "iterator released", "item released"]
print("fsum conversion lifetime", events)
