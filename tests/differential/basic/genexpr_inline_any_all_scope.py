"""Purpose: differential coverage for inline ``any``/``all`` genexpr scoping.

Regression for the inline-any/all path in ``visit_Call``.  Previously the
inline lowering aliased the genexpr target onto the enclosing function's
LOAD_VAR slot for the same name.  When the outer function later assigned
to that name (e.g. ``for prod in products:``), reads inside the genexpr
body resolved through the function-level slot — uninitialised at that
point — instead of the iteration value.  CPython treats the genexpr
target as belonging to its own scope.
"""


def f():
    products = [4, 10, 18]
    if all(isinstance(prod, int) for prod in products):
        print("a")
    total = 0
    for prod in products:
        total += prod
    print("total:", total)


f()


def g():
    items = [4, 10, 18]
    if any(isinstance(it, int) for it in items):
        print("b")
    total = 0
    for it in items:
        total += it
    print("total2:", total)


g()


def h():
    # Outer ``x`` bound BEFORE inline genexpr: the comp must not leak its
    # iteration value into the enclosing scope.  CPython preserves outer ``x``.
    x = "before"
    print(all(isinstance(x, int) for x in [1, 2, 3]))
    print(x)


h()


def i():
    # Same name re-used by both the inline genexpr and a subsequent for-loop.
    # The genexpr should not see the outer for-loop's slot.
    xs = [1, 2, 3]
    print(all(v > 0 for v in xs))
    print(any(v > 5 for v in xs))
    for v in xs:
        pass
    print("post:", v)


i()
