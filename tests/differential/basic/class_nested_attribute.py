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
