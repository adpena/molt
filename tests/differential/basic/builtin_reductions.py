"""Purpose: differential coverage for builtin reductions."""

print(sum([1, 2, 3]))
print(sum((1, 2), 10))
print(sum(range(4), 1))
print(sum([1, 2], start=10))
print(sum([], 5))
print(sum(i * i for i in range(100000) if (i * i) % 2 == 0))
print(sum(v for v in {str(i): i * i for i in range(100000)}.values() if v % 2 == 0))
print(type(sum([1.0, 2.0])).__name__, sum([1.0, 2.0]))
print(sum([0.1] * 10) == 1.0)

try:
    sum([], "")
except TypeError as exc:
    print(f"sum-str:{exc}")

try:
    sum([], b"")
except TypeError as exc:
    print(f"sum-bytes:{exc}")

try:
    sum([], bytearray(b""))
except TypeError as exc:
    print(f"sum-bytearray:{exc}")

print(min([3, 1, 2]))
print(max([3, 1, 2]))
print(min(3, 1, 2))
print(max(3, 1, 2))


def neg(x):
    return -x


print(min([1, 2, 3], key=neg))
print(max([1, 2, 3], key=neg))
print(min([], default=9))
print(max([], default=9))
print(min([], default=9, key=abs))
print(max([], default=9, key=abs))

try:
    min([])
except ValueError as exc:
    print(f"min-empty:{exc}")

try:
    max([])
except ValueError as exc:
    print(f"max-empty:{exc}")

try:
    min()
except TypeError as exc:
    print(f"min-noargs:{exc}")

try:
    max()
except TypeError as exc:
    print(f"max-noargs:{exc}")

try:
    min(1, 2, default=0)
except TypeError as exc:
    print(f"min-default-multi:{exc}")

try:
    max(1, 2, default=0)
except TypeError as exc:
    print(f"max-default-multi:{exc}")
