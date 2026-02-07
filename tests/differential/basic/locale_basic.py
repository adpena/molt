"""Purpose: differential coverage for locale basics."""

import locale


print(locale.setlocale(locale.LC_ALL, "C"))
print(locale.getpreferredencoding(False))
print(locale.getlocale())
print(locale.setlocale(locale.LC_ALL, None))

try:
    locale.setlocale(locale.LC_ALL, 1)
except Exception as exc:
    print("setlocale_bad", type(exc).__name__)
