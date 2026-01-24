"""Purpose: differential coverage for class mutation deopt."""


class Descriptor:
    def __set__(self, inst, val):
        inst.log = val


class Box:
    x: int


b = Box()
b.x = 1
Box.x = Descriptor()
b.x = 2

try:
    print(b.log)
except AttributeError:
    print("AttributeError")
