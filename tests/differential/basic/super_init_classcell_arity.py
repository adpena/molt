"""Purpose: differential coverage for zero-arg super().__init__(args) through the
implicit __class__ closure cell — the constructor call site must thread the
closure env so the boxed lanes (LLVM/WASM) do not drop an argument (task #65).

re.error shape: an Exception subclass whose __init__ uses zero-arg super() with
multiple positional args. Native tolerated the dropped __molt_closure__ arg via
runtime-dispatch fallback; LLVM/WASM emitted an arity-mismatched direct call
("Incorrect number of arguments passed to called function!" /
"expected i64 but nothing on stack"). Covers exception + plain-object parents,
positional/default/kw-only signatures, classmethod super(), and nested-method
super().
"""


# 1) Exception subclass with multi-arg super().__init__ (the re.error shape):
#    zero-arg super() => __class__ cell => __init__ is a closure (extra param).
class MyError(Exception):
    def __init__(self, msg, pattern=None, pos=None):
        self.msg = msg
        self.pattern = pattern
        self.pos = pos
        super().__init__(msg)


def build_error(msg, pattern, pos):
    return MyError(msg, pattern, pos)


e = build_error("bad pattern", "a(b", 3)
print("err", e.msg, e.pattern, e.pos, e.args, str(e))

# Construct directly (not through a helper) too.
e2 = MyError("oops")
print("err2", e2.msg, e2.pattern, e2.pos, e2.args)


# 2) Plain-object parent, positional super().__init__ with several args.
class Vec:
    def __init__(self, x, y, z):
        self.x = x
        self.y = y
        self.z = z


class NamedVec(Vec):
    def __init__(self, name, x, y, z):
        self.name = name
        super().__init__(x, y, z)


nv = NamedVec("origin", 1, 2, 3)
print("vec", nv.name, nv.x, nv.y, nv.z)


# 3) Parent with defaults; child fills some, relies on default for the rest.
class Box:
    def __init__(self, w, h=10, d=1):
        self.w = w
        self.h = h
        self.d = d


class LabeledBox(Box):
    def __init__(self, label, w, h):
        self.label = label
        super().__init__(w, h)


lb = LabeledBox("crate", 4, 5)
print("box", lb.label, lb.w, lb.h, lb.d)


# 4) Keyword-only parent params passed positionally-then-kw through super().
class Config:
    def __init__(self, name, *, debug=False, level=0):
        self.name = name
        self.debug = debug
        self.level = level


class AppConfig(Config):
    def __init__(self, name, level):
        super().__init__(name, debug=True, level=level)


ac = AppConfig("svc", 7)
print("cfg", ac.name, ac.debug, ac.level)


# 5) super() inside a classmethod (alternate-constructor pattern).
class Registry:
    def __init__(self, items):
        self.items = items

    @classmethod
    def empty(cls):
        return cls([])


class TaggedRegistry(Registry):
    def __init__(self, items, tag="t"):
        super().__init__(items)
        self.tag = tag

    @classmethod
    def empty(cls):
        inst = super().empty()
        inst.tag = "from-empty"
        return inst


tr = TaggedRegistry.empty()
print("reg", tr.items, tr.tag)


# 6) super() in a nested-call chain (Leaf -> Mid -> Base), each adding state.
class Base6:
    def __init__(self, a):
        self.a = a


class Mid6(Base6):
    def __init__(self, a, b):
        super().__init__(a)
        self.b = b


class Leaf6(Mid6):
    def __init__(self, a, b, c):
        super().__init__(a, b)
        self.c = c


leaf = Leaf6(1, 2, 3)
print("chain", leaf.a, leaf.b, leaf.c)


# 7) super().method() returning a value (not __init__) through the cell.
class Greeter:
    def greet(self):
        return "hi"


class LoudGreeter(Greeter):
    def greet(self):
        return super().greet().upper() + "!"


print("greet", LoudGreeter().greet())
