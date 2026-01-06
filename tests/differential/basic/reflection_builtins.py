class Base:
    def __init__(self, x: int) -> None:
        self.x = x


class Child(Base):
    pass


obj = Child(5)
print(isinstance(obj, Child))
print(isinstance(obj, Base))
print(issubclass(Child, Base))
print(issubclass(Base, Child))
print(type(obj) is Child)
print(type(Child) is type)
print(isinstance(1, int))
print(isinstance(True, int))
print(type(True) is bool)
print(type(1) is int)
print(isinstance("x", str))
print(isinstance("x", (int, str)))
print(type(None) is type(None))
obj2 = object()
print(isinstance(obj2, object))
print(type(obj2) is object)
