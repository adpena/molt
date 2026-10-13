"""Purpose: differential coverage for pprint basic."""

import pprint
import sys


data = {"a": [1, 2, 3], "b": {"x": 1, "y": 2}}
print(pprint.pformat(data, width=20))


# An ordinary container must not be treated as its own recursive ancestor.
for value in ([1, 2], (1, 2), {"a": 1}):
    print("ordinary", pprint.pformat(value) == repr(value))
print("narrow", pprint.pformat([1, 2, 3], width=4))

cycle = []
cycle.append(cycle)
printer = pprint.PrettyPrinter()
for width in (1, 80):
    rendered = pprint.pformat(cycle, width=width)
    print("cycle", width, "Recursion on list" in rendered)
formatted, readable, recursive = printer.format(cycle, {}, None, 0)
print("format cycle", "Recursion on list" in formatted, readable, recursive)
shared = [1]
print("shared not cyclic", pprint.pformat([shared, shared]) == "[[1], [1]]")
context = {id(shared): 1}
formatted, readable, recursive = printer.format(shared, context, None, 0)
print(
    "existing context",
    "Recursion on list" in formatted,
    readable,
    recursive,
    context == {id(shared): 1},
)

# A selected exception must leave formatting immediately, without retrying repr.
failure = ValueError("pprint repr sentinel")
visits = []


class BrokenRepr:
    def __repr__(self):
        visits.append("repr")
        raise failure


for label, operation in (
    ("pformat", lambda value: pprint.pformat([value], width=1)),
    ("format", lambda value: printer.format(value, {}, None, 0)),
):
    visits.clear()
    try:
        operation(BrokenRepr())
        print(label, "accepted")
    except ValueError as exc:
        print(label, exc is failure, visits)

# The runtime's decimal limit remains effective inside containers.

old_limit = sys.get_int_max_str_digits()
try:
    sys.set_int_max_str_digits(640)
    huge = 10**640
    for width in (1, 80):
        try:
            pprint.pformat([huge], width=width)
            print("huge", width, "accepted")
        except ValueError:
            print("huge", width, "ValueError")
finally:
    sys.set_int_max_str_digits(old_limit)


class InheritedList(list):
    pass


class InheritedDict(dict):
    pass


class InheritedTuple(tuple):
    pass


class InheritedInt(int):
    pass


list_cycle = InheritedList()
list_cycle.append(list_cycle)
dict_cycle = InheritedDict()
dict_cycle["self"] = dict_cycle
child = []
tuple_cycle = InheritedTuple([child])
child.append(tuple_cycle)
for value in (list_cycle, dict_cycle, tuple_cycle):
    rendered, readable, recursive = printer.format(value, {}, None, 0)
    print(
        "inherited cycle",
        type(value).__name__,
        "Recursion on" in rendered,
        readable,
        recursive,
    )
for value in (InheritedList([1]), InheritedDict(a=1), InheritedTuple([1])):
    context = {id(value): 1}
    rendered, readable, recursive = printer.format(value, context, None, 0)
    print(
        "inherited context",
        "Recursion on" in rendered,
        readable,
        recursive,
        context == {id(value): 1},
    )
print(
    "inherited integer",
    pprint.PrettyPrinter(underscore_numbers=True).format(
        InheritedInt(1234567), {}, None, 0
    ),
)


class OverrideList(list):
    def __repr__(self):
        return "custom-list"


class OverrideDict(dict):
    def __repr__(self):
        return "custom-dict"


class OverrideTuple(tuple):
    def __repr__(self):
        return "custom-tuple"


class OverrideInt(int):
    def __repr__(self):
        return "custom-int"


for value in (
    OverrideList([1]),
    OverrideDict(a=1),
    OverrideTuple([1]),
    OverrideInt(1234567),
):
    print(
        "override",
        pprint.PrettyPrinter(underscore_numbers=True).format(
            value, {id(value): 1}, None, 0
        ),
    )
