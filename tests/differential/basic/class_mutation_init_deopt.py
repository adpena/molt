"""Purpose: differential coverage for class mutation init deopt."""


class Descriptor:
    def __set__(self, inst, val):
        inst.log = val


class Box:
    x: int

    def __init__(self, val: int) -> None:
        self.x = val


Box.x = Descriptor()
b = Box(7)
print(b.log)
