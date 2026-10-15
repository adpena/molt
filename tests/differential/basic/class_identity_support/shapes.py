"""Two same-shaped classes and a reader annotated with the first (HF-220)."""


class P:
    def __init__(self):
        self.a = 1
        self.b = 2

    def get_a(self):
        return self.a

    def set_b(self, value):
        self.b = value


class Q:
    # The same methods as P give Q P's layout version, so only the class
    # statement identity tells the two apart.
    def __init__(self):
        self.b = 10
        self.a = 20

    def get_a(self):
        return self.a

    def set_b(self, value):
        self.b = value


def read_a(p: P):
    return p.a
