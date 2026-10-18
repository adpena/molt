# Parity test: control flow
# All output via print() for diff comparison

print("=== if/elif/else ===")
for x in [-1, 0, 1, 5, 10]:
    if x < 0:
        print(f"{x}: negative")
    elif x == 0:
        print(f"{x}: zero")
    elif x < 5:
        print(f"{x}: small positive")
    else:
        print(f"{x}: large positive")

print("=== Ternary ===")
print("yes" if True else "no")
print("yes" if False else "no")
print("a" if 0 else "b" if 1 else "c")

print("=== for loop ===")
for i in range(5):
    print(i, end=" ")
print()

for c in "abc":
    print(c, end=" ")
print()

for k, v in {"a": 1, "b": 2}.items():
    print(f"{k}={v}", end=" ")
print()

print("=== for/else ===")
for i in range(5):
    if i == 10:
        break
else:
    print("for/else: no break")

for i in range(5):
    if i == 3:
        break
else:
    print("for/else: should not print")
print(f"broke at {i}")

print("=== while loop ===")
i = 0
while i < 5:
    print(i, end=" ")
    i += 1
print()

print("=== while/else ===")
i = 0
while i < 3:
    i += 1
else:
    print(f"while/else: completed, i={i}")

i = 0
while i < 10:
    if i == 3:
        break
    i += 1
else:
    print("while/else: should not print")
print(f"while broke at {i}")

print("=== break and continue ===")
for i in range(10):
    if i == 5:
        break
    print(i, end=" ")
print()

for i in range(10):
    if i % 2 == 0:
        continue
    print(i, end=" ")
print()

print("=== Nested loops with break/continue ===")
for i in range(3):
    for j in range(3):
        if j == 2:
            break
        print(f"({i},{j})", end=" ")
    print()

print("=== pass ===")
for i in range(3):
    pass
print("pass in loop ok")


class Empty:
    pass


print("pass in class ok")


def noop():
    pass


noop()
print("pass in function ok")

print("=== try/except ===")
try:
    x = 1 / 0
except ZeroDivisionError:
    print("caught ZeroDivisionError")

try:
    x = int("abc")
except ValueError as e:
    print(f"caught ValueError: {e}")

print("=== Multiple except ===")
for val in [0, "abc", None]:
    try:
        result = 10 / val
    except ZeroDivisionError:
        print("zero division")
    except TypeError:
        print("type error")
    except ValueError:
        print("value error")

print("=== try/except/else ===")
try:
    x = 10
except Exception:
    print("should not catch")
else:
    print(f"no exception, x={x}")

print("=== try/finally ===")
result = []
try:
    result.append("try")
finally:
    result.append("finally")
print(result)

print("=== try/except/finally ===")
result = []
try:
    result.append("try")
    raise ValueError("test")
except ValueError:
    result.append("except")
finally:
    result.append("finally")
print(result)

print("=== Nested try ===")
result = []
try:
    try:
        raise ValueError("inner")
    except ValueError:
        result.append("inner caught")
        raise TypeError("converted")
except TypeError:
    result.append("outer caught")
print(result)

print("=== with statement ===")


class CM:
    def __init__(self, name):
        self.name = name

    def __enter__(self):
        print(f"enter {self.name}")
        return self

    def __exit__(self, *args):
        print(f"exit {self.name}")
        return False


with CM("a") as x:
    print(f"inside {x.name}")

print("=== Nested with ===")
with CM("outer"):
    with CM("inner"):
        print("nested with body")

print("=== match/case ===")


def describe(val):
    match val:
        case 0:
            return "zero"
        case 1 | 2:
            return "one or two"
        case int(n) if n > 100:
            return "big int"
        case int(n):
            return f"int {n}"
        case str(s):
            return f"string: {s}"
        case [x, y]:
            return f"pair: {x}, {y}"
        case {"key": v}:
            return f"dict with key={v}"
        case _:
            return "other"


for val in [0, 1, 2, 42, 200, "hello", [1, 2], {"key": "val"}, 3.14]:
    print(f"match({val!r}) = {describe(val)}")

print("=== match sequence patterns ===")


def classify_seq(seq):
    match seq:
        case []:
            return "empty"
        case [x]:
            return f"single: {x}"
        case [x, y]:
            return f"pair: {x}, {y}"
        case [x, *rest]:
            return f"head={x}, rest={rest}"


for s in [[], [1], [1, 2], [1, 2, 3, 4]]:
    print(f"seq({s}) = {classify_seq(s)}")

print("=== Walrus operator ===")
data = [1, 2, 3, 4, 5, 6, 7, 8]
filtered = [y for x in data if (y := x * 2) > 6]
print(filtered)

if (n := 10) > 5:
    print(f"walrus: n={n}")

print("=== Short-circuit evaluation ===")


def side(name, val):
    print(f"  eval {name}", end="")
    return val


print("and short-circuit:")
result = side("A", False) and side("B", True)
print(f" -> {result}")

print("or short-circuit:")
result = side("A", True) or side("B", False)
print(f" -> {result}")

print("and no short-circuit:")
result = side("A", True) and side("B", False)
print(f" -> {result}")
