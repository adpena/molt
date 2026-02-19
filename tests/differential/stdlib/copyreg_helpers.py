"""Purpose: differential coverage for copyreg helper functions."""

import copyreg


class NewObj:
    def __new__(cls, *args):
        inst = super().__new__(cls)
        inst.args = args
        return inst


obj = copyreg.__newobj__(NewObj, 1, "a")
print("newobj_args", obj.args)


class NewObjEx:
    def __new__(cls, *args, **kwargs):
        inst = super().__new__(cls)
        inst.args = args
        inst.kw = dict(kwargs)
        return inst


obj = copyreg.__newobj_ex__(NewObjEx, (1, 2), {"x": 3})
print("newobj_ex", obj.args, obj.kw)


class Base:
    def __init__(self, state):
        self.state = state


class Child(Base):
    pass


obj = copyreg._reconstructor(Child, Base, {"k": 9})
print("reconstructor", type(obj).__name__, obj.state)


class Slotted:
    __slots__ = ("x",)

    def __init__(self):
        self.x = 1


def show(label, func):
    try:
        res = func()
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))
    else:
        print(label, res)


show("reduce_slots", lambda: copyreg._reduce_ex(Slotted(), 0))


class WithState:
    def __init__(self):
        self.x = 2

    def __getstate__(self):
        return {"x": self.x}


res = copyreg._reduce_ex(WithState(), 0)
print("reduce_len", len(res))
print("reduce_func", res[0].__name__)
print("reduce_args", res[1][0].__name__, res[1][1].__name__, res[1][2] is None)
print("reduce_state", res[2])
