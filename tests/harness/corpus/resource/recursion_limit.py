# Resource enforcement scenario: deep recursion must raise RecursionError.
# Expected: RecursionError when max_recursion_depth=50.
# Note: RecursionError IS catchable (CPython compat).
def recurse(n):
    return recurse(n + 1)

try:
    recurse(0)
except RecursionError:
    print("RecursionError caught correctly")
