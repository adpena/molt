"""Generated comparison methods retain decoration-time fields and evaluation order."""

from dataclasses import dataclass, field
import sys


@dataclass(order=True)
class Record:
    value: int
    ignored: int = field(compare=False)


@dataclass(eq=False)
class Inherited(Record):
    extra: int = 0


left = Record(1, 90)
right = Record(2, 0)
same = Record(1, -90)
inherited_left = Inherited(1, 90, 100)
inherited_right = Inherited(2, 0, -100)
inherited_same = Inherited(1, -90, -100)

# Generated methods keep their original names and selected fields even after
# mutation of shared Field instances and deletion of the public metadata map.
Record.__dataclass_fields__["value"].compare = False
Record.__dataclass_fields__["value"].name = "ignored"
Record.__dataclass_fields__["ignored"].compare = True
Record.__dataclass_fields__.clear()
Inherited.__dataclass_fields__.clear()


def comparisons(label, first, second, equal):
    print(label, first == second, first < second, first <= second,
          first > second, first >= second, first == equal)


comparisons("record", left, right, same)
comparisons("inherited", inherited_left, inherited_right, inherited_same)
print("other class", Record.__eq__(left, inherited_left) is NotImplemented,
      Record.__lt__(left, inherited_left) is NotImplemented)


@dataclass(order=True)
class OwnEquality:
    value: int

    def __eq__(self, other):
        return "custom equality"


own_left = OwnEquality(1)
own_right = OwnEquality(2)
OwnEquality.__dataclass_fields__["value"].compare = False
OwnEquality.__dataclass_fields__.clear()
print("custom equality", own_left == own_right, own_left < own_right,
      own_left <= own_right, own_left > own_right, own_left >= own_right)

events = []
response = []


class Value:
    def __init__(self, name):
        self.name = name

    def __eq__(self, other):
        events.append(self.name)
        return response


@dataclass
class Chain:
    first: object
    second: object


chain_left = Chain(Value("left first"), Value("left second"))
chain_right = Chain(Value("right first"), Value("right second"))
raw = chain_left == chain_right
print("false result", raw is response if sys.version_info >= (3, 13) else raw is False,
      events)
events.clear()
response = [1]
raw = chain_left == chain_right
print("last result", raw is response if sys.version_info >= (3, 13) else raw is True,
      events)
events.clear()


class Temporary:
    def __init__(self, name):
        self.name = name

    def __eq__(self, other):
        events.append("equal " + self.name)
        return True

    def __del__(self):
        events.append("drop " + self.name)


@dataclass
class FailingFields:
    a: object
    b: object
    c: object

    def __getattribute__(self, name):
        if name in ("a", "b", "c"):
            side = object.__getattribute__(self, "side")
            events.append("get " + side + " " + name)
            if side == "left" and name == "c":
                raise ValueError("field getter")
            return Temporary(side + " " + name)
        return object.__getattribute__(self, name)


failing_left = FailingFields(None, None, None)
failing_right = FailingFields(None, None, None)
failing_left.side = "left"
failing_right.side = "right"
try:
    failing_left == failing_right
except ValueError as error:
    print("getter error", str(error), events)
