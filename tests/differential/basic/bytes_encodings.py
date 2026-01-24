"""Purpose: differential coverage for bytes encodings."""

print(bytes("abc", "utf-8"))
print(bytes("abc", "utf16"))
print(bytes("abc", "utf-16le"))
print(bytes("abc", "utf-16-be"))
print(bytes("abc", "utf-32"))
print(bytes("abc", "utf-32le"))
print(bytes("abc", "utf-32-be"))
print(bytearray("abc", "utf-16le"))

print(bytes("\u00e9", "latin-1"))
print(bytes("\u00e9", "ascii", "ignore"))
print(bytes("\u00e9", "ascii", "replace"))
print(bytes("abc", "ascii", "bad"))

try:
    bytes("\u00e9", "ascii")
except UnicodeEncodeError as exc:
    print(f"ascii-strict:{exc}")

try:
    bytes("\u00e9", "ascii", "bad")
except LookupError as exc:
    print(f"ascii-bad:{exc}")

try:
    bytes("abc", "bad-enc")
except LookupError as exc:
    print(f"enc-bad:{exc}")

try:
    bytes("abc", "ascii", 1)
except TypeError as exc:
    print(f"errors-type:{exc}")
