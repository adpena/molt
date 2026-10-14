"""Purpose: an old class's method called unbound on an unrelated same-named class reads the new layout (HF-203)."""


class P:
    def __init__(self):
        self.a = "old-a"
        self.b = "old-b"

    def get(self):
        return self.a

    def get_b(self):
        return self.b


Old = P


class P:  # noqa: F811
    def __init__(self):
        self.b = "new-b"
        self.a = "new-a"

    def get(self):
        return "new-get"

    def get_b(self):
        return "new-get-b"


print(Old.get(P()))
print(Old.get_b(P()))
print(Old().get(), Old().get_b(), P().get(), P().get_b())


def set_a(obj, value):
    Old.__init__(obj)
    return obj.a, obj.b


fresh = P()
print("old init on new", set_a(fresh, None), sorted(vars(fresh).items()))


class Q:
    def __init__(self):
        self.b = "q-b"
        self.a = "q-a"

    def get(self):
        return "q-get"

    def get_b(self):
        return "q-get-b"


P = Q
print("rebound", Old.get(Q()), Old.get_b(Q()))
