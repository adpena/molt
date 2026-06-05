"""Purpose: differential coverage for a comprehension walrus target that is
also bound outside the comprehension, across a loop back-edge.

A walrus (``:=``) inside a comprehension leaks its binding to the enclosing
function scope (PEP 572).  When the same name is *also* bound by a
non-comprehension assignment (a ``while``/``if`` test walrus, a plain
assignment, ...) and lives across a loop iteration, the inline-comprehension
cell and the SSA-local writer must share one storage cell — otherwise the
post-loop value is the stale last comprehension value instead of the
loop-terminating binding.  Regression for the two diverging.
"""


def while_walrus_shadowed_by_comp_walrus() -> object:
    it = iter([10, 20])
    seen = []
    while (n := next(it, None)) is not None:
        # ``n`` is rebound by the comprehension walrus (leaks to this scope),
        # then the next loop turn's ``n := next(...)`` must see / overwrite the
        # same ``n``.  After the loop ``n`` is the terminating ``None``.
        inner = [n := n + 1 for _ in range(3)]
        seen.append((inner, n))
    return seen, n


print(while_walrus_shadowed_by_comp_walrus())


def for_loop_outer_assign_plus_comp_walrus() -> object:
    n = 0
    inner = []
    for _ in range(2):
        n = n + 100
        inner = [n := n + 1 for _ in range(3)]
    return n, inner


print(for_loop_outer_assign_plus_comp_walrus())


def while_walrus_distinct_comp_walrus_name() -> object:
    # Distinct names must remain independent (no over-unification).
    it = iter([10, 20])
    last = None
    while (n := next(it, None)) is not None:
        inner = [m := x + 1 for x in range(n % 3)]
        last = (n, inner, m)
    return last, n


print(while_walrus_distinct_comp_walrus_name())


def comp_walrus_only_no_outer_writer() -> object:
    # A comprehension-walrus target with no other writer keeps working: the
    # post-comp sync mirrors the cell into the local.
    total = 0
    for _ in range(2):
        vals = [y := v * v for v in range(3)]
        total += y
    return total, y, vals


print(comp_walrus_only_no_outer_writer())


def nested_while_and_comprehension_walrus() -> object:
    rows = iter([(1, 2), (3, 4)])
    out = []
    while (pair := next(rows, None)) is not None:
        a, b = pair
        squares = [(acc := a * k) for k in range(b)]
        out.append((squares, acc))
    return out, pair


print(nested_while_and_comprehension_walrus())
