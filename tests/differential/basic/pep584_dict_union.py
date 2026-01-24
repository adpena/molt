"""Purpose: differential coverage for PEP 584 dict union operators."""


left = {"a": 1, "b": 2}
right = {"b": 3, "c": 4}

print(list((left | right).items()))

merged = dict(left)
merged |= right
print(list(merged.items()))

print({"a": 1} | {"a": 2})

try:
    _ = left | [("d", 5)]
except TypeError as exc:
    print(type(exc).__name__, exc)
