"""One `Point` and one `make`; the sibling module defines the same names."""


class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

    def norm(self):
        return abs(self.x) + abs(self.y)


def make(x, y):
    return Point(x, y)


def norm(point):
    return point.norm()
