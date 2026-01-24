"""Purpose: differential coverage for builtin numeric ops."""


class AbsObj:
    def __abs__(self):
        return "abs-ok"


print(abs(-3))
print(abs(True))
print(abs(-3.5))
print(abs(10**50))
print(abs(AbsObj()))

try:
    abs("x")
except TypeError as exc:
    print(f"abs-type:{exc}")

print(divmod(7, 3))
print(divmod(-7, 3))
print(divmod(7, -3))
print(divmod(-7, -3))
print(divmod(7.5, 2.0))
print(divmod(-7.5, 2.0))
print(divmod(7.5, -2.0))
print(divmod(True, 2))

try:
    divmod(1, 0)
except ZeroDivisionError as exc:
    print(f"divmod-zero:{exc}")

try:
    divmod(1.0, 0.0)
except ZeroDivisionError as exc:
    print(f"divmod-fzero:{exc}")

try:
    divmod("a", 1)
except TypeError as exc:
    print(f"divmod-type:{exc}")
