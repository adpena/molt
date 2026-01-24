"""Purpose: differential coverage for instancecheck subclasscheck."""


class Meta(type):
    def __instancecheck__(cls, instance):
        return getattr(instance, "flag", False)

    def __subclasscheck__(cls, subclass):
        return getattr(subclass, "marker", False)


class Base(metaclass=Meta):
    pass


class Child:
    marker = True


obj = Child()
obj.flag = True

print(isinstance(obj, Base))
print(issubclass(Child, Base))
