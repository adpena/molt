"""Purpose: match captures and case-body bindings are branch bindings in every scope.

A capture stores its name like an assignment. A case runs on some paths only, so
a name it binds is unbound after the statement on a path where no case ran, and
a guard that fails leaves the case's captures bound.
"""

subject = {"kind": "point", "x": 1, "y": 2}

match subject:
    case {"kind": "point", "x": x, "y": y}:
        moved = x + y
    case _:
        moved = None
print("module capture", x, y, moved)

match [3, 4, 5]:
    case [first, *rest] if first > 10:
        print("module guard", "taken")
    case _:
        print("module guard", "failed", first, rest)

match 7:
    case 0:
        only_zero = "zero"
try:
    print(only_zero)
except NameError as exc:
    print("module unbound", type(exc).__name__, exc)

for value in (1, 2, 1):
    match value:
        case 1:
            seen = "one"
        case other:
            seen = f"other {other}"
    print("module loop", value, seen)


def in_function(value):
    match value:
        case (left, right):
            total = left + right
        case [single]:
            total = single
    try:
        return "function", total
    except UnboundLocalError as exc:
        return "function unbound", type(exc).__name__, str(exc)


print(in_function((2, 3)))
print(in_function([9]))
print(in_function("no match"))


def guard_keeps_capture(value):
    match value:
        case int(number) if number > 100:
            return "big", number
        case _:
            return "small", number


print(guard_keeps_capture(5))
print(guard_keeps_capture(500))


def closure_capture(value):
    match value:
        case {"name": name}:
            pass
        case _:
            name = "anonymous"

    def read():
        return name

    return read()


print("closure", closure_capture({"name": "ada"}), closure_capture(3))


class Holder:
    match ("a", "b"):
        case (head, tail):
            pair = head + tail


print("class", Holder.head, Holder.tail, Holder.pair)


def declared_global(value):
    global matched_global
    match value:
        case [matched_global]:
            pass


declared_global(["g"])
print("global", matched_global)
