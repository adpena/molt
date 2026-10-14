"""Purpose: a function-local class with a module class's name leaves the module class intact (HF-203)."""

class P:
    def __init__(self):
        self.a = 1
        self.b = 2


p1 = P()


def local_shadow():
    class P:
        def __init__(self):
            self.q = "q"
            self.a = "local-a"

    lp = P()
    return lp.a, lp.q


print("local_shadow", local_shadow())
print("module p1", p1.a, p1.b)


def make_and_read():
    class P:
        def __init__(self):
            self.z = 0
            self.b = "inner-b"
            self.a = "inner-a"

    x = P()
    return x.a, x.b, p1.a, p1.b


print("make_and_read", make_and_read())
