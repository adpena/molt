"""Purpose: differential coverage for builtin formatting."""


class IndexObj:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


class BadIndex:
    def __index__(self):
        return "nope"


class NoIndex:
    pass


class ReprObj:
    def __repr__(self):
        return "snowman:\u2603"


print(ascii("hi"))
print(ascii("caf\u00e9"))
print(ascii("\u2603"))
print(ascii("\U0001f600"))
print(ascii(b"\xff"))
print(ascii(bytearray(b"\xff")))
print(ascii(ReprObj()))
print(bin(10))
print(bin(-10))
print(bin(True))
print(bin(IndexObj(5)))
print(bin(IndexObj(10**30)))
print(oct(9))
print(oct(-9))
print(oct(IndexObj(9)))
print(hex(255))
print(hex(-255))
print(hex(10**20))
print(hex(IndexObj(255)))

try:
    bin(NoIndex())
except TypeError as exc:
    print(f"bin-noindex:{exc}")

try:
    hex(BadIndex())
except TypeError as exc:
    print(f"hex-badindex:{exc}")
