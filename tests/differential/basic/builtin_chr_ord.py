"""Purpose: differential coverage for builtin chr ord."""


class IndexObj:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


class BadIndex:
    def __index__(self):
        return "nope"


print(chr(65))
print(ascii(chr(0)))
print(ascii(chr(0x10FFFF)))
print(chr(IndexObj(66)))
print(ord("A"))
print(ord("é"))
print(ord(b"A"))
print(ord(bytearray(b"\xff")))

try:
    chr(-1)
except ValueError as exc:
    print(f"chr-neg:{exc}")

try:
    chr(0x110000)
except ValueError as exc:
    print(f"chr-range:{exc}")

try:
    chr(IndexObj(0x110000))
except ValueError as exc:
    print(f"chr-index-range:{exc}")

try:
    chr("x")
except TypeError as exc:
    print(f"chr-type:{exc}")

try:
    chr(BadIndex())
except TypeError as exc:
    print(f"chr-badindex:{exc}")

try:
    ord("")
except TypeError as exc:
    print(f"ord-empty:{exc}")

try:
    ord("ab")
except TypeError as exc:
    print(f"ord-two:{exc}")

try:
    ord(b"")
except TypeError as exc:
    print(f"ord-bytes-empty:{exc}")

try:
    ord(b"ab")
except TypeError as exc:
    print(f"ord-bytes-two:{exc}")

try:
    ord(1)
except TypeError as exc:
    print(f"ord-type:{exc}")

try:
    ord(object())
except TypeError as exc:
    print(f"ord-object:{exc}")
