"""Purpose: differential coverage for operator basic."""

import operator


class Inner:
    def __init__(self) -> None:
        self.value = 7


class Obj:
    def __init__(self) -> None:
        self.value = 3
        self.inner = Inner()

    def add(self, x: int, y: int = 0) -> int:
        return self.value + x + y


obj = Obj()

print(operator.add(2, 3))
print(operator.mul(3, 4))
print(operator.eq(3, 3), operator.eq(3, 4))

print(operator.itemgetter(1)([10, 20, 30]))
print(operator.itemgetter(0, 2)([10, 20, 30]))
print(operator.itemgetter(slice(1, None))([1, 2, 3]))

print(operator.attrgetter("value")(obj))
print(operator.attrgetter("inner.value")(obj))
print(operator.attrgetter("value", "inner.value")(obj))

print(operator.methodcaller("add", 5)(obj))
print(operator.methodcaller("add", 5, y=2)(obj))

try:
    operator.itemgetter()
except TypeError:
    print("itemgetter error")

try:
    operator.attrgetter()
except TypeError:
    print("attrgetter error")

try:
    operator.attrgetter("missing")(obj)
except AttributeError:
    print("attrgetter missing")

try:
    operator.itemgetter(5)([1, 2])
except Exception as exc:
    print(type(exc).__name__)

# Both public construction entries initialize; object.__init__ must not replay
# the former constructor arguments or replace the retained getter payload.
for kind, argument, target in (
    (operator.itemgetter, 1, [10, 20, 30]),
    (operator.attrgetter, "value", obj),
    (operator.methodcaller, "add", obj),
):
    if kind is operator.methodcaller:
        explicit = kind.__new__(kind, argument, 5, y=2)
    else:
        explicit = kind.__new__(kind, argument)
    print("explicit-new", kind.__name__, explicit(target))
    print("inherited-init", kind.__init__ is object.__init__)
    explicit.__init__("ignored")
    print("init-preserved", explicit(target))
    for receiver in (list, 7):
        try:
            kind.__new__(receiver, argument)
        except TypeError:
            print("new-receiver-refused", kind.__name__)
    try:
        object.__new__(kind)
    except TypeError:
        print("object-new-refused", kind.__name__)

class KeywordTarget:
    def result(self, *, name, self_value):
        return name, self_value


print("forward-named-keywords", operator.methodcaller("result", name=4, self_value=9)(KeywordTarget()))
for kind in (operator.itemgetter, operator.attrgetter):
    try:
        kind.__new__(kind, "value", self="forbidden")
    except TypeError:
        print("new-keyword-refused", kind.__name__)
try:
    operator.methodcaller(name="result")
except TypeError:
    print("method-name-positional-only")
