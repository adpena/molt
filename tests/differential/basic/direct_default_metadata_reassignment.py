"""Purpose: direct-call lowering must observe reassigned default metadata."""


def f(x=1, *, y=2):
    return x * 100 + y


def call_f():
    return f()


def call_local_alias():
    h = f
    return h()


class Box:
    def __init__(self, seed=4):
        self.seed = seed

    def value(self, bump=5):
        return self.seed * 10 + bump


def call_box():
    return Box().seed


def call_method(box):
    return box.value()


print("func_before", call_f())
print("alias_before", call_local_alias())
f.__defaults__ = (10,)
f.__kwdefaults__ = {"y": 20}
print("func_after", call_f())
print("alias_after", call_local_alias())

print("init_before", call_box())
Box.__init__.__defaults__ = (8,)
print("init_after", call_box())

box = Box(2)
print("method_before", call_method(box))
Box.value.__defaults__ = (7,)
print("method_after", call_method(box))
