"""Purpose: & | ^ on boxed ints keep CPython's answers on the inline-int arm.

The late lowering stage expands a bitwise op on boxed words to an inline-int
arm; every other operand (bool, big int, float, user type) takes the runtime.
"""

edge = [0, 1, -1, 63, -64, 2**45, -(2**45), 2**46 - 1, -(2**46), 2**46, 2**63, -(2**70)]
for left in edge:
    for right in edge:
        print(left & right, left | right, left ^ right)

# bool with bool stays bool; bool with int is int.
for left in (True, False):
    for right in (True, False, 1, 6):
        result = (left & right, left | right, left ^ right)
        print(result, [type(value).__name__ for value in result])


class Mask:
    def __and__(self, other):
        return "Mask.__and__"

    def __rand__(self, other):
        return "Mask.__rand__"


print(Mask() & 3, 3 & Mask())

for bad in (1.5, "s", None):
    try:
        print(3 & bad)
    except TypeError as error:
        print("TypeError", error)

# A loop whose index is boxed: the hot case.
total = 0
values = list(range(-50, 50))
for index in range(10000):
    total += values[index % 100] & 63
    total ^= index | 5
print(total)


# The benchmark's shape: a function-local loop index used through `&`, whose
# result the range plan proves inline-safe (0..63), so the arm joins raw.
def masked_sum(items, n):
    total = 0
    for i in range(n):
        total += items[i & 63]
        total += (i ^ 5) & 15
        total += (i | 3) & 255
    return total


print(masked_sum(list(range(64)), 5000))
print(masked_sum([-(2**70)] + list(range(63)), 300))
