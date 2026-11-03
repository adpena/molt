"""Purpose: differential coverage for copyreg helper functions."""

import copy
import copyreg
import pickle
import types


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

# Reduction tuples carry these public Python functions by reference. A raw
# intrinsic can be callable yet lack the identity needed by save_global.
for name in ("__newobj__", "__newobj_ex__", "_reconstructor", "_reduce_ex"):
    helper = getattr(copyreg, name)
    print(
        "helper identity",
        name,
        type(helper) is types.FunctionType,
        helper.__module__,
        helper.__name__,
        helper.__qualname__,
    )
    for protocol in (2, 3, 4, 5):
        print(
            "helper global",
            name,
            protocol,
            pickle.loads(pickle.dumps(helper, protocol=protocol)) is helper,
        )


class Node:
    def __init__(self):
        self.child = [7]
        self.next = self


class KeywordNode(Node):
    def __new__(cls, *, value):
        obj = super().__new__(cls)
        obj.value = value
        return obj

    def __init__(self, *, value):
        super().__init__()

    def __getnewargs_ex__(self):
        return (), {"value": self.value}


for obj, constructor in (
    (Node(), copyreg.__newobj__),
    (KeywordNode(value=9), copyreg.__newobj_ex__),
):
    for protocol in (2, 3, 4, 5):
        reduced = obj.__reduce_ex__(protocol)
        out = pickle.loads(pickle.dumps(obj, protocol=protocol))
        print(
            "instance global",
            type(obj).__name__,
            protocol,
            reduced[0] is constructor,
            type(out) is type(obj),
            out is not obj,
            out.next is out,
            out.child == obj.child,
            out.child is not obj.child,
            getattr(out, "value", None),
        )
    shallow = copy.copy(obj)
    deep = copy.deepcopy(obj)
    print(
        "instance copy",
        type(obj).__name__,
        type(shallow) is type(obj),
        shallow is not obj,
        shallow.next is obj,
        shallow.child is obj.child,
        type(deep) is type(obj),
        deep is not obj,
        deep.next is deep,
        deep.child == obj.child,
        deep.child is not obj.child,
        getattr(shallow, "value", None),
        getattr(deep, "value", None),
    )
