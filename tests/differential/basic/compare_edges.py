"""Purpose: differential coverage for compare edges."""


def section(name):
    print(f"--- {name} ---")


def side_effect(val):
    print(f"eval: {val}")
    return val


section("Chained comparisons with side effects")
# a < b < c
# If a < b is false, c should not be evaluated
print("Case 1 (True, True):")
if side_effect(1) < side_effect(2) < side_effect(3):
    print("All true")

print("Case 2 (False, ?):")
if side_effect(10) < side_effect(2) < side_effect(3):
    print("Should not be here")
else:
    print("Short-circuited")

section("Mixed type comparisons")
print(1 == 1.0)
print(1 == True)
print(0 == False)
print(1.0 == True)
# These are technically True in Python but type-strictness might vary in some langs
# Molt matches Python
print(side_effect(1) == side_effect(1.0))

section("Equality vs Identity")
a = [1, 2]
b = [1, 2]
print(a == b)
print(a is b)
c = a
print(a is c)
