"""Purpose: differential coverage for sorted() kw-only key/reverse."""

if __name__ == "__main__":
    try:
        sorted([3, 2, 1], None, True)
        print("ok")
    except TypeError:
        print("TypeError")


import builtins
from operator import itemgetter

rows = [("a", 1), ("b", 4), ("c", 4)]
print("direct-key", sorted(rows, key=itemgetter(1), reverse=True))
selected = builtins.sorted
print("indirect-key", selected(rows, **{"key": itemgetter(1), "reverse": True}))
print("defaults", selected([3, 1, 2]))
for args, kwargs in [
    ((rows, None), {}),
    ((), {"iterable": rows}),
    ((rows,), {"unknown": True}),
]:
    try:
        selected(*args, **kwargs)
    except TypeError:
        print("sorted-refusal", True)
    else:
        print("sorted-refusal", False)

events = []


def effect(label, value):
    events.append(label)
    return value


print(
    "keyword-order",
    sorted(
        effect("iterable", rows),
        reverse=effect("reverse", True),
        key=effect("key", itemgetter(1)),
    ),
)
print("effects", events)


def replacement(*args, **kwargs):
    print("replacement", len(args), list(kwargs), args[0])
    return "rebound"


builtins.sorted = replacement
try:
    print("mutable-sorted", sorted([3, 1], reverse=True))
finally:
    builtins.sorted = selected
