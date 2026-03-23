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
