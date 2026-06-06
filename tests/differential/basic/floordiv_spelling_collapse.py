# Exercises the floordiv spelling collapse (task #57): `//` must reach
# OpCode::FloorDiv and stay byte-identical to CPython across positive, negative,
# zero-crossing, bigint, and loop-accumulator cases on every backend.


def fd(a, b):
    return a // b


# Scalar floor-division: positives, negatives, mixed signs (floor toward -inf).
print(fd(17, 5))
print(fd(-17, 5))
print(fd(17, -5))
print(fd(-17, -5))
print(fd(0, 7))
print(fd(100, 1))

# Bigint floor-division (must NOT wrap through the i64 fast path).
print(fd(1 << 60, 7))
print(fd(-(1 << 60), 7))
print(fd((1 << 70) + 3, (1 << 10) - 1))
print((10**30) // (10**10))

# Float floor-division.
print(fd(7.5, 2.0))
print(fd(-7.5, 2.0))


# Loop accumulator (the first-class arith path / overflow_peel territory).
def fd_sum(n):
    s = 0
    for i in range(1, n):
        s += n // i
    return s


print(fd_sum(100))
print(fd_sum(1000))


# Mixed in a larger expression with mod (the sibling op).
def fd_mod(n):
    out = []
    for i in range(1, n):
        out.append((n // i, n % i))
    return out[:5], out[-3:]


print(fd_mod(50))


# augmented //=
def ifloordiv(a, b):
    a //= b
    return a


print(ifloordiv(100, 3))
print(ifloordiv(-100, 3))
print(ifloordiv(1 << 65, 7))
