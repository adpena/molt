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
    def __init__(self):
        self.b = 10
        self.a = 20


def read_a(p: P):
    return p.a
