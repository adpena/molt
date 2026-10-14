"""A star import made after a function's first call shadows its builtin."""


def measure(xs):
    return len(xs)


print("before", measure([1, 2]))
from builtin_binding_guard_support.star import *  # noqa: E402,F403

print("after", measure([1, 2]))
