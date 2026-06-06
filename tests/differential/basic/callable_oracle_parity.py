"""Purpose: pin `callable()` parity across every callable/non-callable shape.

`callable()` is backed by the single runtime callability authority
(`molt_is_callable` -> `is_callable_impl`, shared with the cross-crate
`molt_is_callable_bool`). This matrix locks that authority's result for plain
functions, lambdas, closures (captured-cell functions), `*args` functions,
bound/unbound methods, classmethods/staticmethods, classes, instances with a
class-level `__call__`, builtins, and common non-callables. It exists as a
regression for the tkinter `bind` bug class where a consumer mis-decoded the
boolean result (treating the `bool` object as an int via `as_int()`), so a
genuinely-callable function read as non-callable across an ABI boundary.
Output must be byte-identical under CPython and molt-native.
"""


def plain(a, b):
    return a + b


def variadic(*args, **kwargs):
    return (args, kwargs)


def make_closure(captured):
    def inner(*event_args):
        # Captures `captured` -> compiled as a closure (cell) function, the
        # exact shape tkinter's `bind` wraps the user callback in.
        return (captured, event_args)

    return inner


lam = lambda x: x * 2  # noqa: E731


class WithCall:
    def __call__(self, *a):
        return a

    def method(self):
        return 1

    @classmethod
    def cmethod(cls):
        return cls

    @staticmethod
    def smethod():
        return 0


class NoCall:
    x = 1


def _label(name, value):
    print(f"{name}={bool(callable(value))}")


def main():
    closure = make_closure([1, 2, 3])
    inst = WithCall()
    no_call = NoCall()

    # NOTE: an instance whose `__call__` lives only in the instance `__dict__`
    # is deliberately NOT exercised here — molt currently honors instance-dict
    # `__call__` in BOTH `callable()` and the call dispatch (a self-consistent
    # but CPython-divergent behavior tracked by a separate parity baton). This
    # matrix pins only shapes where molt and CPython already agree.

    # Callables.
    _label("plain_function", plain)
    _label("variadic_function", variadic)
    _label("closure", closure)
    _label("lambda", lam)
    _label("builtin_len", len)
    _label("builtin_print", print)
    _label("class_object", WithCall)
    _label("class_no_call", NoCall)  # classes are always callable
    _label("instance_with_call", inst)
    _label("bound_method", inst.method)
    _label("unbound_method", WithCall.method)
    _label("classmethod_bound", inst.cmethod)
    _label("staticmethod", inst.smethod)
    _label("type_itself", type)
    _label("super_builtin", super)

    # Non-callables.
    _label("int", 5)
    _label("float", 1.5)
    _label("str", "hi")
    _label("list", [1, 2])
    _label("tuple", (1, 2))
    _label("dict", {"a": 1})
    _label("none", None)
    _label("bool_true", True)
    _label("instance_no_call", no_call)

    # Mirror the call (not just `callable`) for the callable cases so that a
    # divergence in callability cannot silently pass by a wrong oracle that is
    # nonetheless internally consistent.
    print("plain_call", plain(2, 3))
    print("closure_call", closure("evt"))
    print("lambda_call", lam(21))
    print("instance_call", inst("z"))
    print("bound_call", inst.method())


if __name__ == "__main__":
    main()
