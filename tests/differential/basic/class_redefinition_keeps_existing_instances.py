"""Purpose: an instance keeps its own class layout after the class name is redefined (HF-203)."""

class P:
    def __init__(self):
        self.a = 1


p1 = P()


class P:  # noqa: F811
    def __init__(self):
        self.z = 0
        self.a = 2


print(p1.a, P().a)
