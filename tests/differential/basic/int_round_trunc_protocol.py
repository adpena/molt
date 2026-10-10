"""Purpose: differential coverage for int round trunc protocol."""

import math


class IntOnly:
    def __int__(self) -> int:
        return 7


class IndexOnly:
    def __index__(self) -> int:
        return 9


class BadInt:
    def __int__(self):
        return "x"


class Roundy:
    def __round__(self, ndigits=None):
        return 123 if ndigits is None else 456


class Truncy:
    def __trunc__(self):
        return 11


class BadTrunc:
    def __trunc__(self):
        return "x"


def main() -> None:
    print(int(IntOnly()))
    print(int(IndexOnly()))
    try:
        int(BadInt())
    except TypeError:
        print("TypeError")
    print(round(Roundy()))
    print(round(Roundy(), 2))
    print(math.trunc(Truncy()))
    print(math.trunc(BadTrunc()))
    print(int("  123 "))
    print(int(b"11", 2))
    print(int("0x10", 0))
    print(int(True))
    try:
        int(1.2, 10)
    except TypeError:
        print("TypeError")
    try:
        int("x", 2)
    except ValueError:
        print("ValueError")
    try:
        int(float("nan"))
    except ValueError:
        print("ValueError")
    try:
        int(float("inf"))
    except OverflowError:
        print("OverflowError")


if __name__ == "__main__":
    main()


# Constructor protocols precede subtype storage and text, while explicit base
# descriptors and operator.index read integer payloads without those overrides.
import operator
import warnings

events = []


class IntOverride(int):
    def __int__(self):
        events.append("int")
        return 99

    def __index__(self):
        events.append("index")
        return 77


class FloatOverride(float):
    def __int__(self):
        events.append("float-int")
        return 31


class ComplexOverride(complex):
    def __int__(self):
        events.append("complex-int")
        return 32


class TextOverride(str):
    def __int__(self):
        events.append("text-int")
        return 33


for item in [IntOverride(42), IntOverride(2**100 + 3), FloatOverride(1.25),
             ComplexOverride(1, 2), TextOverride("123")]:
    events.clear()
    result = int(item)
    print("constructor", type(item).__name__, result, type(result) is int, events)
for payload in [42, 2**100 + 3]:
    item = IntOverride(payload)
    events.clear()
    print("base-payload", int.__int__(item) == payload,
          int.__index__(item) == payload, operator.index(item) == payload, events)


class ReturnedInt:
    result = None

    def __int__(self):
        events.append("return-int")
        return self.result

    def __index__(self):
        raise AssertionError("invalid __int__ must not fall through to __index__")


for returned in [True, IntOverride(2**100 + 3), 1.0, "2"]:
    ReturnedInt.result = returned
    for action in ["ignore", "error"]:
        events.clear()
        with warnings.catch_warnings(record=True):
            warnings.simplefilter(action, DeprecationWarning)
            try:
                result = int(ReturnedInt())
                print("slot-result", type(returned).__name__, action, result,
                      type(result) is int, events)
            except (TypeError, DeprecationWarning) as error:
                print("slot-result", type(returned).__name__, action,
                      type(error).__name__, str(error), events)


class TruncOnly:
    def __trunc__(self):
        events.append("trunc")
        return IndexOnly()


for action in ["ignore", "error"]:
    events.clear()
    with warnings.catch_warnings(record=True):
        warnings.simplefilter(action, DeprecationWarning)
        try:
            print("trunc-delegation", action, int(TruncOnly()), events)
        except (TypeError, DeprecationWarning) as error:
            print("trunc-delegation", action, type(error).__name__, str(error), events)


class FailedBase:
    def __index__(self):
        raise LookupError("base callback failure")


try:
    int("123", FailedBase())
except LookupError as error:
    print("base-failure", str(error))
