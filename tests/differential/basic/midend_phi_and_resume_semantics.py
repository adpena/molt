"""Branch operands and values restored across generator cleanup stay distinct."""


def select(flag, left, right):
    return left if flag else right


class First:
    pass


class Second:
    pass


def values():
    item = First()
    try:
        for index in range(4):
            before = type(item).__name__
            item.__class__ = select(index % 2 == 0, Second, First)
            yield before, type(item).__name__, select(index % 2 == 0, "left", "right")
    finally:
        print("cleanup")


print(list(values()))
iterator = values()
print(next(iterator))
iterator.close()


for flag in (False, True):
    selected = select(flag, ["left"], ["right"])
    selected.append("alive")
    print(selected)


target = {}
print(target.update([("x", 1)]), target)
print(target.update(), target)
try:
    target.update([("bad",)])
except ValueError as error:
    print(type(error).__name__, target)
