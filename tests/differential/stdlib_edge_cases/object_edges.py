def section(name):
    print(f"--- {name} ---")


section("Diamond Inheritance MRO")


class A:
    def method(self):
        print("A")


class B(A):
    def method(self):
        print("B")
        super().method()


class C(A):
    def method(self):
        print("C")
        super().method()


class D(B, C):
    def method(self):
        print("D")
        super().method()


D().method()
print([c.__name__ for c in D.mro()])

section("Descriptor Precedence")


class DataDesc:
    def __get__(self, obj, type=None):
        return "data_get"

    def __set__(self, obj, value):
        pass


class NonDataDesc:
    def __get__(self, obj, type=None):
        return "nondata_get"


class E:
    data = DataDesc()
    nondata = NonDataDesc()


e = E()
e.__dict__["data"] = "instance_val"
e.__dict__["nondata"] = "instance_val"

# Data descriptor wins over instance dict
print(e.data)
# Instance dict wins over non-data descriptor
print(e.nondata)

section("Slots Inheritance")


class F:
    __slots__ = ("a",)

    def __init__(self):
        self.a = 1


class G(F):
    __slots__ = ("b",)

    def __init__(self):
        super().__init__()
        self.b = 2


g = G()
print(g.a, g.b)
try:
    g.c = 3
except AttributeError:
    print("AttributeError caught (slots)")

section("Getattr vs Getattribute")


class H:
    def __getattribute__(self, name):
        print(f"getattribute {name}")
        return super().__getattribute__(name)

    def __getattr__(self, name):
        print(f"getattr {name}")
        return "fallback"


h = H()
h.x = 1
print(f"Result: {h.x}")  # Should call getattribute
print(f"Result: {h.y}")  # Should call getattribute then getattr
