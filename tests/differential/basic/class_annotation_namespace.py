"""Class annotation cells follow the actual owner across namespace copying."""
# MOLT_META: min_py=3.12

import sys

events = []
saved = []


class Meta(type):
    @classmethod
    def __prepare__(mcls, name, bases):
        namespace = {"injected": "prepared-only"}
        saved.append(namespace)
        return namespace

    def __new__(mcls, name, bases, namespace):
        namespace["sentinel"] = "prepared"
        events.append(("before-type", namespace["Early"].__value__))
        copied = dict(namespace)
        copied["sentinel"] = "created"
        copied["injected"] = "copied-only"
        result = super().__new__(mcls, name, bases, copied)
        namespace["sentinel"] = "abandoned"
        events.append(("after-type", result.Created.__value__))
        return result


class Subject(metaclass=Meta):
    sentinel = "body"
    type Early = sentinel
    type Created = sentinel
    type Late = sentinel
    type Injected = injected

    def method(value: sentinel):
        pass


Subject.sentinel = "mutated"
original = Subject
Subject = None
print(events)
print(original.Early.__value__, original.Created.__value__, original.Late.__value__)
print(original.Injected.__value__)
print(original.method.__annotations__)
print("saved", saved[0]["sentinel"])

x = "global"


def outer():
    x = "outer"

    class Capture:
        x = "class"
        type Alias = x

    return Capture


capture = outer()
del capture.x
print("declared fallback", capture.Alias.__value__)


def outer_free():
    x = "outer"

    class Capture:
        type Alias = x

    return Capture


print("free fallback", outer_free().Alias.__value__)


if sys.version_info < (3, 14):
    annotation_events = []

    class PreparedAnnotations(type):
        @classmethod
        def __prepare__(mcls, name, bases):
            return {"__annotations__": {"prepared": "kept"}}

    class Eager(metaclass=PreparedAnnotations):
        before = dict(__annotations__)
        value: annotation_events.append("annotation") = annotation_events.append("rhs")
        after = dict(__annotations__)
        __annotations__ = {"replacement": "kept"}
        for index in (0,):
            nested: int = index

    print("eager order", annotation_events)
    print("eager before", Eager.before)
    print("eager after", Eager.after)
    print("eager replaced", Eager.__annotations__)

    class Dead:
        before = dict(__annotations__)
        if False:
            missing: int

    print("dead annotations", Dead.before, Dead.__annotations__)


class Bounds:
    marker = int
    type Alias[T: marker] = T
    type Constraints[T: (marker, str)] = T


Bounds.marker = float
print("bound", Bounds.Alias.__type_params__[0].__bound__ is float)
print(
    "constraints", Bounds.Constraints.__type_params__[0].__constraints__ == (float, str)
)


def conditional_owner(flag, count):
    class Owner:
        marker = "body"
        if flag:
            type Conditional = marker
        for index in range(count):
            type Repeated = marker
        if flag:
            class Nested[T: marker]:
                pass
        if flag:
            def method(value: marker):
                pass
        else:
            def method(value: marker):
                pass
        marker = "finished"

    return Owner


for flag, count in ((False, 0), (True, 2)):
    owner = conditional_owner(flag, count)
    print("owners", flag, count, hasattr(owner, "Conditional"), hasattr(owner, "Repeated"))
    owner.marker = "updated"
    if flag:
        print("conditional", owner.Conditional.__value__)
        print("nested", owner.Nested.__type_params__[0].__bound__)
    if count:
        print("repeated", owner.Repeated.__value__)
    print("method", owner.method.__annotations__)


class TypeFrames[T]:
    direct = T
    values = [T for _ in (0,)]

    def inherited(self):
        type Alias = T
        return Alias

    def shadowed[T](self):
        type Alias = T
        return Alias

    def alias_shadowed(self):
        type Alias[T] = T
        return Alias


owner = TypeFrames()
print("body frame", TypeFrames.direct is TypeFrames.__type_params__[0])
print("inline frame", TypeFrames.values[0] is TypeFrames.__type_params__[0])
print("inherited frame", owner.inherited().__value__ is TypeFrames.__type_params__[0])
print("method frame", owner.shadowed().__value__ is TypeFrames.shadowed.__type_params__[0])
alias = owner.alias_shadowed()
print("alias frame", alias.__value__ is alias.__type_params__[0])


def generic_factory[T]():
    type Alias = T
    return Alias


print("generic function frame", generic_factory().__value__ is generic_factory.__type_params__[0])


def shared_parameter[T](value: T) -> T:
    return T


parameter_slot = shared_parameter.__code__.co_freevars.index("T")
parameter_cell = shared_parameter.__closure__[parameter_slot]
if sys.version_info >= (3, 14):
    annotation_slot = shared_parameter.__annotate__.__code__.co_freevars.index("T")
    print("shared cell", shared_parameter.__annotate__.__closure__[annotation_slot] is parameter_cell)
parameter_cell.cell_contents = "replaced"
print("changed body cell", shared_parameter(None))
if sys.version_info >= (3, 14):
    print("changed annotation cell", shared_parameter.__annotations__)

    class InjectTypeName(type):
        @classmethod
        def __prepare__(mcls, name, bases):
            return {"T": "prepared"}

    class MappedTypeParameter[T](metaclass=InjectTypeName):
        value: T

    print("parameter mapping hit", MappedTypeParameter.__annotate__(1)["value"])
    del MappedTypeParameter.T
    print("parameter mapping miss", MappedTypeParameter.__annotate__(1)["value"] is MappedTypeParameter.__type_params__[0])

    T = "global"

    class DeclaredTypeName[T]:
        T = "class"
        value: T
        type Alias = T

    del DeclaredTypeName.T
    print("declared parameter miss", DeclaredTypeName.__annotate__(1)["value"], DeclaredTypeName.Alias.__value__)


definition_events = []
T = "outside"


def observe_definition(label, value):
    definition_events.append((label, value))
    return value


def definition_decorator(label, visible):
    definition_events.append(("evaluate", label, visible))

    def apply(function):
        definition_events.append(("apply", label))
        return function

    return apply


@definition_decorator("first", T)
@definition_decorator("second", T)
def ordered_definition[T: observe_definition("bound", int)](
    value=observe_definition("positional", T),
    *, keyword=observe_definition("keyword", T),
):
    return T


print("definition order", definition_events)
print("definition defaults", ordered_definition.__defaults__, ordered_definition.__kwdefaults__)
print("definition binding", ordered_definition() is ordered_definition.__type_params__[0])
print("lazy bound", ordered_definition.__type_params__[0].__bound__ is int)
print("bound order", definition_events)


def suspended_definition():
    @definition_decorator("suspended", T)
    def delayed[U](value=(yield "default")):
        return U, value

    return delayed


suspended = suspended_definition()
print("suspended default", next(suspended))
try:
    suspended.send(17)
except StopIteration as result:
    delayed = result.value
print("suspended ownership", delayed()[0] is delayed.__type_params__[0], delayed()[1])


T = "global"


class LocalParameter[T]:
    print("generic local before", T)
    T = "class"
    print("generic local after", T)

    def annotated(self, value: T):
        pass


print("generic local annotation", LocalParameter.annotated.__annotations__["value"])


class ForwardBound[T: U, U]:
    pass


class ForwardConstraints[T: (U, int), U]:
    pass


print("forward bound", ForwardBound.__type_params__[0].__bound__ is ForwardBound.__type_params__[1])
print("forward constraints", ForwardConstraints.__type_params__[0].__constraints__[0] is ForwardConstraints.__type_params__[1])
