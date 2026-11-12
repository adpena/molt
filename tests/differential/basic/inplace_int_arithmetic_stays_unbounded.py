"""Purpose: int arithmetic keeps Python's unbounded results on every lowering lane.

Guards the SimpleIR constant folders, which fold with i64 wrapping (HF-214), and
the backends' integer fast lanes: a sum, product or left shift whose i64 result
wraps back into the 47-bit inline range [-2**46, 2**46 - 1] must still give the
exact Python integer. Each case is chosen so that its wrapped i64 value is an
inline int: a lane that tests only the inline range keeps the wrong value.
Constant operands reach the lanes after TIR forwards them; text operands reach
them unproven.
"""

TOP = 70368744177663        # 2**46 - 1, the inline maximum
BOTTOM = -70368744177664    # -2**46, the inline minimum


def square_in_place():
    x = 3037000500          # inside the 47-bit inline range
    x *= x                  # 9223372037000250000 > i64::MAX
    return x


def square_to_zero():
    y = 1099511627776       # 2**40
    y *= y                  # 2**80; wraps to 0 in i64
    return y


def sum_in_place():
    z = -70368744177664     # inline minimum
    z -= 70368744177664     # stays in i64, leaves the 47-bit range
    return z


def products_of_constants():
    a = 1099511627776       # 2**40
    b = 16777217            # 2**24 + 1: a * b wraps to 2**40
    k = 262144              # 2**18
    top = 70368744177663    # top * k wraps to -2**18
    bottom = -70368744177664  # bottom * k wraps to 0
    return a * b, -a * b, a * -b, top * k, bottom * k, bottom * -k


def sums_beyond_i64():
    big = 9223372036854775807     # i64::MAX: big + big wraps to -2
    small = -9223372036854775808  # i64::MIN: small - big wraps to 1
    return big + big, small - big, small + small


def sums_of_folded_products():
    a = 3037000499          # a * a = 9223372030926249001: i64, not inline
    square = a * a          # TIR folds it to a constant outside the inline range
    negative = -square
    # Each result wraps to +-11853105614, an inline int.
    return square + square, negative - square, negative + negative, square * 2, square << 1


def in_place_beyond_i64():
    x = 9223372036854775807
    x += 9223372036854775807
    y = -9223372036854775808
    y -= 9223372036854775807
    p = 1099511627776
    p *= 16777217
    q = 70368744177663
    q <<= 18
    return x, y, p, q


def shifts_of_constants():
    top = 70368744177663    # top << 18 wraps to -2**18
    bottom = -70368744177664  # bottom << 18 wraps to 0
    one = 1
    three = 3
    return top << 18, bottom << 18, one << 63, three << 62, one << 64, -one << 63


def shifted_past_the_word(xs):
    # A count of 64 or more gives an inline result (0 or -1) but has no machine
    # shift: LLVM makes it poison and WASM takes the count mod 64. The
    # representation facts keep such a shift off the raw lanes.
    n = len(xs)
    zero = len(xs) & 0
    return n >> 63, n >> 64, n >> 100, -n >> 64, -n >> 100, zero << 64, zero << 100


def products_from_text(a_text, b_text):
    a = int(a_text)
    b = int(b_text)
    product = a
    product *= b
    return a * b, a * -b, -a * b, product


def shifts_from_text(a_text, count_text):
    a = int(a_text)
    count = int(count_text)
    shifted = a
    shifted <<= count
    return a << count, -a << count, shifted


def powers_from_text(a_text):
    a = int(a_text)
    return a ** 2, (-a) ** 2, a ** 3


print(square_in_place())
print(square_to_zero())
print(sum_in_place())
print(products_of_constants())
print(sums_beyond_i64())
print(sums_of_folded_products())
print(in_place_beyond_i64())
print(shifts_of_constants())
print(shifted_past_the_word([1, 2, 3]))
print(products_from_text("1099511627776", "16777217"))
print(products_from_text("1099511627776", "1099511627776"))
print(products_from_text(str(TOP), "262144"))
print(products_from_text(str(BOTTOM), "262144"))
print(products_from_text(str(TOP), str(TOP)))
print(products_from_text(str(BOTTOM), str(BOTTOM)))
print(shifts_from_text(str(TOP), "18"))
print(shifts_from_text(str(BOTTOM), "18"))
print(shifts_from_text("1", "63"))
print(shifts_from_text("1", "64"))
print(powers_from_text("4294967296"))
print(powers_from_text("2097152"))
