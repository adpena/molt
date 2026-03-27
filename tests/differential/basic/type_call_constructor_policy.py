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
print(custom_value.value)
