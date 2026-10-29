"""PyObject_GetIter errors must survive every runtime-call realization."""

for value in (1, 1.0, True, None):
    try:
        iter(value)
    except TypeError as error:
        print(type(value).__name__, str(error))
    else:
        raise AssertionError("a non-iterable must raise TypeError")


class BrokenIterable:
    def __iter__(self):
        raise ValueError("iteration callback")


try:
    iter(BrokenIterable())
except ValueError as error:
    print(type(error).__name__, str(error))
else:
    raise AssertionError("iteration callback exceptions must propagate")


def require_exhausted(iterator):
    try:
        next(iterator)
    except StopIteration:
        return
    raise AssertionError("iterator must remain exhausted")


items = []
items_iter = iter(items)
require_exhausted(items_iter)
items.append("after exhaustion")
require_exhausted(items_iter)
print("list exhaustion is sticky")

mapping = {}
mapping_iter = iter(mapping)
require_exhausted(mapping_iter)
mapping["after"] = True
require_exhausted(mapping_iter)
print("dict exhaustion is sticky")

members = set()
members_iter = iter(members)
require_exhausted(members_iter)
members.add("after")
require_exhausted(members_iter)
print("set exhaustion is sticky")

mapping = {"first": 1, "second": 2}
mapping_iter = iter(mapping)
print("dict first", next(mapping_iter))
del mapping["second"]
mapping["last"] = 3
print("dict replacement", next(mapping_iter))
require_exhausted(mapping_iter)

mapping = {"first": 1, "second": 2}
mapping_iter = iter(mapping)
assert next(mapping_iter) == "first"
del mapping["first"]
mapping["last"] = 3
assert next(mapping_iter) == "second"
try:
    next(mapping_iter)
except RuntimeError as error:
    print("dict keys", str(error))
else:
    raise AssertionError("unexpected remaining dict keys must raise")
require_exhausted(mapping_iter)

mapping = {}
mapping_iter = iter(mapping)
mapping["added"] = 1
for attempt in range(2):
    try:
        next(mapping_iter)
    except RuntimeError as error:
        print("dict size", str(error))
    else:
        raise AssertionError("dict size failure must be sticky")
    if attempt == 0:
        del mapping["added"]

members = set()
members_iter = iter(members)
members.add("added")
for attempt in range(2):
    try:
        next(members_iter)
    except RuntimeError as error:
        print("set size", str(error))
    else:
        raise AssertionError("set size failure must be sticky")
    if attempt == 0:
        members.remove("added")
