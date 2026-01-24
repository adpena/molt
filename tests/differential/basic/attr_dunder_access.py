"""Purpose: differential coverage for attr dunder access."""

calls = []


class F:
    def __getattribute__(self, name):
        if name == "ok":
            return "ok"
        if name == "missing":
            raise AttributeError(name)
        if name == "boom":
            raise ValueError("boom")
        return "miss"

    def __getattr__(self, name):
        return f"fallback:{name}"


f = F()
print(f.ok)
print(f.missing)
print(f.other)
try:
    _ = f.boom
except ValueError as exc:
    print(type(exc).__name__)


class H:
    def __getattribute__(self, name):
        if name == "bad":
            raise RuntimeError("bad")
        raise AttributeError(name)


h = H()
print(hasattr(h, "ok"))
try:
    hasattr(h, "bad")
except RuntimeError:
    print("hasattr-err")


class D:
    def __init__(self):
        self.x = 1

    def __delattr__(self, name):
        calls.append(name)
        if name in self.__dict__:
            self.__dict__.pop(name)


d = D()
delattr(d, "x")
print(calls)
print(hasattr(d, "x"))


class C:
    pass


C.k = 10
c = C()
c.y = 2
print(c.__class__ is C)
print(C.__class__ is type)
print("k" in C.__dict__)
print(C.__dict__["k"])
print(c.__dict__["y"])


def foo():
    return 1


print(foo.__annotations__)
foo.note = "ok"
print(foo.__dict__["note"])
