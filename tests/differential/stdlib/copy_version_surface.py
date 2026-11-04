"""copy.replace belongs to the target Python 3.13+ surface (HF-649)."""
import copy
import sys
from collections import namedtuple
from dataclasses import dataclass, field, make_dataclass, InitVar, replace as dc_replace
from datetime import date, time, datetime
from inspect import Parameter, Signature
from types import SimpleNamespace, FunctionType

modern = sys.version_info >= (3, 13)
print("target surface", modern, hasattr(copy, "replace"), "replace" in copy.__all__)


class Record:
    def __init__(self, value):
        self.value = value

    def __replace__(self, **changes):
        return Record(changes.get("value", self.value))


if modern:
    original = Record(1)
    replaced = copy.replace(original, value=2)
    print("replace hook", original.value, replaced.value, replaced is not original)


def outcome(label, operation):
    try:
        operation()
    except Exception as error:
        print(label, type(error).__name__, str(error))
    else:
        print(label, "accepted")


Point = namedtuple("Point", "x y")
@dataclass(frozen=True)
class Frozen:
    x: int
    cached: int = field(default=7, init=False)


# The 3.12 oracle raises ValueError; 3.13+ raises TypeError. Exercise the
# same public implementation even when the copy protocol is unavailable.
outcome("replace init false", lambda: dc_replace(Frozen(1), cached=4))
RequiredInitVar = make_dataclass("RequiredInitVar", [("x", int), ("scale", InitVar[int])])
outcome("replace missing initvar", lambda: dc_replace(RequiredInitVar(1, 2)))
DefaultInitVar = make_dataclass("DefaultInitVar", [("x", int), ("scale", InitVar[int], field(default=2))])
print("replace default initvar", dc_replace(DefaultInitVar(1), x=3).x)

# A retained function's globals are independent of the mutable import cache.
required_record = RequiredInitVar(1, 2)
dataclasses_module = sys.modules.pop("dataclasses")
try:
    outcome("replace removed module cache", lambda: dc_replace(required_record))
    sys.modules["dataclasses"] = SimpleNamespace(MISSING=object())
    outcome("replace substituted module cache", lambda: dc_replace(required_record))
finally:
    sys.modules["dataclasses"] = dataclasses_module
NoInitVar = make_dataclass("NoInitVar", [("scale", InitVar[int], field(init=False, default=2))])
outcome("replace init false initvar", lambda: dc_replace(NoInitVar(), scale=3))
print("replace unchanged init false initvar", isinstance(dc_replace(NoInitVar()), NoInitVar))


@dataclass
class MappingRecord(dict):
    x: int = 1


@dataclass
class ListRecord(list):
    x: int = 1


@dataclass
class TupleRecord(tuple):
    x: int = 1


for record_type in (MappingRecord, ListRecord, TupleRecord):
    original_record = record_type()
    replaced_record = dc_replace(original_record, x=3)
    print("replace native subclass", type(replaced_record) is record_type,
          original_record.x, replaced_record.x)


# Isolate replacement's observable lookup order from generated-init behavior.
replacement_reads = []


@dataclass(init=False)
class ObservedRecord:
    x: int = 1
    cached: int = field(init=False, default=2)

    def __init__(self, x=1):
        object.__setattr__(self, "x", x)

    def __getattribute__(self, name):
        if name in ("__class__", "__dataclass_fields__"):
            replacement_reads.append(name)
        return object.__getattribute__(self, name)


observed_record = ObservedRecord()
replacement_reads.clear()
outcome("replace observer error", lambda: dc_replace(observed_record, cached=3))
print("replace error reads", replacement_reads)
replacement_reads.clear()
observed_updated = dc_replace(observed_record, x=3)
print("replace success reads", replacement_reads, observed_updated.x)


def code_source(value):
    return value + 1


code = code_source.__code__
producers = [Point, Frozen, SimpleNamespace, date, time, datetime, Parameter, Signature, type(code)]
print("producer gates", [hasattr(producer, "__replace__") for producer in producers])
# The underlying inspect and code replacement methods predate Python 3.13.
parameter = Parameter("value", Parameter.POSITIONAL_OR_KEYWORD, default=2)
parameter_copy = parameter.replace(name="renamed", default=Parameter.empty)
print("parameter replace", parameter.name, parameter.default, parameter_copy.name,
      parameter_copy.default is Parameter.empty, parameter_copy is not parameter)
signature = Signature([parameter], return_annotation=int)
signature_copy = signature.replace(parameters=[parameter_copy], return_annotation=Signature.empty)
print("signature replace", list(signature.parameters), list(signature_copy.parameters),
      signature_copy.return_annotation is Signature.empty, signature_copy is not signature)
renamed = code.replace(co_name="renamed", co_qualname="renamed", co_filename="replacement.py", co_firstlineno=42)
print("code metadata", renamed.co_name, renamed.co_qualname, renamed.co_filename,
      renamed.co_firstlineno, renamed.co_flags == code.co_flags, renamed is not code,
      FunctionType(renamed, globals())(9))
# Every accepted keyword can explicitly repeat its current value, including
# bytecode/frame fields whose *changed* values are outside the AOT contract.
code_fields = ("co_argcount", "co_posonlyargcount", "co_kwonlyargcount", "co_nlocals",
               "co_stacksize", "co_flags", "co_firstlineno", "co_code", "co_consts",
               "co_names", "co_varnames", "co_freevars", "co_cellvars", "co_filename",
               "co_name", "co_qualname", "co_linetable", "co_exceptiontable")
print("code unchanged", [code.replace(**{name: getattr(code, name)}) is not code for name in code_fields])
outcome("code unknown", lambda: code.replace(unknown=1))
outcome("code negative", lambda: code.replace(co_firstlineno=-1))
outcome("code bad type", lambda: code.replace(co_name=5))

if modern:
    for unsupported in (5, object()):
        outcome("unsupported", lambda: copy.replace(unsupported))

    class OnlyOldHook:
        def _replace(self, **changes):
            return self

    outcome("old hook refused", lambda: copy.replace(OnlyOldHook(), x=1))
    point = Point(1, 2)
    updated = copy.replace(point, x=3)
    print("namedtuple", tuple(point), tuple(updated), updated is not point,
          Point.__replace__ is Point._replace)
    outcome("namedtuple unknown", lambda: copy.replace(point, missing=4))
    frozen = Frozen(1)
    updated = copy.replace(frozen, x=3)
    print("frozen", frozen.x, updated.x, updated.cached, updated is not frozen)
    outcome("dataclass init false", lambda: copy.replace(frozen, cached=4))
    WithInitVar = make_dataclass("WithInitVar", [("x", int), ("scale", InitVar[int], field(default=2))])
    with_initvar = WithInitVar(1)
    print("initvar default", copy.replace(with_initvar, x=3).x, dc_replace(with_initvar, x=4).x)
    namespace = SimpleNamespace(x=1, retained=[])
    updated = copy.replace(namespace, x=3, added=4)
    print("namespace", namespace.x, updated.x, updated.added, updated.retained is namespace.retained,
          updated is not namespace)
    outcome("namespace positional", lambda: namespace.__replace__(1))
    print("namespace self field", copy.replace(namespace, self=5).self)
    for original, changes, invalid in (
        (date(2020, 2, 3), {"day": 4}, {"month": 13}),
        (time(1, 2, 3), {"hour": 4}, {"minute": 60}),
        (datetime(2020, 2, 3, 1, 2, 3), {"day": 4, "hour": 5}, {"month": 13}),
    ):
        updated = copy.replace(original, **changes)
        print("datetime producer", str(original), str(updated), updated is not original)
        outcome("datetime invalid", lambda: copy.replace(original, **invalid))
    updated = copy.replace(parameter, name="changed", default=3)
    print("parameter producer", updated.name, updated.default, updated is not parameter,
          Parameter.__replace__ is Parameter.replace)
    outcome("parameter invalid", lambda: copy.replace(parameter, kind=20))
    updated = copy.replace(signature, return_annotation=str)
    print("signature producer", list(updated.parameters), updated.return_annotation is str,
          updated is not signature, Signature.__replace__ is Signature.replace)
    outcome("signature duplicate", lambda: copy.replace(signature, parameters=[parameter, parameter]))
    updated = copy.replace(code, co_name="copy_name")
    print("code producer", updated.co_name, updated is not code, FunctionType(updated, globals())(9))

    SelfField = namedtuple("SelfField", "self value")
    print("namedtuple self field", tuple(copy.replace(SelfField(1, 2), self=3)))

    class DateSubclass(date):
        pass

    class TimeSubclass(time):
        pass

    class DatetimeSubclass(datetime):
        pass

    for original in (DateSubclass(2020, 2, 3), TimeSubclass(1), DatetimeSubclass(2020, 2, 3)):
        print("datetime subtype", type(copy.replace(original)) is type(original))

    class DateIndex:
        def __index__(self):
            return 1

    for original, fields in (
        (date(2020, 2, 3), ("year", "month", "day")),
        (time(1), ("hour", "minute", "second", "microsecond", "fold")),
        (datetime(2020, 2, 3), ("year", "month", "day", "hour", "minute", "second", "microsecond", "fold")),
    ):
        for name in fields:
            for invalid in (None, 1.0, 2**100):
                outcome("datetime integer", lambda: copy.replace(original, **{name: invalid}))
        indexed = copy.replace(original, **{fields[0]: DateIndex()})
        print("datetime index", getattr(indexed, fields[0]))

    class NamespaceSubclass(SimpleNamespace):
        calls = []

        def __init__(self):
            self.calls.append("init")
            self.retained = 1

    namespace = NamespaceSubclass()
    namespace.retained = 2
    updated = copy.replace(namespace, added=3)
    print("namespace subtype", type(updated) is NamespaceSubclass, updated.retained,
          updated.added, NamespaceSubclass.calls)

    @dataclass
    class CustomDataclass:
        x: int

        def __replace__(self, **changes):
            return ("custom", changes["x"])

    print("dataclass user hook", copy.replace(CustomDataclass(1), x=2))

    as_module_name = code.replace(co_name="<module>")
    print("code execution identity", as_module_name.co_flags == code.co_flags,
          as_module_name.co_consts == code.co_consts,
          FunctionType(as_module_name, globals())(9))

    class Text(str):
        pass

    for name in ("co_name", "co_filename", "co_qualname"):
        text = Text("replacement")
        updated = copy.replace(code, **{name: text})
        print("code metadata subtype", name, getattr(updated, name) is text)
        equal_text = Text(getattr(code, name))
        equal_updated = copy.replace(code, **{name: equal_text})
        print("code equal metadata subtype", name, getattr(equal_updated, name) is equal_text)


    # One diagnostic family, with CPython's 3.14 wording transition. Check
    # each field and the leap-aware day boundary through the actual producer.
    for changes in ({"year": 0}, {"month": 13}, {"day": 30}):
        outcome("date range family", lambda: copy.replace(date(2020, 2, 1), **changes))
    for changes in ({"hour": 24}, {"minute": 60}, {"second": 60},
                    {"microsecond": 1000000}, {"fold": 2}):
        outcome("time range family", lambda: copy.replace(time(1), **changes))

# Renaming a closure or generator must preserve its compiled context and kind.
def make_closure(value):
    def captured(extra):
        return value + extra
    return captured


closure = make_closure(7)
cloned_closure_code = closure.__code__.replace(co_name="captured_copy")
cloned_closure = FunctionType(cloned_closure_code, globals(), closure=closure.__closure__)
print("code closure identity", cloned_closure(3), cloned_closure_code.co_freevars == closure.__code__.co_freevars)


def generator_source(value):
    yield value + 1


cloned_generator_code = generator_source.__code__.replace(co_name="generator_copy")
cloned_generator = FunctionType(cloned_generator_code, globals())
print("code generator identity", list(cloned_generator(9)), cloned_generator_code.co_flags == generator_source.__code__.co_flags)
