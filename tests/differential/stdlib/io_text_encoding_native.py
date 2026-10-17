"""Purpose: io.text_encoding follows CPython's C _io rule.

The default selector, the explicit-encoding pass-through, Argument Clinic's
C int stack level and the public signature all match CPython's _io module.
"""

import io
import sys

explicit = object()
print("explicit", io.text_encoding(explicit) is explicit)
print("named", io.text_encoding("ascii", 5))
print("default", io.text_encoding(None) == ("utf-8" if sys.flags.utf8_mode else "locale"))
print("signature", io.text_encoding.__name__, io.text_encoding.__text_signature__)

for level in (1.0, "2", 2**31, -(2**31) - 1):
    try:
        io.text_encoding("ascii", level)
    except (TypeError, OverflowError) as exc:
        print("level", type(exc).__name__, exc)


class Level:
    def __index__(self):
        return 3


print("index", io.text_encoding("ascii", Level()))
try:
    io.text_encoding(encoding=None)
except TypeError:
    print("positional-only")
