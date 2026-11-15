"""A second `Point` and `make` with different fields and behavior."""


class Point:
    def __init__(self, r):
        self.r = r

    def norm(self):
        return self.r * 10


def make(r):
    return Point(r)


def norm(point):
    return -point.norm()
