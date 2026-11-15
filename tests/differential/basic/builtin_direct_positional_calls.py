"""Proven builtins called with plain positional arguments compile to direct
runtime calls; defaults, exhaustion and error messages must stay CPython's."""

it = iter([1, 2])
print(next(it), next(it), next(it, "done"))
try:
    next(it)
except StopIteration as exc:
    print("StopIteration", exc.args)
try:
    next(42)
except TypeError as exc:
    print("TypeError", exc)


class Box:
    value = 7


print(getattr(Box, "value"), getattr(Box, "absent", None))
try:
    getattr(Box, "absent")
except AttributeError as exc:
    print("AttributeError", exc)
print(divmod(17, 5), round(2.675, 2), round(7), abs(-3), len("four"))
print(isinstance(3, int), issubclass(bool, int), hash(5) == hash(5))
print(list(iter(iter([3, 4]).__next__, 4)))


def shadowed(values):
    def next(x, default=None):  # a local binding is not the builtin
        return ("local", x, default)

    return next(values)


print(shadowed([9]))
