"""Purpose: differential coverage for __index__ and __round__."""

class Index:
    def __index__(self):
        return 3


class Round:
    def __round__(self, ndigits=None):
        return ("round", ndigits)


if __name__ == "__main__":
    data = [0, 1, 2, 3, 4]
    print("index", data[Index()])
    print("round0", round(Round()))
    print("round2", round(Round(), 2))


# Observe builtin slots and digit conversion separately from custom __round__.
events = []
index_failure = ValueError("index-failure")


class Digits:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        events.append("index")
        return self.value


class IndexFailure:
    def __index__(self):
        events.append("index-failure")
        raise index_failure


def observe_round(label, operation, *args):
    events.clear()
    try:
        result = operation(*args)
        shown = result.hex() if isinstance(result, float) else result
        print(label, type(result).__name__, shown, events)
    except Exception as exc:
        print(label, type(exc).__name__, str(exc), exc is index_failure, events)


for value in (125, -(10 ** 40 + 15), 3.25):
    observe_round("index", round, value, Digits(-1))
    observe_round("invalid-string", round, value, "2")
    observe_round("invalid-float", round, value, 1.0)
    observe_round("invalid-result", round, value, Digits("2"))
    observe_round("callback-error", round, value, IndexFailure())
    observe_round("none", round, value, None)
    observe_round("bool-index", round, value, True)
    observe_round("huge-positive", round, value, Digits(10 ** 100))

for value in (15, 25, -15, -25, 10 ** 40 + 15, -(10 ** 40 + 15)):
    observe_round("integer-tie", round, value, -1)
    observe_round("integer-zero", round, value, -50)
observe_round("integer-large-admitted-negative", round, 12345, Digits(-1_000_000))

for value, digits in ((25.1, -1), (-25.1, -1), (24.9, -1), (-24.9, -1),
                      (25.0, -1), (-25.0, -1), (5e-324, 323), (-5e-324, 323),
                      (5e-324, 324), (-0.5, 0), (1.7e308, -308)):
    observe_round("float-boundary", round, value, Digits(digits))
for value in (1.25, -1.25, float("inf"), float("nan")):
    observe_round("float-huge-negative", round, value, Digits(-(10 ** 100)))
    observe_round("nonfinite-index-first", round, value, IndexFailure())

observe_round("int-descriptor-omitted", int.__round__, 125)
observe_round("int-descriptor-none", int.__round__, 125, None)
observe_round("float-descriptor-omitted", float.__round__, 1.5)
observe_round("float-descriptor-none", float.__round__, 1.5, None)


class CustomRound:
    def __round__(self, *args):
        events.append("round")
        return len(args), bool(args and isinstance(args[0], Digits))

    def __getattribute__(self, name):
        if name == "__round__":
            raise AssertionError("instance lookup must not run")
        return object.__getattribute__(self, name)


custom = CustomRound()
custom.__round__ = lambda *args: "wrong-instance"
observe_round("custom-omitted", round, custom)
observe_round("custom-none", round, custom, None)
observe_round("custom-original-argument", round, custom, Digits("invalid-but-not-converted"))


class IntRound(int):
    def __round__(self, *args):
        events.append("int-override")
        return 77


class FloatRound(float):
    def __round__(self, *args):
        events.append("float-override")
        return 88


for base, value in ((int, IntRound(125)), (float, FloatRound(1.25))):
    observe_round("subtype-override", round, value, IndexFailure())
    observe_round("explicit-base", base.__round__, value, Digits(-1))
    inherited = type("InheritedRound", (base,), {})(value)
    observe_round("inherited-slot", round, inherited, Digits(-1))


class RoundDescriptor:
    def __get__(self, obj, owner):
        events.append("round-descriptor")
        raise index_failure


class BrokenRound:
    __round__ = RoundDescriptor()


observe_round("descriptor-error", round, BrokenRound(), Digits(1))
observe_round("missing-method", round, object(), Digits(1))
