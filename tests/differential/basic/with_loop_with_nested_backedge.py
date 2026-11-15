# Regression: a `for`/`while` loop whose body contains a `with` (try region),
# itself nested inside an enclosing `with`/`try`, must iterate N times — not
# once.  The mid-end structural-CFG canonicalizer used to mis-match the inner
# `with`'s exception-path `TRY_END` (the twin marker emitted after the handler
# label) to the *outer* `try`/`with`'s `TRY_START`, dismantling the intervening
# loop scope and eliding its `LOOP_CONTINUE`/back-edge.  The loop then fell
# through after a single iteration (silent miscompile).


class CM:
    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


def with_for_with():
    n = 0
    with CM():
        for _ in range(5):
            with CM():
                n += 1
    return n


def with_while_with():
    n = 0
    with CM():
        i = 0
        while i < 5:
            with CM():
                n += 1
            i += 1
    return n


def three_deep():
    n = 0
    with CM():
        for _ in range(3):
            with CM():
                for _ in range(2):
                    with CM():
                        n += 1
    return n


def two_inner_withs():
    n = 0
    with CM():
        for _ in range(4):
            with CM():
                with CM():
                    n += 1
    return n


# Module-level (the originally reported shape: not inside a def).
m = 0
with CM():
    for _ in range(3):
        with CM():
            m += 1

print("with_for_with", with_for_with())
print("with_while_with", with_while_with())
print("three_deep", three_deep())
print("two_inner_withs", two_inner_withs())
print("module_level", m)
