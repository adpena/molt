"""Purpose: arithmetic fused into a loop condition compiles and keeps its errors.

`if i % 3 < 2:` in a range loop once crashed the build: the exception-region
verifier and the check-elimination pass read different facts about whether
`i % 3` can raise (HF-891). A divisor that may be zero must still raise.
"""


def const_mod(n):
    hits = 0
    for i in range(n):
        if i % 3 < 2:
            hits += 1
    return hits


def const_floordiv(n):
    total = 0
    for i in range(n):
        if i // 4 == 2 or i % 5 == 0:
            total += i
    return total


def nested(n):
    out = []
    for i in range(n):
        if i % 2 == 0:
            if (i * 3) % 7 < 3:
                out.append(i)
        elif i % 3:
            out.append(-i)
    return out


def while_mod(n):
    i = 0
    count = 0
    while i < n:
        if i % 4 != 1:
            count += 1
        i += 1
    return count


def variable_divisor(n, k):
    hits = 0
    for i in range(n):
        if i % k < 1:
            hits += 1
    return hits


print(const_mod(20), const_mod(0))
print(const_floordiv(30))
print(nested(12))
print(while_mod(17))
print(variable_divisor(10, 3), variable_divisor(10, -4))
try:
    variable_divisor(3, 0)
except ZeroDivisionError as exc:
    print("ZeroDivisionError", exc)
