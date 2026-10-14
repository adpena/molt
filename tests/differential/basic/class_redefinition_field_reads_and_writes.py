"""Purpose: same-named classes, plain and __slots__, read and write their own fields (HF-203)."""

class P:
    def __init__(self):
        self.a = 1
        self.b = 2


p1 = P()


class P:  # noqa: F811
    def __init__(self):
        self.z = 100
        self.y = 200
        self.b = 20
        self.a = 10


p2 = P()
print("p1", p1.a, p1.b)
print("p2", p2.a, p2.b, p2.y, p2.z)
p1.a = 7
p1.b = 8
print("p1 after write", p1.a, p1.b, vars(p1))
print("p2 after p1 write", p2.a, p2.b, p2.y, p2.z)


def first_a(p: P) -> int:
    return p.a


print("first_a(p1)", first_a(p1), "first_a(p2)", first_a(p2))


class S:
    __slots__ = ("u", "v")

    def __init__(self):
        self.u = "u0"
        self.v = "v0"


s_outer = S()


def make_inner():
    class S:
        __slots__ = ("w", "v", "u", "t")

        def __init__(self):
            self.t = "t1"
            self.u = "u1"
            self.v = "v1"
            self.w = "w1"

    return S()


s_inner = make_inner()
print("outer", s_outer.u, s_outer.v)
print("inner", s_inner.u, s_inner.v, s_inner.w, s_inner.t)
s_outer.v = "V0"
print("outer after", s_outer.u, s_outer.v)


def read_uv(s: S):
    return s.u, s.v


print("read_uv", read_uv(s_outer), read_uv(s_inner))


def local_shadow():
    class P:
        def __init__(self):
            self.q = "q"
            self.a = "local-a"

    lp = P()
    return lp.a, lp.q, p1.a, p2.a


print("local_shadow", local_shadow())
