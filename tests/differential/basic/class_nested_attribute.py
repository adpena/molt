"""Purpose: differential coverage for nested class statements.

A ``class`` nested inside another class body must be bound as an attribute of
the enclosing class (CPython binds it in the enclosing class namespace), and
its methods must resolve the class layout without referencing a non-existent
module-global name.  Regression for nested classes being silently dropped from
the enclosing class namespace and for the layout guard of a nested class's
methods being routed through ``module.<name>`` (which a nested class does not
have).
"""


class Box:
    class Lid:
        OPEN = "open"

        def __init__(self, state: str = "closed") -> None:
            self.state = state

        def describe(self) -> str:
            return "lid is " + self.state

    class Hinge:
        def __init__(self) -> None:
            self.angle = 90

    def __init__(self) -> None:
        self.lid = Box.Lid("ajar")
        self.hinge = self.__class__.Hinge()

    def report(self) -> str:
        return self.lid.describe() + ", hinge " + str(self.hinge.angle)


b = Box()
print(b.report())
print(Box.Lid.OPEN)
print(Box.Lid("shut").describe())
print(Box.Hinge().angle)


# Reference a nested class from a method via ``self.__class__``.
print(Box().report())


# Three levels of nesting; class-body reference to a deeper nested attribute.
class A:
    class B:
        class C:
            VALUE = 3

            def get(self) -> int:
                return self.VALUE

    X = B.C.VALUE


print(A.B.C.VALUE)
print(A.B.C().get())
print(A.X)


# A method referencing a sibling nested class by ``Enclosing.Nested``.
class Container:
    class Item:
        def __init__(self, n: int) -> None:
            self.n = n

    def make_item(self, n: int) -> "Container.Item":
        return Container.Item(n)


c = Container()
print(c.make_item(7).n)


# isinstance against a nested class referenced from an enclosing method.
class Holder:
    class Token:
        pass

    def check(self, obj: object) -> bool:
        return isinstance(obj, Holder.Token)


h = Holder()
print(h.check(Holder.Token()))
print(h.check(42))


# classmethod / staticmethod defined inside a nested class.
class Registry:
    class Entry:
        count = 0

        @classmethod
        def bump(cls) -> int:
            cls.count += 1
            return cls.count

        @staticmethod
        def label() -> str:
            return "entry"


print(Registry.Entry.bump())
print(Registry.Entry.bump())
print(Registry.Entry.label())


# Compound statements publish to the same mapping as straight-line classes.
def conditional_classes(flag, count):
    class Owner:
        if flag:
            class Conditional:
                marker = "conditional"
        for index in range(count):
            class Repeated:
                marker = "repeated"
        try:
            class Protected:
                marker = "protected"
        finally:
            finished = True
    return Owner


for flag, count in ((False, 0), (True, 2)):
    owner = conditional_classes(flag, count)
    print("conditional-classes", flag, hasattr(owner, "Conditional"), hasattr(owner, "Repeated"), owner.Protected.marker, owner.finished)


# A custom __prepare__ mapping sees only final transformed classes once.
from dataclasses import dataclass

publication_events = []
prepared_namespaces = []


class RecordingNamespace(dict):
    def __setitem__(self, key, value):
        if key in ("Plain", "Decorated", "Slotted"):
            publication_events.append(("store", key))
        dict.__setitem__(self, key, value)


class RecordingMeta(type):
    @classmethod
    def __prepare__(metacls, name, bases):
        namespace = RecordingNamespace()
        prepared_namespaces.append(namespace)
        return namespace


def observe_decoration(cls):
    publication_events.append(("decorate", "Decorated" in prepared_namespaces[-1]))
    return ("replacement", cls.__name__)


class PublicationOwner(metaclass=RecordingMeta):
    class Plain:
        pass

    @observe_decoration
    class Decorated:
        pass

    @dataclass(slots=True)
    class Slotted:
        value: int


print("publication-events", publication_events)
print("decorated-result", PublicationOwner.Decorated)
print("slotted-result", PublicationOwner.Slotted(7).value)


# Explicit directives choose their declared storage, not class attributes.
GlobalClass = "before"


class DirectiveOwner:
    global GlobalClass
    class GlobalClass:
        marker = "global"


def nonlocal_class():
    Nested = "before"
    class Owner:
        nonlocal Nested
        class Nested:
            marker = "nonlocal"
    return Nested.marker, hasattr(Owner, "Nested")


print("global-class", GlobalClass.marker, hasattr(DirectiveOwner, "GlobalClass"))
print("nonlocal-class", nonlocal_class())
