import attr

@attr.s(auto_attribs=True)
class Point:
    x: int = 0
    y: int = 0

p = Point(x=3, y=4)
print("attrs", attr.__version__)
print("point", p.x, p.y)
