"""Purpose: differential coverage for arithmetic error message parity."""


# 1. Integer division by zero
try:
    1 // 0
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

# 2. Float division by zero
try:
    1.0 / 0.0
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

# 3. Modulo by zero (int)
try:
    5 % 0
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

# 4. Modulo by zero (float)
try:
    5.0 % 0.0
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

# 5. True division by zero (int)
try:
    1 / 0
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

# 6. divmod by zero
try:
    divmod(10, 0)
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

# 7. pow with non-int modulus
try:
    pow(2, 3, 1.5)
except TypeError as e:
    print(f"TypeError: {e}")

# 8. int too large to convert to float
try:
    float(10 ** 400)
except OverflowError as e:
    print(f"OverflowError: {e}")

# 9. Negative shift count
try:
    1 << -1
except ValueError as e:
    print(f"ValueError: {e}")

# 10. Zero to negative power (float)
try:
    0.0 ** -1
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

# 11. complex modulo
try:
    complex(1, 2) % complex(1, 0)
except TypeError as e:
    print(f"TypeError: {e}")

# 12. round with non-integer ndigits type
try:
    round(3.14, "2")
except TypeError as e:
    print(f"TypeError: {e}")


# Sequence slots own rejection diagnostics for operators and explicit base
# descriptors. CPython is the independent oracle for each complete message.
def concat_add(left, right):
    return left + right


def concat_iadd(left, right):
    left += right
    return left


def show_concat(label, operation, left, right):
    try:
        print(label, "result", operation(left, right))
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


for base, seed in ((str, "x"), (tuple, (1,)), (list, [1]),
                   (bytes, b"x"), (bytearray, b"x")):
    child = type("Child" + base.__name__, (base,), {})
    for value in (base(seed), child(seed)):
        label = type(value).__name__
        show_concat(label + "+", concat_add, value, 1)
        show_concat(label + "-descriptor", base.__add__, value, 1)
        if base is not list:
            show_concat(label + "+=", concat_iadd, value, 1)


class ReflectedConcat:
    def __radd__(self, other):
        print("concat-reflected", type(other).__name__)
        return "reflected-result"


class DeclinedConcat:
    def __radd__(self, other):
        print("concat-declined", type(other).__name__)
        return NotImplemented


class RaisedConcat:
    def __radd__(self, other):
        raise ValueError("reflected-concat-error")


for value in ("x", (1,), [1], b"x", bytearray(b"x")):
    show_concat("reflected", concat_add, value, ReflectedConcat())
    show_concat("declined", concat_add, value, DeclinedConcat())
show_concat("raised", concat_add, "x", RaisedConcat())


# A user class spelling is never evidence of a builtin sequence slot.
class NamedLikeSequence:
    def __add__(self, other):
        return NotImplemented


for name in ("str", "list", "tuple", "bytes"):
    NamedLikeSequence.__name__ = name
    show_concat("spelled-" + name, concat_add, NamedLikeSequence(), 1)


# Diagnostic precision counts UTF-8 bytes, including a split final character.
long_right = type("r" * 199 + "é", (), {})()
for value in ("x", (1,), [1]):
    show_concat("long-sequence-name", concat_add, value, long_right)
shorter_right = type("r" * 99 + "é", (), {})()
for base in (bytes, bytearray):
    child = type("l" * 99 + "é", (base,), {})
    show_concat("long-buffer-names", concat_add, child(b"x"), shorter_right)
    if base is bytearray:
        show_concat("long-buffer-inplace-names", concat_iadd, child(b"x"), shorter_right)
