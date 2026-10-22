"""Purpose: differential coverage for {**a, **b} dict unpacking syntax."""


# Basic double-star unpacking
defaults = {"a": 1, "b": 2}
overrides = {"b": 3, "c": 4}
merged = {**defaults, **overrides}
print(sorted(merged.items()))

# Single star unpacking
x = {"x": 10}
d = {**x}
print(sorted(d.items()))

# Mixed: literals + star unpacking
a = {"a": 1}
b = {"c": 3}
mixed = {**a, "b": 2, **b}
print(sorted(mixed.items()))

# Empty dict unpacking
empty = {}
result = {**empty, **defaults}
print(sorted(result.items()))

# Override order matters (last wins)
first = {"key": "first"}
second = {"key": "second"}
print({**first, **second}["key"])
print({**second, **first}["key"])

# Star unpacking with literal override
base = {"x": 0, "y": 0}
print(sorted({**base, "x": 99}.items()))

# Nested dict unpacking (not nested dicts, but chained)
a1 = {"a": 1}
a2 = {"b": 2}
a3 = {"c": 3}
print(sorted({**a1, **a2, **a3}.items()))

# Verify original dicts are not mutated
orig = {"k": "v"}
copy = {**orig, "k2": "v2"}
print(sorted(orig.items()))
print(sorted(copy.items()))

# dict() constructor with **kwargs
d1 = {"a": 1}
d2 = {"b": 2}
constructed = dict(**d1, **d2)
print(sorted(constructed.items()))

# dict() with positional + **kwargs
base_pairs = [("x", 10)]
extra = {"y": 20}
constructed2 = dict(base_pairs, **extra)
print(sorted(constructed2.items()))

# Unpacking in return position
def make_config(**overrides):
    defaults = {"debug": False, "verbose": False}
    return {**defaults, **overrides}

print(sorted(make_config(debug=True).items()))

# Displays demand mappings; dict.update/constructor/|= deliberately accept pairs.
pairs = [(1, "first"), (1, "last"), (2, "two")]
try:
    print({**pairs})
except TypeError as error:
    print(type(error).__name__, str(error))
print(dict(pairs))
updated = {}
updated.update(pairs)
print(updated)
updated |= [(3, "three")]
print(updated)

trace = []
class Mapping:
    def keys(self):
        trace.append("keys")
        return [1, "x", 1]
    def __getitem__(self, key):
        trace.append(("get", key))
        return len(trace)
    def __iter__(self):
        raise AssertionError("mapping expansion must use keys")

print({**Mapping(), "tail": 9})
print(trace)

class BrokenKeys:
    def keys(self):
        raise AttributeError("keys failed")

class BrokenItem:
    def keys(self):
        return [1]
    def __getitem__(self, key):
        raise AttributeError("item failed")

for mapping in (BrokenKeys(), BrokenItem()):
    try:
        print({**mapping})
    except (AttributeError, TypeError) as error:
        print(type(error).__name__, str(error))

# Failure stops evaluation before the later segment; successful prefixes may
# have invoked callbacks, but the unfinished dictionary never escapes.
trace.clear()
def later():
    trace.append("later")
    return {}
try:
    result = {**Mapping(), **[(7, 8)], **later()}
except TypeError:
    print(trace)


# Native protocol type-name precision is byte-based and version-sensitive at
# an incomplete UTF-8 boundary; the complete neighboring name is a control.
class BoundaryName:
    pass

for name in ("a" * 198 + "é", "a" * 199 + "é"):
    BoundaryName.__name__ = name
    try:
        result = {**BoundaryName()}
    except TypeError as error:
        print("type-name-boundary", ascii(str(error)))
