"""Purpose: no code in this program writes `math`'s namespace, so every
`math.f(x)` call with the intrinsic's exact positional arity is a direct call
of the intrinsic (call_anatomy.md 4.5, closed-world bindings). The results,
the exceptions (domain errors, wrong arity, keywords), edge values (negative,
NaN, infinities, -0.0, ints, bools, big ints) and reference counts must match
CPython, as must aliases, shadowing, nested functions and class bodies."""

import math
import math as m


def show(label, thunk):
    try:
        value = thunk()
    except Exception as exc:  # noqa: BLE001 - the exception is the result
        print(label, type(exc).__name__, exc)
    else:
        print(label, repr(value), math.copysign(1.0, value) if isinstance(value, float) else "")


for arg in [4.0, 2.0, 0.0, -0.0, 1e308, float("inf"), float("nan"), -1.0, -float("inf"), 9, True, 10**40, -4]:
    show(("sqrt", repr(arg)), lambda: math.sqrt(arg))
    show(("log2", repr(arg)), lambda: math.log2(arg))
    show(("floor", repr(arg)), lambda: math.floor(arg))
    show(("fabs", repr(arg)), lambda: math.fabs(arg))

# Wrong arity and keywords keep the generic call. Only the exception type is
# compared: builtin binding messages differ from CPython's (HF-842).
for label, thunk in [
    ("arity0", lambda: math.sqrt()),
    ("arity2", lambda: math.sqrt(1.0, 2.0)),
    ("keyword", lambda: math.sqrt(x=4.0)),
]:
    try:
        thunk()
    except TypeError:
        print(label, "TypeError")
show("alias", lambda: m.sqrt(16.0))
show("str", lambda: math.sqrt("4"))


def shadow_parameter(math):
    return math.sqrt(9.0)


class Fake:
    @staticmethod
    def sqrt(x):
        return ("fake", x)


print("parameter", shadow_parameter(Fake))


def shadow_local():
    math = Fake
    return math.sqrt(25.0)


print("local", shadow_local())


def outer():
    def inner(x):
        return math.sqrt(x) + m.sqrt(x)

    return inner(36.0)


print("nested", outer())


class Body:
    root = math.sqrt(49.0)


print("class body", Body.root)

total = 0.0
values = []
for index in range(20000):
    root = math.sqrt(float(index))
    total += root
    if index % 5000 == 0:
        values.append(root)
print("loop", total, values)
print("same object", math.sqrt is m.sqrt, math.sqrt(4.0) == 2.0)
