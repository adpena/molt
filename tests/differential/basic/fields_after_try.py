"""Purpose: object field state must be correct across every try/except join
(Task #20, P0 silent-wrong-value regression).

Root cause this guards against: `dead_store_elim`'s Pattern 2 dropped a stack-
promoted object's constructor field stores whenever a CFG split (try/except, or
any branch) separated the object's construction from the FIRST field read. The
object value reaches the read block via a dominance-based cross-block SSA use
that the old escape check (terminator-args only) missed, so the field read
returned a NaN-box zero default (`0.0`) instead of the stored value.

The matrix below crosses:
  - construction BEFORE the try vs INSIDE the try
  - field stores BEFORE / INSIDE / AFTER the try
  - raise vs no-raise inside the try
  - nested try/except
  - int / float / str fields (the zero-default is type-agnostic)
  - first field read BEFORE vs ONLY AFTER the try (the trigger condition)

SROA / dead-store elimination is fail-closed: a correct compile produces the
SAME result whether or not those passes fire. Output must be byte-identical to
CPython 3.14.
"""


class P:
    x: int
    y: int

    def __init__(self, x: int, y: int) -> None:
        self.x = x
        self.y = y


class Q:
    a: float
    b: str

    def __init__(self, a: float, b: str) -> None:
        self.a = a
        self.b = b


def construct_before_read_only_after() -> None:
    # The exact trigger: construct before the try, read ONLY after.
    p = P(11, 22)
    try:
        r = 1
    except ValueError:
        r = 2
    print(p.x, p.y, r)


def construct_inside_try() -> None:
    # Object built inside the try; read after the try.
    try:
        q = P(33, 44)
        r = 1
    except ValueError:
        q = P(0, 0)
        r = 2
    print(q.x, q.y, r)


def store_inside_try_with_raise() -> None:
    # A field written inside the try survives even when a raise unwinds.
    p = P(11, 22)
    try:
        p.x = 99
        raise ValueError("boom")
    except ValueError:
        pass
    print(p.x, p.y)


def store_after_try() -> None:
    p = P(1, 2)
    try:
        r = 10
    except ValueError:
        r = 20
    p.x = 7
    p.y = 8
    print(p.x, p.y, r)


def read_before_and_after_try() -> None:
    # Reading before the try must not change the after-try result.
    p = P(5, 6)
    print(p.x, p.y)
    try:
        r = 1
    except ValueError:
        r = 2
    print(p.x, p.y, r)


def float_str_fields_after_try() -> None:
    # The zero-default miscompile was type-agnostic: verify float + str fields.
    q = Q(3.5, "hello")
    try:
        r = 1
    except ValueError:
        r = 2
    print(q.a, q.b, r)


def nested_try() -> None:
    p = P(100, 200)
    try:
        try:
            inner = P(7, 8)
            raise ValueError("inner")
        except KeyError:
            inner = P(-1, -1)
        finally:
            mid = P(9, 10)
    except ValueError:
        pass
    print(p.x, p.y, inner.x, inner.y, mid.x, mid.y)


def raising_path_taken() -> None:
    # The except handler actually runs; both objects must keep their fields.
    a = P(1, 1)
    try:
        b = P(2, 2)
        raise ValueError("x")
    except ValueError:
        c = P(3, 3)
    print(a.x, a.y, b.x, b.y, c.x, c.y)


def loop_with_try_reads_after() -> None:
    # Construction inside a loop body that contains a try; read the object
    # after the loop. Exercises the same join under a back-edge.
    last = P(0, 0)
    for i in range(3):
        last = P(i, i + 1)
        try:
            _ = 1 // (i + 1)
        except ZeroDivisionError:
            pass
    print(last.x, last.y)


def main() -> None:
    construct_before_read_only_after()
    construct_inside_try()
    store_inside_try_with_raise()
    store_after_try()
    read_before_and_after_try()
    float_str_fields_after_try()
    nested_try()
    raising_path_taken()
    loop_with_try_reads_after()


main()
