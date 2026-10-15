"""Purpose: method calls on builtin receivers keep CPython results and errors (HF-216).

Builtin methods (list, str, dict, int, float, bytes, set) go through the
generic method load, including receivers whose value is an inline integer or a
float such as 0.0.
"""

items = []
items.append(1)
items.append(2)
items.extend([3, 4])
print(items, items.index(3), items.count(1))
print(", ".join(["a", "b", "c"]))
print("".join([]))
print("x-y-z".split("-"))
mapping = {"a": 1}
print(mapping.get("a"), mapping.get("b"), mapping.get("b", 0))
print((5).bit_length(), (-7).bit_length())
print((0.0).hex(), (2.5).is_integer())
print(b"abc".upper(), {1, 2}.union({3}))
print("Hello".lower().upper().strip())


def loop(values):
    out = []
    for value in values:
        out.append(str(value).zfill(3))
    return out


print(loop([1, 22, 333]))
try:
    [].pop()
except IndexError as error:
    print("IndexError", error)
try:
    "abc".index("z")
except ValueError as error:
    print("ValueError", error)
