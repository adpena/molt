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

# Dead-result unary operators on a non-numeric must STILL raise TypeError. The
# result of each expression statement is discarded, so the frontend dead-code
# pass must not classify NEG/POS/INVERT/ABS as removable "pure" ops and silently
# delete the raise (the abs("x") parity bug, mirrored across the unary family).
try:
    -"x"
except TypeError as exc:
    print(f"neg-type:{exc}")

try:
    +"x"
except TypeError as exc:
    print(f"pos-type:{exc}")

try:
    ~"x"
except TypeError as exc:
    print(f"invert-type:{exc}")

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
