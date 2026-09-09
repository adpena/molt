"""Rich result identity, reflected dispatch, and explicit truth consumers.

Output is address-free and portable. CPython oracle runs do not attest either
compiled target; replay this same source on native and WASM.
"""

events = []


class Result:
    def __init__(self, name, truth=False, raises=False):
        self.name = name
        self.truth = truth
        self.raises = raises

    def __bool__(self):
        events.append("truth:" + self.name)
        if self.raises:
            raise ValueError("comparison truth")
        return self.truth


class Compared:
    def __init__(self, name, decline=False, raises=False):
        self.name = name
        self.result = Result(name)
        self.decline = decline
        self.raises = raises

    def compare(self, other, symbol):
        events.append(self.name + symbol)
        if self.raises:
            raise ValueError("comparison callback")
        return NotImplemented if self.decline else self.result

    def __eq__(self, other):
        return self.compare(other, "==")

    def __ne__(self, other):
        return self.compare(other, "!=")

    def __lt__(self, other):
        return self.compare(other, "<")

    def __le__(self, other):
        return self.compare(other, "<=")

    def __gt__(self, other):
        return self.compare(other, ">")

    def __ge__(self, other):
        return self.compare(other, ">=")


class Child(Compared):
    pass


def eq(a, b):
    return a == b


def ne(a, b):
    return a != b


def lt(a, b):
    return a < b


def le(a, b):
    return a <= b


def gt(a, b):
    return a > b


def ge(a, b):
    return a >= b


operations = (("eq", eq), ("ne", ne), ("lt", lt), ("le", le), ("gt", gt), ("ge", ge))


def report(label, operation, left, right):
    events.clear()
    try:
        result = operation(left, right)
        if isinstance(result, Result):
            print(label, "result", result.name, events)
        else:
            print(label, "boolean", result, events)
    except (ValueError, TypeError) as error:
        print(label, "error", type(error).__name__, events)


for name, operation in operations:
    left = Compared("left")
    right = Compared("right")
    report(name + ":value", operation, left, right)
    report(name + ":same", operation, left, left)
    report(name + ":none-left", operation, None, right)
    report(name + ":none-right", operation, left, None)
    report(name + ":subtype", operation, left, Child("child"))
    report(name + ":reflected", operation, Compared("decline", decline=True), right)
    report(
        name + ":both-decline",
        operation,
        Compared("a", decline=True),
        Compared("b", decline=True),
    )
    report(name + ":raises", operation, Compared("raises", raises=True), right)

    # Sequence equality consumes truth; ordering returns the selected element's
    # rich result unchanged after testing element equality. The same-element
    # shortcut belongs to container equality, not to a scalar comparison.
    report(name + ":list", operation, [left], [right])
    report(name + ":tuple", operation, (left,), (right,))
    report(name + ":same-list-element", operation, [left], [left])
    report(name + ":same-tuple-element", operation, (left,), (left,))
    left.result.raises = True
    report(name + ":unconsumed-truth", operation, left, right)
    report(name + ":sequence-truth-error", operation, [left], [right])


class DefaultNe:
    def __eq__(self, other):
        events.append("default-eq")
        return Result("default")


report("default-ne", ne, DefaultNe(), DefaultNe())


def discarded_equal(left, right):
    left == right
    left == right
    return True


def repeated_equal(left, right):
    first = left == right
    second = left == right
    return first is second


report("discarded-callbacks", discarded_equal, Compared("left"), Compared("right"))
report("repeated-callbacks", repeated_equal, Compared("left"), Compared("right"))


# Rich comparison values become booleans only at explicit consumers. Errors
# and owned temporaries must survive each consumer's early-exit path.
from operator import countOf


def count_equal(left, right):
    return countOf([left, left], right)


for raises in (False, True):
    left = Compared("consumer")
    right = Compared("other")
    left.result.raises = raises
    report("slice-truth:" + str(raises), eq, slice(left), slice(right))
    report("count-truth:" + str(raises), count_equal, left, right)
    report("count-identity:" + str(raises), count_equal, left, left)


class Container:
    def __init__(self, result):
        self.result = result

    def __contains__(self, item):
        events.append("contains")
        return self.result


def discarded_bool(value):
    bool(value)


def discarded_not(value):
    not value


def discarded_double_not(value):
    not not value


def discarded_in(value):
    0 in Container(value)


def discarded_not_in(value):
    0 not in Container(value)


for name, operation in (
    ("bool", discarded_bool),
    ("not", discarded_not),
    ("double-not", discarded_double_not),
    ("in", discarded_in),
    ("not-in", discarded_not_in),
):
    for raises in (False, True):
        events.clear()
        try:
            operation(Result("discarded", raises=raises))
            outcome = "returned"
        except ValueError:
            outcome = "ValueError"
        print("discarded:" + name, raises, outcome, events)
