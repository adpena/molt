"""Purpose: a raise inside a try leaves at once; the code after it never runs.

Each case raises from a compiler-generated check inside an `if`, then has an
observable side effect that CPython never performs (HF-213).
"""


def unbound_local_in_loop(xs):
    out = []
    for x in xs:
        if x > 0:
            v = x
        try:
            out.append(v)
        except UnboundLocalError:
            out.append("unbound")
    return out


def unbound_local_once(flag):
    out = []
    if flag:
        v = 1
    try:
        out.append(v)
    except UnboundLocalError:
        out.append("unbound")
    return out


def unbound_free_variable():
    out = []

    def inner():
        try:
            out.append(captured)
        except NameError:
            out.append("free unbound")

    inner()
    captured = 1
    inner()
    return out


def range_zero_step(step):
    out = []
    try:
        for i in range(0, 3, step):
            out.append(i)
        out.append("after loop")
    except ValueError as exc:
        out.append(f"ValueError: {exc}")
    return out


def assert_then_side_effect(flag):
    out = []
    try:
        assert flag, "flag is false"
        out.append("asserted")
    except AssertionError as exc:
        out.append(f"AssertionError: {exc}")
    return out


def generator_throw():
    out = []

    def gen():
        try:
            yield 1
            out.append("resumed")
        except KeyError:
            out.append("caught in generator")
            raise

    g = gen()
    next(g)
    try:
        g.throw(KeyError("k"))
    except KeyError:
        out.append("propagated")
    return out


print(unbound_local_in_loop([-1, 2]))
print(unbound_local_once(False))
print(unbound_local_once(True))
print(unbound_free_variable())
print(range_zero_step(0))
print(range_zero_step(1))
print(assert_then_side_effect(False))
print(assert_then_side_effect(True))
print(generator_throw())
