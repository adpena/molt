"""Purpose: a function annotated with a class reads and writes its argument's fields correctly after the class name is rebound to a same-shaped class (HF-220)."""


class P:
    def __init__(self):
        self.a = 1
        self.b = 2


class Q:
    def __init__(self):
        self.b = 10
        self.a = 20


def read_a(p: P):
    return p.a


def write_b(p: P, value):
    p.b = value


print(read_a(P()))
P = Q
q = Q()
print(read_a(q))
write_b(q, 99)
print(q.a, q.b)
print(read_a(P()))
