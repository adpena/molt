"""
Differential parity test suite: CPython >= 3.12 vs molt.
Tests core Python semantics that must be identical across implementations.
"""

import sys

passed = 0
failed = 0
errors = []

def check(name, expr, expected=True):
    global passed, failed
    try:
        result = expr() if callable(expr) else expr
        if result == expected:
            passed += 1
        else:
            failed += 1
            errors.append(f"FAIL: {name}: got {result!r}, expected {expected!r}")
    except Exception as e:
        failed += 1
        errors.append(f"ERROR: {name}: {type(e).__name__}: {e}")

# ============================================================
# 1. Float parity (critical after Neumaier fix)
# ============================================================
print("--- 1. Float parity ---")

check("sum([0.1]*10) == 1.0",
      sum([0.1] * 10) == 1.0)

check("sum Kahan/Neumaier",
      sum([1e100, 1.0, -1e100, 1.0]) == 2.0)

check("0.1 + 0.2 value",
      0.1 + 0.2 == 0.30000000000000004)

check("repr(0.1+0.2)",
      repr(0.1 + 0.2), '0.30000000000000004')

check("repr(1/3)",
      repr(1/3), '0.3333333333333333')

check("repr(inf)",
      repr(float('inf')), 'inf')

check("repr(-inf)",
      repr(float('-inf')), '-inf')

check("repr(nan)",
      repr(float('nan')), 'nan')

check("inf > 1e308",
      float('inf') > 1e308)

check("nan != nan",
      not (float('nan') == float('nan')))

# ============================================================
# 2. Integer parity
# ============================================================
print("--- 2. Integer parity ---")

check("2**100",
      2 ** 100, 1267650600228229401496703205376)

check("(-1)**0",
      (-1) ** 0, 1)

check("(-1)**1",
      (-1) ** 1, -1)

check("10**20",
      10 ** 20, 100000000000000000000)

check("divmod(17,5)",
      divmod(17, 5), (3, 2))

check("divmod(-17,5)",
      divmod(-17, 5), (-4, 3))

# ============================================================
# 3. String parity
# ============================================================
print("--- 3. String parity ---")

check("repr('hello')",
      repr("hello"), "'hello'")

check("repr(\"it's\")",
      repr("it's"), '"it\'s"')

check('repr(\'he said "hi"\')',
      repr('he said "hi"'), '\'he said "hi"\'')

check("str * 3",
      "hello" * 3, "hellohellohello")

check("join",
      ",".join(["a", "b", "c"]), "a,b,c")

check("split",
      "hello world".split(), ["hello", "world"])

check("strip",
      "  hello  ".strip(), "hello")

# ============================================================
# 4. List parity
# ============================================================
print("--- 4. List parity ---")

def _stable_sort():
    data = [(1, 'b'), (2, 'a'), (1, 'a')]
    data.sort(key=lambda x: x[0])
    return data

check("stable sort",
      _stable_sort(), [(1, 'b'), (1, 'a'), (2, 'a')])

check("list comp squares",
      [x**2 for x in range(5)], [0, 1, 4, 9, 16])

check("list comp filter",
      [x for x in range(10) if x % 2 == 0], [0, 2, 4, 6, 8])

# ============================================================
# 5. Dict parity
# ============================================================
print("--- 5. Dict parity ---")

def _dict_order():
    d = {}
    d['c'] = 3
    d['a'] = 1
    d['b'] = 2
    return (list(d.keys()), list(d.values()))

check("dict insertion order keys",
      _dict_order()[0], ['c', 'a', 'b'])

check("dict insertion order values",
      _dict_order()[1], [3, 1, 2])

# ============================================================
# 6. Exception parity
# ============================================================
print("--- 6. Exception parity ---")

def _zdiv():
    try:
        1 / 0
    except ZeroDivisionError as e:
        return str(e)
    return None

check("ZeroDivisionError message",
      _zdiv(), 'division by zero')

def _int_valueerror():
    try:
        int("abc")
    except ValueError as e:
        return "invalid literal" in str(e)
    return False

check("ValueError int('abc')",
      _int_valueerror())

# ============================================================
# 7. Type system parity
# ============================================================
print("--- 7. Type system parity ---")

check("type(1) is int",
      type(1) is int)

check("type(1.0) is float",
      type(1.0) is float)

check("type('hello') is str",
      type("hello") is str)

check("type([]) is list",
      type([]) is list)

check("type({}) is dict",
      type({}) is dict)

check("type(()) is tuple",
      type(()) is tuple)

check("isinstance(True, int)",
      isinstance(True, int))

check("issubclass(bool, int)",
      issubclass(bool, int))

# ============================================================
# 8. Iteration parity
# ============================================================
print("--- 8. Iteration parity ---")

check("range(5)",
      list(range(5)), [0, 1, 2, 3, 4])

check("range(2,8,2)",
      list(range(2, 8, 2)), [2, 4, 6])

check("zip",
      list(zip([1,2,3], [4,5,6])), [(1,4), (2,5), (3,6)])

check("enumerate",
      list(enumerate(["a", "b"])), [(0, "a"), (1, "b")])

check("map",
      list(map(str, [1, 2, 3])), ["1", "2", "3"])

check("filter",
      list(filter(lambda x: x > 2, [1, 2, 3, 4])), [3, 4])

# ============================================================
# 9. Slicing parity
# ============================================================
print("--- 9. Slicing parity ---")

a = [0, 1, 2, 3, 4, 5]

check("a[1:4]",
      a[1:4], [1, 2, 3])

check("a[-2:]",
      a[-2:], [4, 5])

check("a[::2]",
      a[::2], [0, 2, 4])

check("a[::-1]",
      a[::-1], [5, 4, 3, 2, 1, 0])

# ============================================================
# 10. math module parity
# ============================================================
print("--- 10. math module parity ---")

import math

check("math.sqrt(4.0)",
      math.sqrt(4.0), 2.0)

check("math.floor(3.7)",
      math.floor(3.7), 3)

check("math.ceil(3.2)",
      math.ceil(3.2), 4)

check("math.pi",
      abs(math.pi - 3.141592653589793) < 1e-15)

check("math.e",
      abs(math.e - 2.718281828459045) < 1e-15)

check("math.log(e)",
      math.log(math.e), 1.0)

check("math.sin(pi/2)",
      abs(math.sin(math.pi/2) - 1.0) < 1e-15)

# ============================================================
# Summary
# ============================================================
print()
print(f"=== RESULTS: {passed} passed, {failed} failed out of {passed + failed} ===")
if errors:
    print()
    for e in errors:
        print(f"  {e}")
    print()
    sys.exit(1)
else:
    print("ALL PASSED")
