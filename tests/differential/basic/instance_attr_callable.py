"""Purpose: storing a plain function in an instance attribute and then calling
it through the attribute must invoke the function (no implicit self binding).

`g.method = fn; g.method()` -- a function assigned to an *instance* attribute is
NOT a bound method (descriptor binding only applies to functions found on the
*class*). So `g.method()` calls fn with no arguments.

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
