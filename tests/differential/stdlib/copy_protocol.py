"""CPython 3.12 copy protocol: identity, state, hooks and shared memo."""
import copy
import copyreg
import sys


class Plain:
    def __init__(self, child):
        self.child = child

    def method(self):
        return self.child


class Slotted:
    __slots__ = ("child",)

    def __init__(self, child):
        self.child = child


for cls in (Plain, Slotted):
    obj = cls([1])
    shallow = copy.copy(obj)
    deep = copy.deepcopy(obj)
    print(cls.__name__, type(shallow) is cls, shallow is not obj,
          shallow.child is obj.child, deep is not obj,
          deep.child == obj.child, deep.child is not obj.child)
    rv = obj.__reduce_ex__(4)
    print("default reduction", rv[0] is copyreg.__newobj__, rv[1] == (cls,),
          len(rv), rv[3] is None, rv[4] is None)


class Hook:
    def __init__(self, child):
        self.child = child

    def __copy__(self):
        return Hook(self.child)

    def __deepcopy__(self, memo):
        print("memo dict", isinstance(memo, dict), memo["marker"])
        result = Hook(None)
        memo[id(self)] = result
        result.child = copy.deepcopy(self.child, memo)
        return result


shared = []
obj = Hook(shared)
memo = {"marker": "shared"}
deep = copy.deepcopy([obj, shared, obj], memo)
print("hooks", copy.copy(obj).child is shared,
      deep[0] is deep[2], deep[0].child is deep[1], deep[1] is not shared,
      memo[id(obj)] is deep[0], memo[id(memo)][-1] is not None)
seeded = object()
print("preseeded", copy.deepcopy(obj, {id(obj): seeded}) is seeded)
print("preseeded atomic", copy.deepcopy([17], {id(17): seeded})[0] is seeded)

nil = copy.deepcopy.__defaults__[1]
for atom in (17, None, False, 1.25, "sentinel-text"):
    print("memo sentinel miss", copy.deepcopy([atom], {id(atom): nil})[0] is atom)
child = Plain([1])
print("memo instance sentinel", copy.deepcopy([child], {id(child): nil})[0] is not child)
tuple_value = (17,)
print("tuple post-recursion sentinel", copy.deepcopy(tuple_value, {id(tuple_value): nil}) is nil)


class MemoKey:
    def __hash__(self):
        return hash(id(17))

    def __eq__(self, other):
        print("memo key equality", isinstance(other, int))
        return other == id(17)


print("equal memo key", copy.deepcopy([17], {MemoKey(): seeded})[0] is seeded)


class SeedMemo:
    def __deepcopy__(self, memo):
        memo[id(17)] = "hook seed"
        return self


print("hook seeds native child", copy.deepcopy([SeedMemo(), 17])[1])

trace = []


class CopyKey:
    def __deepcopy__(self, memo):
        trace.append("key")
        return self


class CopyValue:
    def __deepcopy__(self, memo):
        trace.append("value")
        return self


copy.deepcopy({CopyKey(): CopyValue()})
print("dict copy order", trace)


class Failing:
    def __deepcopy__(self, memo):
        raise ValueError("child copy failed")


original = [Failing()]
memo = {}
try:
    copy.deepcopy(original, memo)
except ValueError as exc:
    print("copy failure", str(exc))
print("failure memo", memo[id(original)] == [], id(memo) in memo)


class Mutating:
    def __deepcopy__(self, memo):
        changing["added"] = 0
        return self


changing = {"before": Mutating()}
try:
    copy.deepcopy(changing)
except RuntimeError as exc:
    print("dict mutation", str(exc))

obj = Plain(None)
obj.child = [obj]
deep = copy.deepcopy(obj)
print("instance cycle", deep is not obj, deep.child[0] is deep)
obj = Hook(None)
obj.child = obj
deep = copy.deepcopy(obj, {"marker": "cycle"})
print("hook cycle", deep.child is deep)

# The immutable tuple can only be memoized by the recursive child walk.
loop = []
tuple_cycle = (loop,)
loop.append(tuple_cycle)
deep = copy.deepcopy(tuple_cycle)
print("tuple cycle", deep is not tuple_cycle, deep[0] is not loop,
      deep[0][0] is deep)
mapping_cycle = {}
alias = [mapping_cycle, mapping_cycle]
mapping_cycle["alias"] = alias
deep = copy.deepcopy(alias)
print("mapping cycle", deep[0] is deep[1], deep[0]["alias"] is deep)


class Memo(dict):
    def __init__(self):
        self.calls = 0

    def get(self, key, default=None):
        self.calls += 1
        return dict.get(self, key, default)


memo = Memo()
out = copy.deepcopy([[17]], memo)
print("memo subclass", out, memo.calls)


class Appender:
    def __init__(self):
        self.values = []

    def append(self, value):
        self.values.append(value)


alive = Appender()
memo = {}
memo[id(memo)] = alive
original = [[17]]
out = copy.deepcopy(original, memo)
print("seeded keep alive", len(alive.values), alive.values[0] is original[0],
      alive.values[1] is original)


def generator():
    yield 1


for label, value in (("module", sys), ("memoryview", memoryview(b"x")),
                     ("generator", generator())):
    print("opaque getstate", label, value.__getstate__() is None)
    for copier in (copy.copy, copy.deepcopy):
        try:
            copier(value)
            print("opaque copied", label)
        except TypeError:
            print("opaque rejected", label)


class State:
    def __getstate__(self):
        return {"child": self.child, "custom": True}

    def __setstate__(self, state):
        self.child = state["child"]
        self.custom = state["custom"]


obj = State()
obj.child = [2]
for operation in (copy.copy, copy.deepcopy):
    out = operation(obj)
    print("state", out is not obj, out.child == [2],
          out.child is obj.child, out.custom)


class Items:
    def __init__(self):
        self.sequence = []
        self.mapping = {}

    def append(self, value):
        self.sequence.append(value)

    def __setitem__(self, key, value):
        self.mapping[key] = value

    def __reduce_ex__(self, protocol):
        print("protocol", protocol)
        return (Items, (), {"child": self.child},
                iter([self.child]), iter([("key", self.child)]))


obj = Items()
obj.child = [3]
for operation in (copy.copy, copy.deepcopy):
    out = operation(obj)
    print("reconstructed", out is not obj, out.child == [3],
          out.child is obj.child, out.sequence[0] is out.child,
          out.mapping["key"] is out.child)


class Registered:
    def __reduce_ex__(self, protocol):
        raise AssertionError("registry precedes reduction")


def registered_reducer(obj):
    return Plain, (obj.child,)


copyreg.dispatch_table[Registered] = registered_reducer
obj = Registered()
obj.child = [4]
for operation in (copy.copy, copy.deepcopy):
    out = operation(obj)
    print("registry", type(out) is Plain, out.child == [4], out.child is obj.child)
del copyreg.dispatch_table[Registered]


class ListSubclass(list):
    pass


class DictSubclass(dict):
    pass


for obj in (ListSubclass([[5]]), DictSubclass(value=[5])):
    obj.child = [6]
    for operation in (copy.copy, copy.deepcopy):
        out = operation(obj)
        print("subclass", type(out) is type(obj), out is not obj, out == obj,
              out.child == [6], out.child is obj.child)


class Uncopyable:
    def __getattribute__(self, name):
        if name in ("__reduce_ex__", "__reduce__"):
            raise AttributeError(name)
        return object.__getattribute__(self, name)


for operation in (copy.copy, copy.deepcopy):
    try:
        operation(Uncopyable())
    except copy.Error as exc:
        print("uncopyable", str(exc))


class StringReduction:
    def __reduce_ex__(self, protocol):
        return "global_name"


obj = StringReduction()
print("string reduction", copy.copy(obj) is obj, copy.deepcopy(obj) is obj)


def function():
    pass


for obj in (function, Plain, range(4)):
    print("atomic", copy.copy(obj) is obj, copy.deepcopy(obj) is obj)
obj = slice([1], 5, 2)
out = copy.deepcopy(obj)
print("slice", copy.copy(obj) is obj, out is not obj,
      out.start == obj.start, out.start is not obj.start)
obj = Plain([7])
method = copy.deepcopy(obj.method)
print("method", method.__func__ is obj.method.__func__,
      method.__self__ is not obj, method() == [7], method() is not obj.child)


class IntSubclass(int):
    def __int__(self):
        raise AssertionError("newargs must read integer storage")


class FloatSubclass(float):
    def __float__(self):
        raise AssertionError("newargs must read float storage")


class StrSubclass(str):
    def __str__(self):
        raise AssertionError("newargs must read string storage")


class BytesSubclass(bytes):
    def __bytes__(self):
        raise AssertionError("newargs must read bytes storage")


class ComplexSubclass(complex):
    pass


class TupleSubclass(tuple):
    pass


class SetSubclass(set):
    pass


class FrozenSubclass(frozenset):
    pass


class BytearraySubclass(bytearray):
    def __bytes__(self):
        raise AssertionError("bytearray reduction reads physical storage")


class SlottedStr(str):
    __slots__ = ("child",)


class SlottedSet(set):
    __slots__ = ("child",)


for obj in (IntSubclass(7), FloatSubclass(1.25), StrSubclass("héllo"),
            BytesSubclass(b"data"), ComplexSubclass(1, 2), TupleSubclass(([8],)),
            SetSubclass([1, 2]), FrozenSubclass([1, 2]), BytearraySubclass(b"\xffx"),
            SlottedStr("héllo"), SlottedSet([4])):
    obj.child = [9]
    for operation in (copy.copy, copy.deepcopy):
        out = operation(obj)
        print("native subtype", type(obj).__name__, type(out) is type(obj),
              out is not obj, out == obj, out.child == [9], out.child is obj.child)

for obj in (set([1, 2]), frozenset([1, 2]), bytearray(b"\xffx")):
    out = copy.deepcopy(obj)
    print("reduced builtin", type(obj).__name__, out is not obj, out == obj)

# A user reducer still precedes the builtin reduction for types absent from
# _deepcopy_dispatch, including sets and bytearrays.
def reduce_set(obj):
    return list, (["registered set"],)


copyreg.dispatch_table[set] = reduce_set
print("registered exact set", copy.deepcopy({1}))
del copyreg.dispatch_table[set]
