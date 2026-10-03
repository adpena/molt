class SubscriptionTuple(tuple):
    def __getitem__(self, key):
        if key == -1:
            raise ValueError("subscription body failure")
        return key

    def __setitem__(self, key, value):
        self.written = (key, value)

    def __delitem__(self, key):
        self.deleted = key


value = SubscriptionTuple((99,))
print(value[0.0])
value[7] = 0.0
print(value.written)
del value[7]
print(value.deleted)
try:
    print(value[-1])
except ValueError as error:
    print(type(error).__name__, str(error))


class InstanceOnly:
    pass


instance = InstanceOnly()
instance.__getitem__ = lambda key: "incorrect instance dispatch"
instance.__setitem__ = lambda key, value: "incorrect instance dispatch"
instance.__delitem__ = lambda key: "incorrect instance dispatch"
for value in (instance, 17, None):
    for operation in range(3):
        try:
            if operation == 0:
                result = value[0]
            elif operation == 1:
                value[0] = 0.0
            else:
                del value[0]
        except TypeError as error:
            print(type(value).__name__, operation, type(error).__name__)


class DictOverride(dict):
    def __getitem__(self, key):
        return ("override", key)


mapping = DictOverride(stored=31)
print(mapping["stored"])
print(dict.__getitem__(mapping, "stored"))
dict.__setitem__(mapping, "new", 42)
print(dict.__getitem__(mapping, "new"))
dict.__delitem__(mapping, "new")
print("new" in mapping)


class BrokenDescriptor:
    def __get__(self, instance, owner):
        raise LookupError("descriptor body failure")


class BrokenSubscription:
    __getitem__ = BrokenDescriptor()
    __setitem__ = BrokenDescriptor()
    __delitem__ = BrokenDescriptor()


broken = BrokenSubscription()
for operation in range(3):
    try:
        if operation == 0:
            result = broken[0]
        elif operation == 1:
            broken[0] = 0
        else:
            del broken[0]
    except LookupError as error:
        print(operation, type(error).__name__, str(error))


view = memoryview(bytearray(b"abc"))
print(view[1])
view[1] = 90
print(bytes(view))
try:
    del view[1]
except TypeError as error:
    print(type(error).__name__, str(error))
