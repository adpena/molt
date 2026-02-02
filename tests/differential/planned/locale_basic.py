"""Purpose: differential coverage for locale basics."""

import locale

locale.setlocale(locale.LC_ALL, "C")
print(locale.getpreferredencoding(False))
print(locale.getlocale())
