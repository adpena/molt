"""Exact-str read admission must preserve live Python hash/equality protocols."""

events = []


def lookup(operation, mapping, key):
    if operation == "item":
        return mapping[key]
    if operation == "contains":
        return key in mapping
    if operation == "contains-method":
        return mapping.__contains__(key)
    if operation == "get":
        return mapping.get(key, -1)
    if operation == "setdefault":
        return mapping.setdefault(key, 91)
    if operation == "setdefault-list":
        return mapping.setdefault(key, [])
    if operation == "pop":
        return mapping.pop(key, -1)
    if operation == "delete":
        del mapping[key]
        return "deleted"
    raise AssertionError(operation)


operations = (
    "item", "contains", "contains-method", "get", "setdefault",
    "setdefault-list", "pop", "delete",
)


class Name(str):
    def __hash__(self):
        events.append("hash")
        return str.__hash__(self)

    def __eq__(self, other):
        events.append("equal")
        return str.__eq__(self, other)


stored = Name("x")
mapping = {stored: 11}
events.clear()
print("stored subclass", mapping.get("x"), events)
events.clear()
print("query subclass", {"x": 12}.get(Name("x")), events)
for operation in operations:
    mapping = {Name("x"): 11}
    events.clear()
    result = lookup(operation, mapping, "x")
    print("stored subclass family", operation, result, events)
    mapping = {"x": 12}
    events.clear()
    result = lookup(operation, mapping, Name("x"))
    print("query subclass family", operation, result, events)
mapping = {stored: 11}
Name.__hash__ = None
events.clear()
try:
    mapping.get(stored)
except TypeError:
    print("disabled hash", events)
for operation in operations:
    events.clear()
    try:
        lookup(operation, mapping, stored)
    except TypeError:
        print("disabled hash family", operation, events, len(mapping))


class Collision:
    def __init__(self, name):
        self.name = name
        self.mode = "unequal"
        self.mapping = None

    def __hash__(self):
        return hash(self.name)

    def __eq__(self, other):
        events.append(self.mode)
        if self.mode == "raise":
            raise LookupError("collision equality")
        if self.mode == "mutate":
            self.mode = "unequal"
            self.mapping.clear()
            for index in range(64):
                self.mapping[index] = index
            self.mapping[self.name] = 43
        return self.mode == "equal"


collision = Collision("x")
mapping = {collision: 7, "x": 21}
for mode in ("unequal", "equal", "raise", "mutate"):
    collision.mode = mode
    collision.mapping = mapping
    events.clear()
    try:
        print("collision", mode, mapping.get("x"), events)
    except LookupError as error:
        print("collision", mode, type(error).__name__, str(error), events)


for operation in operations:
    for mode in ("unequal", "equal", "raise", "mutate"):
        collision = Collision("x")
        mapping = {collision: 7, "x": 21}
        collision.mode = mode
        collision.mapping = mapping
        events.clear()
        try:
            result = lookup(operation, mapping, "x")
            print("collision family", operation, mode, result, events, len(mapping))
        except LookupError as error:
            print("collision family", operation, mode, type(error).__name__, events, len(mapping))
    mapping = {"x": 21}
    events.clear()
    try:
        result = lookup(operation, mapping, "missing")
        print("missing family", operation, result, len(mapping))
    except KeyError:
        print("missing family", operation, "KeyError", len(mapping))


class MergeKey:
    def __hash__(self):
        events.append("merge hash")
        return hash("merge-key")


merge_key = MergeKey()
source = {merge_key: 61}
events.clear()
target = {}
target.update(source)
print("prehashed update", len(target), next(iter(target)) is merge_key, events)


class OnceKey:
    def __init__(self):
        self.calls = 0

    def __hash__(self):
        self.calls += 1
        if self.calls > 1:
            raise RuntimeError("setdefault hashed twice")
        return 37


for operation in ("setdefault", "setdefault-list"):
    key = OnceKey()
    mapping = {}
    result = lookup(operation, mapping, key)
    print("setdefault one hash", operation, key.calls, result, len(mapping), next(iter(mapping)) is key)
    key.calls = 0
    again = lookup(operation, mapping, key)
    print("setdefault hit", operation, key.calls, again is result, len(mapping))


class UnequalKey:
    def __hash__(self):
        return 37

    def __eq__(self, other):
        events.append("unequal once")
        return False


for operation in ("setdefault", "setdefault-list"):
    mapping = {UnequalKey(): 1}
    key = OnceKey()
    events.clear()
    result = lookup(operation, mapping, key)
    print("setdefault one probe", operation, key.calls, events, result, len(mapping))


class Point:
    def __init__(self):
        self.x = 1


point = Point()
for value in range(3):
    point.x = value
    print("field", point.x)

collision = Collision("x")
point.__dict__ = {collision: 7, "x": 31}
events.clear()
print("attribute dictionary", point.x, events)
collision.mode = "equal"
events.clear()
print("attribute equal", point.x, events)
collision.mode = "raise"
events.clear()
try:
    point.x
except LookupError as error:
    print("attribute error", type(error).__name__, str(error), events)

events.clear()
Point.x = property(lambda self: 51)
print("descriptor precedence", point.x, events)
del Point.x
point.__dict__ = {"x": 61}
print("replacement dictionary", point.x)
Point.__getattribute__ = lambda self, name: 71
print("changed hook", point.x)
del Point.__getattribute__
print("restored hook", point.x)

del point.x
Point.__getattr__ = lambda self, name: 81
print("missing fallback", point.x)
