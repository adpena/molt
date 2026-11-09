"""Purpose: differential coverage for int rounding dunders, __subclasshook__ and
BaseException.__setstate__ (HF-815 B3).

Each case prints its result or its exception type, so a missing method shows
as AttributeError and a wrong result as a different line.
"""


def attempt(label, thunk):
    try:
        print(label, repr(thunk()))
    except Exception as error:
        print(label, type(error).__name__)


class P:
    pass


class Q(P):
    pass


class Big(int):
    pass


# int.__floor__, __ceil__, __trunc__ return the exact int, for every int kind.
for value in (0, 7, -7, 2**70, -(2**70), True, False, Big(5)):
    attempt(f"floor {value!r}", lambda value=value: value.__floor__())
    attempt(f"ceil {value!r}", lambda value=value: value.__ceil__())
    attempt(f"trunc {value!r}", lambda value=value: value.__trunc__())
    attempt(
        f"types {value!r}",
        lambda value=value: (
            type(value.__floor__()).__name__,
            type(value.__ceil__()).__name__,
            type(value.__trunc__()).__name__,
        ),
    )
attempt("int.__floor__ unbound", lambda: int.__floor__(9))
attempt("int.__trunc__ wrong self", lambda: int.__trunc__(1.5))

# object.__subclasshook__ is a classmethod that always answers NotImplemented.
attempt("object hook", lambda: object.__subclasshook__(int))
attempt("P hook", lambda: P.__subclasshook__(Q))
attempt("instance hook", lambda: P().__subclasshook__(int))
attempt("hook is NotImplemented", lambda: P.__subclasshook__(int) is NotImplemented)
attempt("hook arity", lambda: object.__subclasshook__())
attempt("issubclass unaffected", lambda: (issubclass(Q, P), issubclass(int, P)))

# BaseException.__setstate__ restores attributes from a dict; None is a no-op.
error = ValueError("message")
attempt("setstate dict", lambda: error.__setstate__({"code": 3, "hint": "x"}))
attempt("restored", lambda: (error.code, error.hint, error.args))
attempt("setstate none", lambda: error.__setstate__(None))
attempt("after none", lambda: error.code)
attempt("setstate bad", lambda: error.__setstate__(5))
attempt("setstate bad key", lambda: error.__setstate__({1: 2}))
attempt("reduce round trip", lambda: error.__reduce__()[2]["code"])
copy = ValueError(*error.args)
copy.__setstate__(error.__reduce__()[2])
attempt("copy", lambda: (copy.args, copy.code, copy.hint))
