"""Int arrays take the whole range of their C type: the upper half of `Q`
(and 8-byte `L`) holds ints above i64. Wider ints overflow with the message
of the C conversion each typecode uses; a non-int is a type error."""

from array import array

print(array("Q", [2**64 - 1, 0, 2**63]).tolist())
print(array("Q", [2**64 - 1]) == array("Q", [2**64 - 1]))

for typecode in "bBhHiIlLqQ":
    try:
        array(typecode, [2**70])
    except OverflowError as exc:
        print(typecode, "OverflowError", exc)

for typecode, value in [("q", 2**63), ("Q", 2**64), ("b", 2**64 - 1)]:
    try:
        array(typecode, [value])
    except OverflowError as exc:
        print(typecode, value, "OverflowError", exc)

try:
    array("q", ["text"])
except TypeError:
    print("q TypeError")

# Rich comparison: elementwise, first difference decides; int and float
# elements compare mathematically; NaN is unequal to itself.
a, b = array("q", [1, 2, 3]), array("d", [1.0, 2.0, 3.0])
print(a == b, a != b, a < array("q", [1, 2, 4]), a <= a, a > array("q", [1, 2]))
print(array("d", [float("nan")]) == array("d", [float("nan")]))
print(array("Q", [2**64 - 1]) > array("q", [-1]), array("b") == array("b"))
print(array("d", [1.5]).count(1.5), array("d", [1.0]).count(1), a == [1, 2, 3])

# `u` arrays hold one-character strings.
u = array("u", "xy")
print(u[0], u.tolist(), u.count("x"), u.index("y"), u == array("u", "xy"), u < array("u", "xz"))
for bad in [1, "ab"]:
    try:
        array("u", [bad])
    except TypeError as exc:
        print("u TypeError", exc)
print(u == array("q", [120, 121]))
try:
    u < array("q", [1])
except TypeError:
    print("u < q TypeError")
