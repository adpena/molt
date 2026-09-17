"""Purpose: differential coverage for float protocol."""

import warnings


class Floaty:
    def __float__(self):
        return 1.25


class Indexy:
    def __index__(self):
        return 7


print(float(Floaty()), float(Indexy()))


class BadFloat:
    def __float__(self):
        return 1


try:
    float(BadFloat())
except Exception as e:
    print(e)


class BadIndex:
    def __index__(self):
        return 1.5


try:
    float(BadIndex())
except Exception as e:
    print(e)


print(pow(2, 5, 7))
print(pow(3, -1, 11))

try:
    pow(2.0, 3, 5)
except Exception as e:
    print(e)

try:
    pow(2, 3, 0)
except Exception as e:
    print(e)

try:
    pow(2, -1, 4)
except Exception as e:
    print(e)


# One numeric protocol, with distinct constructor/payload-first entry policies.


events = []


class FloatSubclass(float):
    def __float__(self):
        events.append("float-subclass")
        return 2.5


class IntSubclass(int):
    def __float__(self):
        events.append("int-subclass")
        return 3.5

    def __index__(self):
        events.append("int-index")
        return 9


class IndexIntSubclass(int):
    def __index__(self):
        events.append("ignored-int-index")
        raise AssertionError("inherited int float conversion reads payload")


class NumericText(str):
    def __float__(self):
        events.append("text-float")
        return 8.5


class FloatAndIndex:
    def __float__(self):
        events.append("float")
        return 4.5

    def __index__(self):
        events.append("index")
        return 9


class RaiseFloat(FloatAndIndex):
    def __float__(self):
        events.append("raise-float")
        raise RuntimeError("float sentinel")


class SpecialLookup(FloatAndIndex):
    def __getattribute__(self, name):
        events.append("getattribute")
        raise AssertionError("numeric special lookup used instance attributes")


class FloatSlot:
    def __get__(self, obj, owner):
        events.append("bind-float")
        return lambda: 5.25


class DescriptorFloat:
    __float__ = FloatSlot()


class RaiseIndex:
    def __index__(self):
        events.append("raise-index")
        raise LookupError("index sentinel")


class InvalidFloat(FloatAndIndex):
    def __float__(self):
        events.append("invalid-float")
        return 1


class InvalidIndex:
    def __index__(self):
        events.append("invalid-index")
        return 1.5


class FloatReturnSubclass:
    def __float__(self):
        events.append("return-float-subclass")
        return FloatSubclass(6.25)


class IndexReturnSubclass:
    def __index__(self):
        events.append("return-int-subclass")
        return IntSubclass(6)


class IndexReturnBool:
    def __index__(self):
        events.append("return-bool")
        return True


class HugeIndex:
    def __index__(self):
        events.append("huge-index")
        return 1 << 4096


class InstanceOnly:
    pass


instance_only = InstanceOnly()
instance_only.__float__ = lambda: 10.5
instance_only.__index__ = lambda: 10


def memoryview_number(value):
    view = memoryview(bytearray(8)).cast("d")
    try:
        view[0] = value
        return view[0]
    finally:
        view.release()


operations = [
    ("float", float),
    ("percent", lambda value: "%f" % value),
    ("memoryview", memoryview_number),
]
if hasattr(float, "from_number"):
    operations.append(("from-number", float.from_number))


def conversion_result(label, value, action):
    for name, operation in operations:
        events.clear()
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter(action, DeprecationWarning)
            try:
                result = operation(value)
                outcome = ("ok", repr(result))
            except Exception as exc:
                outcome = (type(exc).__name__, str(exc))
        print("conversion", label, name, action, outcome, events)
        print(
            "warnings", [(item.category.__name__, str(item.message)) for item in caught]
        )


for label, value in (
    ("float-subclass", FloatSubclass(1.25)),
    ("int-subclass", IntSubclass(1)),
    ("int-index-override", IndexIntSubclass(7)),
    ("numeric-text", NumericText("not a float")),
    ("both", FloatAndIndex()),
    ("special-lookup", SpecialLookup()),
    ("float-descriptor", DescriptorFloat()),
    ("raise-float", RaiseFloat()),
    ("raise-index", RaiseIndex()),
    ("invalid-float", InvalidFloat()),
    ("invalid-index", InvalidIndex()),
    ("instance-only", instance_only),
    ("huge-int", 1 << 4096),
    ("huge-index", HugeIndex()),
    ("text", "1.25"),
    ("bytes", b"1.25"),
    ("nan", float("nan")),
    ("infinity", float("inf")),
):
    conversion_result(label, value, "always")

for label, value in (
    ("float-return-subclass", FloatReturnSubclass()),
    ("index-return-subclass", IndexReturnSubclass()),
    ("index-return-bool", IndexReturnBool()),
):
    for action in ("always", "ignore", "error"):
        conversion_result(label, value, action)

# The runtime warning also reaches custom display hooks, without a private
# deduplication cache suppressing repeated "always" warnings.
with warnings.catch_warnings():
    warnings.simplefilter("always", DeprecationWarning)
    old_showwarning = warnings.showwarning
    shown = []

    def showwarning(message, category, filename, lineno, file=None, line=None):
        shown.append((category.__name__, str(message)))

    warnings.showwarning = showwarning
    try:
        float(FloatReturnSubclass())
        float(FloatReturnSubclass())
    finally:
        warnings.showwarning = old_showwarning
    print("custom-warning", shown)
