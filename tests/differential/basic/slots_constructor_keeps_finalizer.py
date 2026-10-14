"""Purpose: a __slots__ object built and dropped at once still runs __del__.

Guards dead struct-allocation elision against finalizers (HF-214).
"""

class P:
    __slots__ = ("x", "y")

    def __init__(self, x, y):
        self.x = x
        self.y = y

    def __del__(self):
        print("del", self.x, self.y)


def make():
    P(1, 2)
    print("after")


make()
