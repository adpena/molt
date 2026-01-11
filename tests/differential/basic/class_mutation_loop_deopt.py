class Descriptor:
    def __set__(self, inst, val):
        inst.log = val


class Box:
    x: int


b = Box()
b.x = 0
for i in range(3):
    if i == 1:
        Box.x = Descriptor()
    b.x = i

try:
    print(b.log)
except AttributeError:
    print("AttributeError")
