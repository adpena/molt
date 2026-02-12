"""Purpose: differential coverage for copy basics."""

import copy


class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y


p = Point(1, 2)
q = copy.copy(p)
r = copy.deepcopy(p)

print(p is q, p is r)
print((p.x, p.y), (q.x, q.y), (r.x, r.y))
