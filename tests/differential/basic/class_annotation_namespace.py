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
