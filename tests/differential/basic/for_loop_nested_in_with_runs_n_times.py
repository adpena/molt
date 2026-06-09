"""Purpose: differential coverage for the P45 miscompile class — a `for` loop
nested inside a `with` must run N times, not once.

Root cause was the midend structural-CFG validator mis-pairing the `with`
lowering's divergent `TRY_END`s (one on the protected-body exit path, one on the
exception-handler path) against the enclosing loop's `LOOP_START`, which orphaned
the loop back-edge so the body ran a single iteration. These variants exercise
loop-in-with / with-in-loop / with-in-loop-in-with / break / continue /
exception-in-body / exception-in-__exit__ at module scope AND inside a `def`.
"""


class CM:
    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


class Suppress:
    def __enter__(self):
        return self

    def __exit__(self, *a):
        return True  # swallow the exception


# 1. The canonical repro: with > for > with, at module scope.
n = 0
with CM():
    for i in range(3):
        with CM():
            n += 1
print("module_with_for_with", n)

# 2. Same shape inside a def.
def in_def():
    n = 0
    with CM():
        for i in range(5):
            with CM():
                n += 1
    return n


print("def_with_for_with", in_def())

# 3. Loop directly inside a with (no inner with).
n = 0
with CM():
    for i in range(4):
        n += 1
print("with_for", n)

# 4. with inside the loop body, loop NOT wrapped by an outer with.
n = 0
for i in range(4):
    with CM():
        n += 1
print("for_with", n)

# 5. Three-deep: with > for > with > for > with.
total = 0
with CM():
    for i in range(3):
        with CM():
            for j in range(2):
                with CM():
                    total += 1
print("three_deep", total)

# 6. break inside for-in-with.
n = 0
with CM():
    for i in range(10):
        with CM():
            if i == 4:
                break
            n += 1
print("for_with_break", n)

# 7. continue inside for-in-with.
n = 0
with CM():
    for i in range(6):
        with CM():
            if i % 2 == 0:
                continue
            n += 1
print("for_with_continue", n)

# 8. Exception raised in the loop body, suppressed by an inner __exit__,
#    loop must still complete all iterations.
n = 0
with CM():
    for i in range(5):
        with Suppress():
            n += 1
            raise ValueError("boom")  # suppressed; loop continues
print("for_with_suppressed_exc", n)

# 9. Exception raised in __exit__ on a specific iteration, caught outside.
seen = []
try:
    with CM():
        for i in range(5):

            class ExitRaises:
                def __enter__(self):
                    return self

                def __exit__(self, *a):
                    if i == 2:
                        raise RuntimeError("exit-boom")
                    return False

            with ExitRaises():
                seen.append(i)
except RuntimeError as exc:
    seen.append("caught:" + str(exc))
print("for_with_exit_raises", seen)

# 10. Nested with inside the loop body (two withs back-to-back, no outer with).
n = 0
for i in range(3):
    with CM():
        with CM():
            n += 1
print("for_nested_with", n)

# 11. while-loop nested in with (the bug-class generalizes beyond `for`).
n = 0
with CM():
    k = 0
    while k < 4:
        with CM():
            n += 1
        k += 1
print("with_while_with", n)
