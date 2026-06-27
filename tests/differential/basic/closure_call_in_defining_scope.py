# Regression: a closure that CAPTURES an enclosing variable and is CALLED
# within its defining scope must not miscompile.
#
# Bug (molt task #44): the TIR inliner (runtime/molt-passes/src/tir/passes/
# inliner.rs) spliced such a closure into its caller and bound the closure's
# implicit env parameter (`__molt_closure__`, always param[0] of a closure) to
# the call's *function-value* operand instead of the captured environment. The
# inlined body then subscripted that function object
# (`__molt_closure__[0]` -> the capture cell), raising
# `TypeError: 'function' object is not subscriptable`.
#
# Root cause: `is_inlineable` did not exclude closures, and the arity guard in
# `splice_call_site` (`callee_entry.args.len() == call_args.len()`) accidentally
# matched because a closure adds exactly one param (`__molt_closure__`), which
# re-balances against the call's leading function-value operand. Non-closures
# are protected by that same arity guard (callee has N params, the Call carries
# N+1 operands -> mismatch -> refused), so only env-capturing closures tripped.
#
# Every shape below is byte-identical across CPython 3.12 / 3.13 / 3.14.


def plain_capture_call(base):
    def add(x):
        return base + x

    return add(10)


def no_arg_capture_call(base):
    def get():
        return base + 1

    return get()


def capture_local_not_param():
    base = 5

    def add(x):
        return base + x

    return add(10)


def capture_then_alias_then_call(base):
    def add(x):
        return base + x

    g = add
    return g(10)


def capture_call_and_return(base):
    def add(x):
        return base + x

    r = add(1)
    return add(100) + r


def capture_call_in_loop(base):
    def add(x):
        return base + x

    total = 0
    for i in range(3):
        total += add(i)
    return total


def nested_two_levels(base):
    def mid(y):
        def inner(z):
            return base + y + z

        return inner(100)

    return mid(10)


def multi_capture(a, b):
    def combine(x):
        return a + b + x

    return combine(7)


class Holder:
    def method(self, base):
        def add(x):
            return base + x

        return add(10)


print(plain_capture_call(5))
print(no_arg_capture_call(5))
print(capture_local_not_param())
print(capture_then_alias_then_call(5))
print(capture_call_and_return(5))
print(capture_call_in_loop(5))
print(nested_two_levels(5))
print(multi_capture(3, 4))
print(Holder().method(5))
