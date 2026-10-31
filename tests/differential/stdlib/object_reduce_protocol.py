"""Default object reduction, independently observed against CPython 3.12."""
import copyreg


class Plain:
    def __init__(self, value=1):
        self.value = value


class Slotted:
    __slots__ = ("value",)

    def __init__(self):
        self.value = 2


class State(Slotted):
    def __getstate__(self):
        return {"custom": self.value}


class NewArgs(Plain):
    def __getnewargs__(self):
        return (self.value,)


class NewArgsEx(Plain):
    def __getnewargs_ex__(self):
        return (), {"value": self.value}


class EmptyKwargs(Plain):
    def __getnewargs_ex__(self):
        return (self.value,), {}


class ListSubclass(list):
    pass


class DictSubclass(dict):
    pass


class Override(Plain):
    def __reduce__(self):
        return (Plain, (9,))


def stable(value):
    if isinstance(value, type):
        return ("class", value.__name__)
    if isinstance(value, tuple):
        return tuple(stable(item) for item in value)
    if isinstance(value, list):
        return [stable(item) for item in value]
    if isinstance(value, dict):
        return {key: stable(item) for key, item in value.items()}
    return value


objects = [Plain(), Slotted(), State(), NewArgs(3), NewArgsEx(4),
           EmptyKwargs(5), ListSubclass([6, 7]), DictSubclass(key=8), Override()]
objects[6].value = "list state"
objects[7].value = "dict state"
for obj in objects:
    for protocol in range(6):
        try:
            rv = obj.__reduce_ex__(protocol)
            constructor = rv[0]
            if constructor is copyreg.__newobj__:
                name = "__newobj__"
            elif constructor is copyreg.__newobj_ex__:
                name = "__newobj_ex__"
            elif constructor is copyreg._reconstructor:
                name = "_reconstructor"
            elif constructor is Plain:
                name = "Plain"
            else:
                raise AssertionError("unexpected reduction constructor")
            tail = list(rv[1:3])
            if len(rv) > 3:
                tail.append(None if rv[3] is None else list(rv[3]))
                tail.append(None if rv[4] is None else list(rv[4]))
            print(type(obj).__name__, protocol, name, stable(tail))
        except TypeError as error:
            print(type(obj).__name__, protocol, type(error).__name__, str(error))

# __reduce_ex__ observes a class override, and object.__reduce__ does not
# recursively re-enter that override. Instance-only __reduce__ is ignored.
obj = Plain()
obj.__reduce__ = lambda: (Plain, (77,))
rv = obj.__reduce_ex__(4)
print("instance shadow ignored", rv[0] is copyreg.__newobj__)
rv = object.__reduce__(Override())
print("base reduce", rv[0] is copyreg._reconstructor, rv[1][0] is Override)


class SpecialLookup(Plain):
    def __getnewargs__(self):
        return (11,)


obj = SpecialLookup()
obj.__getnewargs__ = lambda: (99,)
print("type-only newargs", obj.__reduce_ex__(4)[1][1])


class BadNewArgs:
    def __getnewargs__(self):
        return [1]


class BadNewArgsEx:
    def __getnewargs_ex__(self):
        return (), {}, 3


for obj in (BadNewArgs(), BadNewArgsEx()):
    try:
        obj.__reduce_ex__(4)
    except (TypeError, ValueError) as error:
        print("bad newargs", type(error).__name__, str(error))
