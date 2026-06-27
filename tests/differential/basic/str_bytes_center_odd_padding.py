"""Purpose: str/bytes/bytearray .center() must split odd padding like CPython.

CPython's ``str.center``/``bytes.center``/``bytearray.center`` share
``stringlib_center`` (Objects/stringlib/transmogrify.h):

    marg = width - len(self)
    left = marg / 2 + (marg & width & 1)

so the extra fill goes on the RIGHT unless BOTH the total padding ``marg`` and
the target ``width`` are odd. This is intentionally *different* from the format
mini-language ``^`` alignment (Python/formatter_unicode.c ``calculate_padding``),
where ``left = pad / 2`` always puts the extra on the right. Both rules are
covered here so a fix to one cannot silently break the other.
"""


def show(value):
    print(ascii(value))


# --- str.center: extra-right when width even & pad odd; extra-left when both odd
show("abc".center(6))      # ' abc  '   (width 6 even, pad 3 -> extra right)
show("a".center(4))        # ' a  '     (width 4 even, pad 3 -> extra right)
show("hello".center(8))    # ' hello  ' (width 8 even, pad 3 -> extra right)
show("ab".center(5))       # '  ab '    (width 5 odd, pad 3 -> extra left)
show("xy".center(7))       # '   xy  '  (width 7 odd, pad 5 -> extra left)
show("ab".center(7))       # '   ab  '  (width 7 odd, pad 5 -> extra left)
show("abcd".center(7))     # '  abcd '  (width 7 odd, pad 3 -> extra left)
show("q".center(6))        # '  q   '   (width 6 even, pad 5 -> extra right)
show("z".center(8))        # '   z    ' (width 8 even, pad 7 -> extra right)
show("".center(5))         # '     '
show("".center(4))         # '    '
show("abc".center(7))      # '  abc  '  (pad 4 even -> symmetric)
show("hi".center(2))       # 'hi'       (width <= len -> unchanged)
show("hello".center(3))    # 'hello'    (width < len)
show("ab".center(5, "*"))  # '**ab*'    (custom fill, extra left)
show("abc".center(6, "-")) # '-abc--'   (custom fill, extra right)

# --- bytes.center / bytearray.center: same rule
show(b"ab".center(5))         # b'  ab '
show(b"abc".center(6))        # b' abc  '
show(b"xy".center(7))         # b'   xy  '
show(b"abcd".center(7))       # b'  abcd '
show(b"q".center(6))          # b'  q   '
show(b"ab".center(5, b"*"))   # b'**ab*'
show(bytearray(b"ab").center(5))    # bytearray(b'  ab ')
show(bytearray(b"abc").center(6))   # bytearray(b' abc  ')
show(bytearray(b"xy").center(7, b"."))  # bytearray(b'...xy..')

# --- format-spec '^' must KEEP the calculate_padding rule (extra always right)
show(format("ab", "^5"))   # ' ab  '   (NOT '  ab ' -- different from .center!)
show(format("abc", "^6"))  # ' abc  '
show(format("xy", "^7"))   # '  xy   '  (NOT '   xy  ')
show(format("abcd", "^7")) # ' abcd  '
show(f"{42:^7}")           # '  42   '
show(f"{'q':*^6}")         # '**q***'
