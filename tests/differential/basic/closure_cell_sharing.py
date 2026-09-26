"""Purpose: differential coverage for closure cell sharing semantics."""


def outer():
    x = []

    def add(value):
        x.append(value)
        return list(x)

    def snapshot():
        return list(x)

    return add, snapshot


class Rebound:
    marker = "rebound"

    def __call__(self):
        return "called"


def invalidated_cells():
    value = [11]

    def replace(replacement):
        nonlocal value
        value = replacement

    def read():
        return value

    def clear():
        nonlocal value
        del value

    # Arbitrary callbacks invalidate value facts, not the shared lexical cell.
    # The caller and closure must both see the new representation and protocol.
    replace((13, 17))
    print("tuple", value[1], read()[0])
    replace(Rebound())
    print("object", value.marker, value(), read().marker)
    clear()
    try:
        print(value)
    except UnboundLocalError:
        print("unbound-local")
    try:
        read()
    except UnboundLocalError:
        raise AssertionError("a missing free cell is not a local-slot error")
    except NameError:
        print("unbound-free")


def shadowed_names(__name__, len):
    def replace():
        nonlocal __name__, len
        __name__ = "lexical-name"
        len = lambda: "lexical-call"

    replace()
    print(__name__, len())


async def async_cells(value):
    def replace():
        nonlocal value
        value = "async-cell"

    replace()
    return value


def generator_cells(value):
    def replace():
        nonlocal value
        value = "generator-cell"

    replace()
    yield value


def suspended_augassign():
    value = 10

    def replace():
        nonlocal value
        value = 100

    value += yield replace
    yield value


def replace_comprehension_cell(read):
    read.__closure__[0].cell_contents = Rebound()


def class_fallback():
    value = [1]

    def replace():
        nonlocal value
        value = Rebound()

    class Namespace:
        replace()
        result = value.marker, value()

    return Namespace.result


if __name__ == "__main__":
    add, snapshot = outer()
    print("first", add(1))
    print("second", add(2))
    print("snap", snapshot())
    invalidated_cells()
    shadowed_names("old-name", None)
    coroutine = async_cells(None)
    try:
        coroutine.send(None)
    except StopIteration as exc:
        print(exc.value)
    print(next(generator_cells(None)))
    suspended = suspended_augassign()
    replace = next(suspended)
    replace()
    print("captured-before-yield", suspended.send(5))
    print([
        (replace_comprehension_cell(lambda: item), item.marker, item())[1:]
        for item in [[1]]
    ])
    print("class-fallback", class_fallback())
