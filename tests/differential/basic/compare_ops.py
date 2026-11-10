"""Purpose: differential coverage for compare ops."""

nums = [1, 2, 3]

print((1 < 2, 2 <= 2, 3 > 2, 3 >= 4, 3 != 4, 3 == 3))

a = [1]
b = a
c = [1]
print((a is b, a is c, a is not c))

print((2 in nums, 4 in nums, 4 not in nums))
print(("a" in "cat", "z" in "cat"))
print((b"a" in b"cat", b"ab" in b"cat", b"" in b"cat"))
print((97 in b"cat", 97 in bytearray(b"cat")))
print((3 in range(1, 5), 6 in range(1, 5)))

print((1 < 2 < 3, 1 < 3 > 2, 1 < 2 > 3, 1 < 2 == 2))

print((not False, not 0, not 1))


# A rich comparison result keeps its object word; identity consumes neither
# comparison truth nor numeric equality. This also exercises mixed raw/boxed
# boolean carriers on backends with scalar representation lowering.
class TruthMustNotRun:
    def __bool__(self):
        raise AssertionError("identity invoked comparison-result truth")


marker = TruthMustNotRun()


class RichValue:
    def __init__(self, result):
        self.result = result

    def __gt__(self, other):
        return self.result


for selected in (False, True, marker):
    result = RichValue(selected) > 2
    print("rich identity", result is False, False is result, result is True,
          result is not False, False is not result, result is marker)
zero, one = 0, 1
false, true = False, True
print("boolean tags", false is zero, true is one, zero is not false, one is not true)
