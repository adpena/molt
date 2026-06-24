"""Purpose: differential guard for IN-FUNCTION vectorized reductions (vec_* ops).

Regression anchor: commit 8b5773878 ("Extract arithmetic codegen handler")
dropped the 24 `vec_*` reduction kinds from the native backend's dispatch arm
(they are handled inside `fc::arith::handle_arith_op` via delegation to
`fc::vec_reductions`). The dropped kinds fell through the silent `_ => {}`
catch-all: no codegen emitted, the result SSA value left undefined (resolved to
the None sentinel), and every in-function accumulator loop silently miscompiled
(`TypeError: 'NoneType' object is not subscriptable` downstream). Fixed in
0323ad28c; the dispatch<->handler mirror is now derived from a single source of
truth (`fc::op_family`), but this test pins the behavior so any future drop of a
vec_* family fails the differential suite loudly rather than miscompiling.

Each accumulator loop below is the exact AST shape the frontend recognizes as a
vector reduction (single-statement body), so these functions exercise the native
vec_sum/vec_prod/vec_min/vec_max int+float lowering. Float results are chosen to
be summation-order-independent (small exact integers as floats), so molt's
reduction order cannot diverge from CPython's.
"""


# --- sum, int, over range(n) (the exact `total += i` bug repro) ---
def sum_range_aug(n):
    total = 0
    for i in range(n):
        total += i
    return total


def sum_range_assign(n):
    total = 0
    for i in range(n):
        total = total + i
    return total


# --- sum, int, over a list (iterator reduction) ---
def sum_list(xs):
    total = 0
    for v in xs:
        total += v
    return total


# --- sum, int, indexed over range(len(xs)) ---
def sum_indexed(xs):
    total = 0
    for i in range(len(xs)):
        total += xs[i]
    return total


# --- sum, float, over a list ---
def sum_float_list(xs):
    total = 0.0
    for v in xs:
        total += v
    return total


# --- sum, float, over range(n) (float accumulator over int range) ---
def sum_float_range(n):
    total = 0.0
    for i in range(n):
        total += i
    return total


# --- product, int, over range(1, n) ---
def prod_range(n):
    p = 1
    for i in range(1, n):
        p *= i
    return p


# --- product, int, over a list ---
def prod_list(xs):
    p = 1
    for v in xs:
        p = p * v
    return p


# --- min, int, over a list ---
def min_list(xs):
    m = xs[0]
    for v in xs:
        if v < m:
            m = v
    return m


# --- max, int, over a list ---
def max_list(xs):
    m = xs[0]
    for v in xs:
        if m < v:
            m = v
    return m


# A downstream index of a reduction result: this is precisely what crashed in
# the original bug (the None-sentinel result was indexed next).
def sum_then_index(n, table):
    total = sum_range_aug(n)
    return table[total % len(table)]


nums = [3, 1, 4, 1, 5, 9, 2, 6]
floats = [1.0, 2.0, 3.0, 4.0, 5.0]  # sum 15.0, order-independent in f64
table = [10, 20, 30, 40, 50]

print("sum_range_aug:", sum_range_aug(0), sum_range_aug(1), sum_range_aug(10), sum_range_aug(100))
print("sum_range_assign:", sum_range_assign(0), sum_range_assign(1), sum_range_assign(100))
print("sum_list:", sum_list([]), sum_list([7]), sum_list(nums))
print("sum_indexed:", sum_indexed([]), sum_indexed(nums))
print("sum_float_list:", sum_float_list([]), sum_float_list(floats))
print("sum_float_range:", sum_float_range(0), sum_float_range(101))
print("prod_range:", prod_range(1), prod_range(2), prod_range(6))
print("prod_list:", prod_list([5]), prod_list([1, 2, 3, 4]))
print("min_list:", min_list([7]), min_list(nums))
print("max_list:", max_list([7]), max_list(nums))
print("sum_then_index:", sum_then_index(10, table), sum_then_index(7, table))

# Type fidelity: a float reduction must stay float, an int reduction int.
print("types:", type(sum_list(nums)).__name__, type(sum_float_list(floats)).__name__)
