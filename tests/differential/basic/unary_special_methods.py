"""Purpose: unary +/- dispatch through __pos__/__neg__ special methods."""

from collections import Counter


class UnaryBox:
    def __pos__(self):
        print("box pos called")
        return "pos-result"

    def __neg__(self):
        print("box neg called")
        return "neg-result"


box = UnaryBox()
print(+box)
print(-box)
print(+True, type(+True).__name__)
print(-True, type(-True).__name__)

counter = Counter({"a": 2, "b": -1, "c": 0})
print(dict(+counter))
print(dict(-counter))
