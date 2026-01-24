"""Purpose: differential coverage for bytes constructors."""

print(bytes("abc", "utf-8"))
print(bytes("abc", encoding="utf-8"))
print(bytearray("abc", "utf-8"))
print(bytearray("abc", encoding="utf-8"))

try:
    bytes("abc")
except TypeError as exc:
    print(f"bytes-missing-encoding:{exc}")

try:
    bytearray("abc")
except TypeError as exc:
    print(f"bytearray-missing-encoding:{exc}")

try:
    bytes(encoding="utf-8")
except TypeError as exc:
    print(f"bytes-encoding-nosrc:{exc}")

try:
    bytes(errors="strict")
except TypeError as exc:
    print(f"bytes-errors-nosrc:{exc}")

print(bytes(3))
print(bytearray(3))
print(bytes([1, 2, 3]))
print(bytearray([1, 2, 3]))

try:
    bytes([256])
except ValueError as exc:
    print(f"bytes-range:{exc}")

try:
    bytearray([256])
except ValueError as exc:
    print(f"bytearray-range:{exc}")

try:
    bytes([1, "a"])
except TypeError as exc:
    print(f"bytes-type:{exc}")

try:
    bytearray([1, "a"])
except TypeError as exc:
    print(f"bytearray-type:{exc}")

try:
    bytes(1.2)
except TypeError as exc:
    print(f"bytes-float:{exc}")

try:
    bytearray(1.2)
except TypeError as exc:
    print(f"bytearray-float:{exc}")
