"""Purpose: a same-named subclass `class P(P)` keeps inherited base-method field reads correct (HF-203)."""


class P:
    def __init__(self):
        self.a = "base-a"
        self.b = "base-b"

    def read_a(self):
        return self.a

    def read_b(self):
        return self.b


base = P()


class P(P):  # noqa: F811
    def __init__(self):
        self.c = "sub-c"
        self.b = "sub-b"
        self.a = "sub-a"

    def read_c(self):
        return self.c

    def pair(self):
        return self.a, self.c


sub = P()
print("base reads", base.read_a(), base.read_b())
print("sub reads", sub.read_a(), sub.read_b(), sub.read_c(), sub.pair())
print("base vars", sorted(vars(base).items()))
print("sub vars", sorted(vars(sub).items()))
sub.a = "sub-a2"
base.a = "base-a2"
print("after writes", base.read_a(), sub.read_a(), sub.pair())
print("mro names", [cls.__qualname__ for cls in type(sub).__mro__])


def read_through_annotation(p: P):
    return p.a, p.b


print("annotated", read_through_annotation(sub), read_through_annotation(base))
