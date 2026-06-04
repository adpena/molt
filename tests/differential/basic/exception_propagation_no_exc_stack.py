"""Purpose: a function with no syntactic try/except/with/raise (especially a
LAMBDA) that calls a raising callee must propagate the exception, not silently
return None.

Regression for the `needs_exception_stack` polarity trap (foundation Tier-1 C2).
`_function_needs_exception_stack` opted a function OUT of exception bookkeeping
by a syntactic scan, but a raising callee sets the runtime exception-pending
flag regardless.  A `needs_exception_stack=False` function (every lambda was
always False) with a raising call — `lambda: int("x")`, `lambda: {}["k"]`,
`lambda: o.missing`, a nested raising lambda, a raising comprehension — left an
UNOBSERVED pending exception and returned None, so an outer `try` caught
nothing and the program printed the silent-None result instead of the
exception.

The fix DELETES `_function_needs_exception_stack`: every function now carries a
function-level exception label (needs_exception_stack defaults to True), and the
oracle-driven `check_exception_elim` TIR pass removes the redundant per-op
CHECK_EXCEPTION ops — so exception observation survives exactly where a may-raise
op precedes it.  The bug class is un-expressible.

All exception messages here are version-stable across CPython 3.12/3.13/3.14.
"""


def show(label, fn):
    try:
        r = fn()
        print(label, "OK", repr(r))
    except Exception as e:
        print(label, type(e).__name__, str(e))


# --- Lambda calling a raising builtin (the canonical bug: `lambda: int("x")`).
show("lambda_int", lambda: int("x"))
show("lambda_float", lambda: float("nope"))

# --- Lambda calling a raising method.
show("lambda_strip_index", lambda: "abc".index("z"))
show("lambda_list_remove", lambda: [1, 2, 3].remove(99))

# --- Lambda performing a raising subscript (KeyError / IndexError).
show("lambda_keyerror", lambda: {"a": 1}["b"])
show("lambda_indexerror", lambda: [1, 2][5])

# --- Lambda performing a raising getattr on an object with no such attr.
class Plain:
    pass


_p = Plain()
show("lambda_getattr", lambda: _p.missing_attr)

# --- Lambda performing a raising getattr on None.
show("lambda_getattr_none", lambda: None.whatever)

# --- Lambda whose body is a raising comprehension (eager iterator consumer).
def raising_gen():
    yield 1
    yield 2
    raise ValueError("gen-boom")


show("lambda_comprehension", lambda: [y for y in raising_gen()])
show("lambda_list_of_gen", lambda: list(raising_gen()))

# --- Exception chains through NESTED lambdas: an outer lambda calls an inner
#     lambda that raises.  Both are needs_exception_stack=False historically.
_inner = lambda: int("still-bad")
_outer = lambda: _inner() + 1
show("nested_lambda", _outer)

# --- Triple-nested lambda chain.  (Uses an empty-list pop for a message that is
#     stable across CPython 3.12/3.13/3.14, unlike ZeroDivisionError which
#     changed wording in 3.14.)
_a = lambda: [].pop()
_b = lambda: _a()
_c = lambda: _b()
show("triple_nested_lambda", _c)

# --- A plain (non-lambda) function with no try/except/with/raise that calls a
#     raising callee.  This was also needs_exception_stack=False.
def plain_no_try():
    return int("plain-bad")


show("plain_function", plain_no_try)


def plain_calls_lambda():
    f = lambda: {}["missing"]
    return f()


show("plain_calls_lambda", plain_calls_lambda)

# --- Arithmetic overflow-into-raise inside a lambda (TypeError on bad operand).
show("lambda_type_error", lambda: 1 + "two")

# --- Lambda map/filter style: the raising call is the lambda's whole body but
#     the exception originates several frames deep.
def level3():
    raise RuntimeError("deep")


def level2():
    return level3()


def level1():
    return level2()


show("deep_call_chain", lambda: level1())

# --- After all the raises, the program must still run normally: a lambda that
#     does NOT raise returns its value, and the pending-exception machinery has
#     not corrupted state.
show("lambda_ok", lambda: 2 + 3)
print("done")
