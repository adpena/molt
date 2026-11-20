"""Arithmetic that mixes bool with int behaves as CPython's.

bool subclasses int, so True and False add, subtract, multiply and compare as
1 and 0 and the result is an int. A bool accumulator can still cross i64 into
a big int. Bitwise operators on two bools return a bool. A user operand's
reflected method receives the bool itself, not the int.
"""


def accumulate(count, owner):
    total = 0
    other = owner
    for index in range(count):
        total += other is owner
        total += index == 3
        total -= index == 5
    return total


def big_accumulate():
    total = 2**63 - 3
    for index in range(6):
        total += index >= 0
    return total


class Reflects:
    def __radd__(self, other):
        return ("radd", type(other).__name__, other)

    def __rsub__(self, other):
        return ("rsub", type(other).__name__, other)


def mixed(flag, number):
    return (
        number + flag,
        flag + number,
        flag + flag,
        flag - number,
        number * flag,
        flag * flag,
        number - flag,
        type(flag + flag).__name__,
        type(number * flag).__name__,
    )


owner = object()
print("accumulate", accumulate(10, owner), type(accumulate(10, owner)).__name__)
print("big", big_accumulate(), type(big_accumulate()).__name__)
print("mixed true", mixed(True, 7))
print("mixed false", mixed(False, 7))
print("compare", 1 == True, 0 == False, True > 0, False < 1, 2 > True, True >= True)
print("bitwise", True & True, True | False, True ^ True, True & 3, type(True & False).__name__)
print("float", 1.5 + True, True + 2.25, 3.0 * False)
flag = 5 > 2
print("reflected", flag + Reflects(), flag - Reflects())
total = 0.5
for index in range(4):
    total += index % 2 == 0
print("float accumulator", total)
