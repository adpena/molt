"""Purpose: inherited builtin/custom __new__ should not misroute constructor args into object.__init__."""


class MyTuple(tuple):
    pass


tuple_value = MyTuple([1, 2, 3])
assert isinstance(tuple_value, MyTuple)
print(type(tuple_value).__name__)
print(tuple(tuple_value))


class CustomNew:
    def __new__(cls, value):
        instance = super().__new__(cls)
        instance.value = value
        return instance


custom_value = CustomNew(7)
print("direct")
print(custom_value.value)


class BaseNew:
    def __new__(cls, value, tag):
        instance = super().__new__(cls)
        instance.events = ["base", cls.__name__, value, tag]
        return instance


class InheritedNew(BaseNew):
    pass


inherited_value = InheritedNew(11, "inherited")
print("inherited")
print(type(inherited_value).__name__)
for event in inherited_value.events:
    print(event)


class InheritedNewWithInit(BaseNew):
    def __init__(self, value, tag):
        self.events.append("init")


with_init_value = InheritedNewWithInit(13, "with-init")
print("inherited-init")
for event in with_init_value.events:
    print(event)


class OverriddenNew(BaseNew):
    def __new__(cls, value):
        instance = BaseNew.__new__(cls, value + 1, "override")
        instance.events.append("override")
        return instance


overridden_value = OverriddenNew(17)
print("overridden")
print(type(overridden_value).__name__)
for event in overridden_value.events:
    print(event)


class ReplacementNew:
    def __new__(cls, value):
        return ("replacement", value)

    def __init__(self, value):
        print("replacement init should not run")


replacement_value = ReplacementNew(19)
print("replacement")
print(type(replacement_value).__name__)
print(replacement_value[0])
print(replacement_value[1])
