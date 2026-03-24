"""Purpose: differential coverage for RuntimeError message parity."""


# 1. Maximum recursion depth
import sys
sys.setrecursionlimit(50)

def recurse(n):
    return recurse(n + 1)

try:
    recurse(0)
except RecursionError as e:
    print(f"RecursionError: {e}")

sys.setrecursionlimit(1000)


# 2. StopIteration raised inside generator
def gen_stop():
    it = iter([1])
    next(it)
    next(it)

try:
    gen_stop()
except StopIteration:
    print("StopIteration raised")


# 3. Generator already executing (via send to running generator)
# This is hard to trigger deterministically, test with throw instead
def gen_throw():
    try:
        yield 1
    except ValueError:
        yield "caught"

g = gen_throw()
print("gen-next", next(g))
print("gen-throw", g.throw(ValueError("test-error")))


# 4. Dictionary changed size during iteration
try:
    d = {1: "a", 2: "b", 3: "c"}
    for k in d:
        if k == 1:
            d[99] = "new"
except RuntimeError as e:
    print(f"RuntimeError: {e}")


# 5. Set changed size during iteration
try:
    s = {1, 2, 3}
    for item in s:
        if item == 1:
            s.add(99)
except RuntimeError as e:
    print(f"RuntimeError: {e}")


# 6. Super() with no arguments outside class
try:
    super()
except RuntimeError as e:
    print(f"RuntimeError: {e}")


# 7. Reuse exhausted generator
def simple_gen():
    yield 1
    yield 2

g = simple_gen()
print("gen-1", next(g))
print("gen-2", next(g))
try:
    next(g)
except StopIteration:
    print("generator-exhausted")


# 8. NotImplementedError
try:
    raise NotImplementedError("abstract method")
except NotImplementedError as e:
    print(f"NotImplementedError: {e}")


# 9. Assert with message
try:
    assert False, "custom assertion message"
except AssertionError as e:
    print(f"AssertionError: {e}")


# 10. Assert without message
try:
    assert False
except AssertionError as e:
    print(f"AssertionError: {e!r}")
