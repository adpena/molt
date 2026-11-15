# Regression companion to with_loop_with_nested_backedge.py: exercise the
# break / continue / exception exit paths of a loop whose body holds a `with`
# nested inside an enclosing `with`/`try`.  These must match CPython exactly —
# the loop back-edge and the enclosing context's single exit must both survive
# the mid-end structural-CFG canonicalizer.


class CM:
    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


class CMRaise:
    def __enter__(self):
        return self

    def __exit__(self, *a):
        raise RuntimeError("exit-fail")


def with_for_with_break():
    n = 0
    with CM():
        for i in range(10):
            with CM():
                if i == 4:
                    break
                n += 1
    return n


def with_for_with_continue():
    n = 0
    with CM():
        for i in range(6):
            with CM():
                if i % 2 == 0:
                    continue
                n += 1
    return n


def exc_in_body_caught_outside():
    n = 0
    try:
        with CM():
            for i in range(5):
                with CM():
                    n += 1
                    if i == 3:
                        raise ValueError("boom")
    except ValueError:
        n += 100
    return n


def exc_in_exit_caught_outside():
    n = 0
    try:
        with CM():
            for _ in range(3):
                with CMRaise():
                    n += 1
    except RuntimeError:
        n += 100
    return n


print("break", with_for_with_break())
print("continue", with_for_with_continue())
print("exc_body", exc_in_body_caught_outside())
print("exc_exit", exc_in_exit_caught_outside())
