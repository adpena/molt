"""Purpose: differential coverage for enumerate basic."""

print(list(enumerate(["a", "b"])))
print(list(enumerate(range(3), 5)))

items = []
for idx, val in enumerate([10, 20, 30], start=1):
    items.append((idx, val))
print(items)


import builtins

print("named", list(enumerate(iterable=["a", "b"], start=5)))
selected = builtins.enumerate
print("indirect-named", list(selected(**{"iterable": ["x"], "start": 7})))
for args, kwargs in [
    (([1],), {"iterable": [2]}),
    (([1], 2, 3), {}),
    (([1],), {"bad": 0}),
]:
    try:
        selected(*args, **kwargs)
    except TypeError:
        print("enumerate-refusal", True)
    else:
        print("enumerate-refusal", False)


def replacement(*args, **kwargs):
    print("replacement-enumerate", args, kwargs)
    return [(99, "replacement")]


builtins.enumerate = replacement
try:
    print("mutable-enumerate", list(enumerate(["x"], start=4)))
finally:
    builtins.enumerate = selected


# Force the actual declared class constructor, independently of the builtin
# function projection used by a direct frontend call.
public_class = type(selected([]))
print("class-named", list(public_class(iterable=["class"], start=11)))
for args, kwargs in [
    (([1],), {"iterable": [2]}),
    ((), {"start": 2}),
    (([1], 2), {"start": 3}),
    ((), {"bad": 0}),
]:
    try:
        public_class(*args, **kwargs)
    except TypeError:
        print("enumerate-class-refusal", True)
    else:
        print("enumerate-class-refusal", False)
