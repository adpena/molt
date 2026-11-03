"""Purpose: HF-203 decorators use captured binding identities and Python order."""

from dataclasses import dataclass as record, is_dataclass
import dataclasses as records
from typing import overload as declared, get_overloads, clear_overloads


def dataclass(cls):
    cls.marker = "custom"
    return cls


@dataclass
class Plain:
    value: int


print(is_dataclass(Plain), Plain.marker, Plain.__repr__ is object.__repr__)
try:
    Plain(3)
except TypeError:
    print("plain takes no fields")


def overload(fn):
    return fn


@overload
def ordinary():
    return 7


def kernel(fn):
    return fn


@kernel
def scalar():
    return 11


print(ordinary(), scalar())

original = records.dataclass
events = []


def replacement(cls):
    events.append("replacement")
    return cls


@record
class Captured:
    records.dataclass = replacement
    value: int


records.dataclass = original
print(Captured(13), events)

options = {"repr": False}


@record(**options)
class Options:
    options["repr"] = True
    value: int


print(Options.__repr__ is object.__repr__, Options(17).value)


class ClassVar:
    @classmethod
    def __class_getitem__(cls, parameter):
        return int


@record
class ActualField:
    value: ClassVar[int]


try:
    ActualField()
except TypeError:
    print("custom annotation is a required field")
print(ActualField(19).value)


class InitVar:
    __molt_initvar__ = True

    @classmethod
    def __class_getitem__(cls, parameter):
        return cls()


class _KW_ONLY_TYPE:
    __molt_kw_only__ = True


@record
class MarkerLookalikes:
    first: InitVar[int]
    second: _KW_ONLY_TYPE()


lookalikes = MarkerLookalikes(23, 29)
print("marker lookalikes", lookalikes.first, lookalikes.second,
      sorted(vars(lookalikes)))

clear_overloads()
headers = []


@declared
def standalone(value=headers.append("header")):
    return value


print(callable(standalone), headers)
try:
    standalone()
except NotImplementedError:
    print("overload dummy")


@declared
def choose(value: int):
    return "integer declaration"


@declared
def choose(value: str):
    return "string declaration"


def choose(value):
    return value


print([fn.__annotations__["value"].__name__ for fn in get_overloads(choose)])
clear_overloads()
print(get_overloads(choose))


@record
class ForeignNew:
    value: int

    def __new__(cls, *args):
        return 42


print("dataclass new", ForeignNew(1))


class InvalidInit:
    def __init__(self):
        return 9


try:
    InvalidInit()
except TypeError:
    print("initializer return rejected")


class MutableMethod:
    def __init__(self):
        self.value = 1

    def read(self):
        return self.value


def replacement_read(self):
    return 2


MutableMethod.read.__code__ = replacement_read.__code__
print("changed method", MutableMethod().read())


def not_a_class(name, bases, namespace):
    return 7

try:
    @record
    class NonClassDataclass(metaclass=not_a_class):
        value: int
except Exception as error:
    print("dataclass non-class result", type(error).__name__, str(error))

# A finite possible binding can propose a body, but only the real transform
# consumes it. The user branch keeps its ordinary class namespace.
for use_record in (False, True):
    selected = record if use_record else dataclass
    @selected
    class ProposedRecord:
        value: int
    print("selected decorator", use_record, is_dataclass(ProposedRecord))
    if use_record:
        print("selected initializer", ProposedRecord(17).value)
    else:
        print("selected ordinary class", "__init__" in vars(ProposedRecord), ProposedRecord.marker)
def returns_function(cls):
    def answer(value):
        return "captured function", value
    return answer


@returns_function
class FunctionResult:
    pass


def replace_function_result():
    global FunctionResult
    FunctionResult = lambda value: ("replacement", value)
    return 19


print(FunctionResult(replace_function_result()))
print(FunctionResult(20))
