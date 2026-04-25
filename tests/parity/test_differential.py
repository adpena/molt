#!/usr/bin/env python3
"""Molt differential testing harness — compares molt output to CPython."""

# Each test is a small Python snippet. We compile with molt, run, and
# compare output to CPython. Any divergence is a bug.

# === BASIC TYPES ===
# 1. Integer arithmetic
print("int:", 1 + 2, 3 * 4, 10 // 3, 10 % 3, 2 ** 10)
# 2. Float arithmetic
print("float:", 1.5 + 2.5, 3.0 * 4.0, 10.0 / 3.0)
# 3. String operations
print("str:", "hello" + " " + "world", "ab" * 3, len("test"))
# 4. Bool operations
print("bool:", True and False, True or False, not True)
# 5. None
print("none:", None is None, None is not None)

# === CONTAINERS ===
# 6. List
lst = [1, 2, 3]
lst.append(4)
print("list:", lst, len(lst), lst[0], lst[-1])
# 7. Dict
d = {"a": 1, "b": 2}
d["c"] = 3
print("dict:", sorted(d.keys()), d["a"], len(d))
# 8. Tuple
t = (1, 2, 3)
print("tuple:", t, len(t), t[0])
# 9. Set
s = {1, 2, 3, 2, 1}
print("set:", sorted(s), len(s))

# === CONTROL FLOW ===
# 10. if/elif/else
x = 5
if x > 10:
    print("big")
elif x > 3:
    print("medium")
else:
    print("small")
# 11. for loop
total = 0
for i in range(5):
    total += i
print("for:", total)
# 12. while loop
n = 0
while n < 5:
    n += 1
print("while:", n)
# 13. break/continue
vals = []
for i in range(10):
    if i == 3:
        continue
    if i == 7:
        break
    vals.append(i)
print("break_continue:", vals)

# === FUNCTIONS ===
# 14. Basic function
def add(a, b):
    return a + b
print("func:", add(3, 4))
# 15. Default args
def greet(name, greeting="Hello"):
    return f"{greeting}, {name}!"
print("default:", greet("World"), greet("Bob", "Hi"))
# 16. *args, **kwargs
def variadic(*args, **kwargs):
    return len(args), sorted(kwargs.keys())
print("variadic:", variadic(1, 2, 3, x=4, y=5))
# 17. Lambda
sq = lambda x: x * x
print("lambda:", sq(5))
# 18. Closure
def make_adder(n):
    def adder(x):
        return x + n
    return adder
add5 = make_adder(5)
print("closure:", add5(10))
# 19. Recursion
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)
print("recursion:", fib(10))

# === CLASSES ===
# 20. Basic class
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y
    def __repr__(self):
        return f"Point({self.x}, {self.y})"
p = Point(3, 4)
print("class:", p, p.x, p.y)
# 21. Inheritance
class Point3D(Point):
    def __init__(self, x, y, z):
        super().__init__(x, y)
        self.z = z
    def __repr__(self):
        return f"Point3D({self.x}, {self.y}, {self.z})"
p3 = Point3D(1, 2, 3)
print("inherit:", p3)
# 22. @classmethod
class Counter:
    count = 0
    @classmethod
    def increment(cls):
        cls.count += 1
        return cls.count
print("classmethod:", Counter.increment(), Counter.increment())
# 23. @staticmethod
class Math:
    @staticmethod
    def square(x):
        return x * x
print("staticmethod:", Math.square(7))

# === EXCEPTION HANDLING ===
# 24. try/except
try:
    x = 1 / 0
except ZeroDivisionError:
    print("except: caught")
# 25. try/except/finally
result = []
try:
    result.append("try")
finally:
    result.append("finally")
print("finally:", result)
# 26. raise and catch
try:
    raise ValueError("test")
except ValueError as e:
    print("raise:", str(e))

# === COMPREHENSIONS ===
# 27. List comprehension
print("listcomp:", [x * x for x in range(5)])
# 28. Dict comprehension
print("dictcomp:", {k: k * 2 for k in range(3)})
# 29. Set comprehension
print("setcomp:", sorted({x % 3 for x in range(10)}))
# 30. Nested comprehension
print("nested:", [x + y for x in range(3) for y in range(3)])

# === STRING OPERATIONS ===
# 31. f-strings
name = "molt"
print("fstring:", f"hello {name}!", f"{1 + 2}")
# 32. String methods
print("methods:", "hello world".split(), "HELLO".lower(), "hello".upper())
# 33. String slicing
s = "abcdefgh"
print("slice:", s[1:4], s[:3], s[5:], s[::2], s[::-1])

# === UNPACKING ===
# 34. Tuple unpacking
a, b, c = 1, 2, 3
print("unpack:", a, b, c)
# 35. Star unpacking
first, *rest = [1, 2, 3, 4, 5]
print("star:", first, rest)
# 36. Swap
x, y = 10, 20
x, y = y, x
print("swap:", x, y)

# === IMPORTS ===
# 37. json
import json
print("json:", json.dumps({"key": "value"}))
# 38. os
import os
print("os:", type(os.sep).__name__)
# 39. sys
import sys
print("sys:", sys.maxsize > 0)

# === GENERATORS ===
# 40. Generator function
def gen():
    yield 1
    yield 2
    yield 3
print("gen:", list(gen()))

# === MISC ===
# 41. Chained comparison
print("chain:", 1 < 2 < 3, 1 < 2 > 3)
# 42. Ternary
print("ternary:", "yes" if True else "no", "yes" if False else "no")
# 43. Multiple return values
def multi():
    return 1, 2, 3
print("multi:", multi())
# 44. Global
counter = 0
def inc():
    global counter
    counter += 1
inc()
inc()
print("global:", counter)
# 45. Walrus operator
if (n := 10) > 5:
    print("walrus:", n)

print("ALL PARITY TESTS PASSED")
