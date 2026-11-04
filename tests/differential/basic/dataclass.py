"""Purpose: differential coverage for dataclass."""

from dataclasses import dataclass


@dataclass
class Point:
    x: int
    y: int = 2


p = Point(1)
print(p.x)
print(p.y)
print(p)
print(p == Point(1, 2))
print(p == Point(2, 2))
p.y = 5
print(p.y)
p.z = 9
print(p.z)

# The decorator reads cls.__dict__.get/items and must see a live namespace.
from dataclasses import fields, is_dataclass


class RuntimePoint:
    __annotations__ = {"x": int, "y": int}
    y = 4


namespace = RuntimePoint.__dict__
RuntimePoint = dataclass(RuntimePoint)
runtime_point = RuntimePoint(6)
print(runtime_point.x, runtime_point.y)
print([field.name for field in fields(RuntimePoint)], is_dataclass(RuntimePoint))
print("__dataclass_fields__" in namespace, "__molt_dataclass__" in namespace)
print("__molt_field_offsets__" in namespace, "__molt_layout_size__" in namespace)
print(namespace.get("y"), vars(RuntimePoint).get("y"))


def annotated(value):
    return value


annotated.__annotations__ = {"value": int}
annotated.extra = 7
print(annotated.__annotations__["value"] is int, annotated.extra)
del annotated.extra
print(hasattr(annotated, "extra"))

# An initializer snapshots its defaults and is also the direct-call method.
saved_default = Point.__init__.__defaults__
Point.__dataclass_fields__["y"].default = 99
print("captured default", Point(8).y, saved_default, Point.__init__.__defaults__)
direct = object.__new__(Point)
Point.__init__(direct, 7, 6)
print("direct init", direct.x, direct.y, Point.__init__.__name__)

# Class callbacks can change the actual schema after the compiler saw the
# annotations. The unchanged control must retain the ordinary compiled body.
class AddField:
    def __set_name__(self, owner, name):
        owner.__annotations__["late"] = int
        owner.late = 12


@dataclass
class ChangedShape:
    first: int
    callback = AddField()


@dataclass
class UnchangedShape:
    first: int
    late: int = 12


for record in (ChangedShape, UnchangedShape):
    value = record(4)
    print("shape", value.first, value.late,
          record.__init__.__code__.co_varnames[:record.__init__.__code__.co_argcount])

# Runtime and static shapes expose real function code, defaults and binding.
from types import FunctionType

for record in (Point, RuntimePoint):
    method = record.__init__
    copied = FunctionType(method.__code__, method.__globals__,
                          method.__name__, method.__defaults__, method.__closure__)
    value = object.__new__(record)
    copied(value, 3)
    print("reconstructed", value.x, value.y)


def replacement_init(self, x, y=0):
    self.x = x + 10
    self.y = y + 20


for record in (Point, RuntimePoint):
    saved_code = record.__init__.__code__
    record.__init__.__code__ = replacement_init.__code__
    value = record(5, 6)
    print("replaced code", value.x, value.y)
    record.__init__.__code__ = saved_code

# A factory sees an ordinary initializer frame. PEP 667 writes must affect
# subsequent fields on 3.13+, for runtime plans as well as compiled templates.
import sys
from dataclasses import field, make_dataclass


def observing_factory():
    frame = sys._getframe(1)
    print("factory frame", frame.f_code.co_name,
          sorted(name for name in frame.f_locals if name in ("self", "first", "later")))
    frame.f_locals["later"] = 55
    return []


@dataclass
class StaticFrame:
    first: list = field(default_factory=observing_factory)
    later: int = 7


RuntimeFrame = make_dataclass("RuntimeFrame", [
    ("first", list, field(default_factory=observing_factory)),
    ("later", int, 7),
])
print("static frame write", StaticFrame().later)
print("runtime frame write", RuntimeFrame().later)

# Generated functions capture the transform's module namespace, including the
# empty namespace required when a class names an unregistered module.
MissingModule = make_dataclass("MissingModule", [("x", int)],
                               module="_hf668_unregistered_module_")
missing_globals = MissingModule.__init__.__globals__
print("missing module globals", sorted(missing_globals),
      MissingModule.__init__.__module__, MissingModule(9).x)

class ChangeModuleDefault:
    def __get__(self, instance, owner):
        owner.__module__ = "_hf668_changed_module_"
        return 13

@dataclass
class CapturedModule:
    value: int = ChangeModuleDefault()

print("captured module globals", CapturedModule.__init__.__globals__ is globals(),
      CapturedModule.__init__.__module__ == __name__, CapturedModule().value)

# Environment capture must keep the original dictionary across callbacks, and
# function creation may insert only the missing __builtins__ entry. An explicit
# existing entry is preserved, even when it is not the interpreter's dictionary.
import sys
from types import ModuleType

environment_name = "_hf668_initializer_environment_"
environment = ModuleType(environment_name)
environment.__dict__.clear()
sys.modules[environment_name] = environment

class ReplaceEnvironment:
    def __get__(self, instance, owner):
        sys.modules[environment_name] = ModuleType(environment_name)
        return 17

class EnvironmentOwner:
    __module__ = environment_name
    __annotations__ = {"value": int}
    value = ReplaceEnvironment()

EnvironmentOwner = dataclass(EnvironmentOwner)
print("environment snapshot", EnvironmentOwner.__init__.__globals__ is environment.__dict__,
      sorted(environment.__dict__), EnvironmentOwner.__init__.__module__, EnvironmentOwner().value)
explicit_builtins = {"sentinel": 19}
environment.__dict__["__builtins__"] = explicit_builtins
sys.modules[environment_name] = environment
class ExplicitEnvironment:
    __module__ = environment_name
    __annotations__ = {"value": int}
ExplicitEnvironment = dataclass(ExplicitEnvironment)
print("explicit builtins", ExplicitEnvironment.__init__.__globals__["__builtins__"] is explicit_builtins,
      ExplicitEnvironment(23).value)

class EnvironmentGlobals(dict):
    def __contains__(self, key):
        raise AssertionError("exec uses the native dictionary boundary")

environment_globals = EnvironmentGlobals(__name__=environment_name)
class EnvironmentHolder:
    @property
    def __dict__(self):
        return environment_globals

sys.modules[environment_name] = EnvironmentHolder()
class SubclassEnvironment:
    __module__ = environment_name
    __annotations__ = {"value": int}
SubclassEnvironment = dataclass(SubclassEnvironment)
print("subclass environment", SubclassEnvironment.__init__.__globals__ is environment_globals,
      sorted(environment_globals), SubclassEnvironment(29).value)
del sys.modules[environment_name]
