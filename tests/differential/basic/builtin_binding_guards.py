"""Builtin calls in functions stay live lookups under every rebinding path.

Molt specializes `len(xs)`, `range(n)` loops, `isinstance`, `ord` and `print`
inside functions behind a runtime binding guard (HF-79). Each case rebinds a
builtin between calls, inside loops, from another module, through globals(),
and from callbacks, and the output must match CPython exactly.
"""

import builtins
import sys

import builtin_binding_guard_support as support

THIS = sys.modules[__name__]
ORIGINAL_LEN = builtins.len
ORIGINAL_RANGE = builtins.range
ORIGINAL_PRINT = builtins.print
ORIGINAL_ISINSTANCE = builtins.isinstance
ORIGINAL_ORD = builtins.ord


def fake_len(value):
    return 99


def measure(xs):
    return len(xs)


def classify(value):
    return isinstance(value, int)


def first_code(text):
    return ord(text[0])


def count_to(n):
    total = 0
    for i in range(n):
        total += i
    return total


def loop_lengths(xs, rebind_at, rebind):
    seen = []
    for i in range(len(xs)):
        if i == rebind_at:
            rebind()
        seen.append(len(xs))
    return seen


# 1. builtins attribute rebinding between calls.
out = [measure([1, 2, 3])]
builtins.len = fake_len
out.append(measure([1, 2, 3]))
builtins.len = ORIGINAL_LEN
out.append(measure([1, 2, 3]))
print("builtins-attr", out)

# 2. builtins rebinding inside a loop, from this module and from another.
print("loop-builtins", loop_lengths([1, 2, 3, 4], 2, lambda: setattr(builtins, "len", fake_len)))
builtins.len = ORIGINAL_LEN
print("loop-other-module", loop_lengths([1, 2, 3], 1, lambda: support.set_builtin("len", fake_len)))
builtins.len = ORIGINAL_LEN
print("loop-builtins-dict", loop_lengths([5, 6], 1, lambda: support.set_builtin_item("len", fake_len)))
builtins.len = ORIGINAL_LEN

# 3. Module globals shadow the builtin: globals(), setattr, __dict__, del.
out = [measure("ab")]
globals()["len"] = fake_len
out.append(measure("ab"))
del globals()["len"]
out.append(measure("ab"))
support.set_module_attr(THIS, "len", fake_len)
out.append(measure("ab"))
support.del_module_attr(THIS, "len")
out.append(measure("ab"))
support.set_module_item(THIS, "len", fake_len)
out.append(measure("ab"))
del THIS.__dict__["len"]
out.append(measure("ab"))
print("module-shadow", out)

# 4. A shadow inserted during the loop that reads the name.
def shadow_midway(xs):
    seen = []
    for i in range(3):
        if i == 1:
            globals()["len"] = fake_len
        seen.append(len(xs))
    del globals()["len"]
    return seen


print("shadow-midway", shadow_midway([1]))

# 5. isinstance with a shadowed class operand.
out = [classify(5)]
globals()["int"] = str
out.append(classify(5))
out.append(classify("x"))
del globals()["int"]
out.append(classify(5))
print("isinstance-shadow", out)

# 6. ord rebound from another module between calls.
out = [first_code("A")]
support.set_builtin("ord", lambda text: -1)
out.append(first_code("A"))
builtins.ord = ORIGINAL_ORD
out.append(first_code("A"))
print("ord-rebind", out)

# 7. range rebound between calls and inside the loop it drives.
class CountingRange:
    def __init__(self, n):
        self.n = n

    def __iter__(self):
        return iter([100] * self.n)


out = [count_to(4)]
builtins.range = CountingRange
out.append(count_to(4))
builtins.range = ORIGINAL_RANGE
out.append(count_to(4))


def rebind_range_in_body(n):
    total = 0
    for i in range(n):
        builtins.range = CountingRange
        total += i
    builtins.range = ORIGINAL_RANGE
    return total


out.append(rebind_range_in_body(4))
out.append(count_to(3))
print("range-rebind", out)

# 8. A callback during iteration rebinds len for the next reads.
class Rebinder:
    def __init__(self, items, at):
        self.items = items
        self.at = at

    def __iter__(self):
        for index, item in enumerate(self.items):
            if index == self.at:
                builtins.len = fake_len
            yield item


def iterate_and_measure(iterable, probe):
    seen = []
    for item in iterable:
        seen.append((item, len(probe)))
    return seen


print("iteration-callback", iterate_and_measure(Rebinder("abc", 1), [0, 0]))
builtins.len = ORIGINAL_LEN

# 9. __len__ rebinds len while len runs: that call keeps the loaded builtin.
class SelfRebinding:
    def __len__(self):
        builtins.len = fake_len
        return 7


def twice(value):
    return [len(value), len(value)]


print("dunder-rebind", twice(SelfRebinding()))
builtins.len = ORIGINAL_LEN

# 10. Arguments run after the callee load: print keeps the loaded builtin.
captured = []


def capture_print(*args, **kwargs):
    captured.append(args)


def rebind_print():
    builtins.print = capture_print
    return "argument"


def print_with_callback():
    print("printed", rebind_print())
    print("captured")


print_with_callback()
builtins.print = ORIGINAL_PRINT
print("print-order", captured)

# 11. A later operand load sees a rebinding made by an earlier operand.
class Probe:
    @property
    def value(self):
        globals()["int"] = str
        return "text"


def operand_order(probe):
    return isinstance(probe.value, int)


print("operand-order", operand_order(Probe()))
del globals()["int"]

# 12. Nested builtins: the callee load precedes the argument's dunder.
class RangeRebinder:
    def __len__(self):
        builtins.range = CountingRange
        return 3


def nested_range(value):
    return [i for i in range(len(value))]


def nested_range_loop(value):
    seen = []
    for i in range(len(value)):
        seen.append(i)
    return seen


print("nested-range", nested_range_loop(RangeRebinder()), nested_range([1, 2]))
builtins.range = ORIGINAL_RANGE
print("nested-range-after", nested_range_loop([1, 2]))

# 13. Deleting the builtin makes the read fail; restoring revives it.
del builtins.len
try:
    measure([1])
except NameError as error:
    print("deleted", type(error).__name__, error)
builtins.len = ORIGINAL_LEN
print("restored", measure([1]))

# 14. Keys that shadow without being exact str.
class LenKey(str):
    pass


globals()[LenKey("len")] = fake_len
print("str-subclass-key", measure([1, 2]))
del globals()["len"]
print("str-subclass-removed", measure([1, 2]))

# 15. Generators and lambdas observe rebinding between resumptions.
def lengths_forever(xs):
    while True:
        yield len(xs)


generator = lengths_forever([1, 2])
out = [next(generator)]
builtins.len = fake_len
out.append(next(generator))
builtins.len = ORIGINAL_LEN
out.append(next(generator))
size = lambda xs: len(xs)  # noqa: E731
out.append(size("abcd"))
builtins.len = fake_len
out.append(size("abcd"))
builtins.len = ORIGINAL_LEN
out.append(support.call_through(size, "ab"))
print("generator-lambda", out)

# 16. Functions defined after a shadow still see it, and see its removal.
globals()["isinstance"] = lambda value, kind: "shadowed"


def late_classify(value):
    return isinstance(value, int)


out = [late_classify(1)]
del globals()["isinstance"]
out.append(late_classify(1))
print("late-shadow", out)
assert builtins.len is ORIGINAL_LEN and builtins.range is ORIGINAL_RANGE
