"""Purpose: a method keeps its own class's field layout after the module rebinds the class name to another class of the same shape (HF-220)."""


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

    def get_a(self):
        return self.a

    def set_b(self, value):
        self.b = value


Old = P
P = Q
q = Q()
print(Old.get_a(q))
Old.set_b(q, 99)
print(q.a, q.b)
print(Old.get_a(Old()))
