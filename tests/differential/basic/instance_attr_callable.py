# MOLT_META: expect_fail=molt expect_fail_reason=backend_escape_analysis_stack_promotes_object_with_out_of_layout_attr
"""Purpose: storing a plain function in an instance attribute and then calling
it through the attribute must invoke the function (no implicit self binding).

`g.method = fn; g.method()` -- a function assigned to an *instance* attribute is
NOT a bound method (descriptor binding only applies to functions found on the
*class*).  So `g.method()` calls fn with no arguments.

XFAIL (pre-existing BACKEND bug, not the frontend super fix in this change):
inside a function, `g.method = fn; g.method()` raises AttributeError. Root cause
is in the backend TIR escape analysis: a non-escaping `object_new_bound` is
stack-promoted (`object_new_bound_stack`) with a FIXED layout sized to the
class's declared fields, but the instance then receives an out-of-layout
attribute store (`set_attr_generic_ptr method`) that needs a `__dict__` the
fixed stack slot cannot hold. The store is dropped/no-ops and the later load
fails. Reproduces only in function scope (module scope is correct, not
stack-promoted). Fix: the escape analysis must treat an out-of-layout
attribute store as forcing heap allocation
(runtime/molt-tir/src/tir/passes/escape_analysis.rs), outside this change's
frontend lane. See the session baton.

Output must be byte-identical to CPython 3.14.
"""


class Gadget:
    def __init__(self) -> None:
        self.name = "gadget"


def make_action() -> str:
    return "action!"


def add(a: int, b: int) -> int:
    return a + b


def main() -> None:
    g = Gadget()
    # Store a plain function in an instance attribute, then call it.
    g.method = make_action
    print(g.method())

    # Instance-attribute function taking args -- no implicit self binding.
    g.op = add
    print(g.op(3, 4))

    # Reassign and call again.
    g.method = lambda: "lambda!"
    print(g.method())

    # A function stored on the instance shadowing a same-named class method
    # must use the instance attribute (no self binding) when called.
    g.describe = lambda: "instance-describe"
    print(g.describe())


class WithMethod:
    def __init__(self) -> None:
        self.handler = None

    def describe(self) -> str:
        return "class-describe:" + str(self)


def main2() -> None:
    w = WithMethod()
    # Instance attribute holds a plain function; class also defines describe.
    w.handler = make_action
    print(w.handler())
    # The class method is still bound normally.
    print(w.describe()[:14])


if __name__ == "__main__":
    main()
    main2()
