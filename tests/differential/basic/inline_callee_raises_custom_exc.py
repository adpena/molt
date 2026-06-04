# An inlinable callee that raises a user-defined Exception subclass with both
# string and int-derived payloads, caught after propagating through an
# intermediate inlinable frame. Verifies the inlined raise preserves the
# exception object identity/message across the inlining boundary.
# Byte-identical to CPython 3.12/3.13/3.14.


class MyError(Exception):
    pass


def check(n):
    if n < 0:
        raise MyError("negative: " + str(n))
    return n * 2


def process(n):
    return check(n) + 1


for v in [3, -1, 0, -5, 7]:
    try:
        print(process(v))
    except MyError as e:
        print("caught:", e)
