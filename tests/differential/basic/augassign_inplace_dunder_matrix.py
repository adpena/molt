"""Purpose: differential coverage for the in-place augmented-assignment dunder
protocol across all seven operators that previously lowered to plain binary ops
(//=, /=, %=, **=, <<=, >>=, @=).

For each operator CPython requires:
  1. only __iX__ defined            -> __iX__ is called (NOT the binary fallback).
  2. both __iX__ and __X__ defined  -> __iX__ wins.
  3. __iX__ returns NotImplemented  -> fall back to the binary __X__/__rX__ chain.
  4. neither defined                -> TypeError (exercised separately).

The bug (doc 30 1e): the frontend lowered //= etc. to the binary kind, so a class
defining only __ifloordiv__ was silently routed to __floordiv__ (or TypeError).
"""


def banner(title):
    print("===", title, "===")


# ---- Case 1: only the in-place dunder is defined -> it must be called. ----
banner("only-inplace")


class IFloor:
    def __init__(self, v):
        self.v = v

    def __ifloordiv__(self, other):
        print("ifloordiv called", self.v, other)
        return ("ip-floordiv", self.v, other)


class ITrue:
    def __init__(self, v):
        self.v = v

    def __itruediv__(self, other):
        print("itruediv called", self.v, other)
        return ("ip-truediv", self.v, other)


class IMod:
    def __init__(self, v):
        self.v = v

    def __imod__(self, other):
        print("imod called", self.v, other)
        return ("ip-mod", self.v, other)


class IPow:
    def __init__(self, v):
        self.v = v

    def __ipow__(self, other):
        print("ipow called", self.v, other)
        return ("ip-pow", self.v, other)


class ILshift:
    def __init__(self, v):
        self.v = v

    def __ilshift__(self, other):
        print("ilshift called", self.v, other)
        return ("ip-lshift", self.v, other)


class IRshift:
    def __init__(self, v):
        self.v = v

    def __irshift__(self, other):
        print("irshift called", self.v, other)
        return ("ip-rshift", self.v, other)


class IMatmul:
    def __init__(self, v):
        self.v = v

    def __imatmul__(self, other):
        print("imatmul called", self.v, other)
        return ("ip-matmul", self.v, other)


a = IFloor(17)
a //= 3
print("floordiv result", a)

b = ITrue(17)
b /= 3
print("truediv result", b)

c = IMod(17)
c %= 3
print("mod result", c)

d = IPow(2)
d **= 5
print("pow result", d)

e = ILshift(1)
e <<= 4
print("lshift result", e)

f = IRshift(64)
f >>= 2
print("rshift result", f)

g = IMatmul(9)
g @= 4
print("matmul result", g)


# ---- Case 2: both defined -> the in-place dunder wins. ----
banner("both-defined")


class BothFloor:
    def __init__(self, v):
        self.v = v

    def __ifloordiv__(self, other):
        print("ifloordiv (both) wins")
        return ("ip", self.v, other)

    def __floordiv__(self, other):
        print("floordiv (both) SHOULD NOT be called")
        return ("bin", self.v, other)


class BothPow:
    def __init__(self, v):
        self.v = v

    def __ipow__(self, other):
        print("ipow (both) wins")
        return ("ip", self.v, other)

    def __pow__(self, other):
        print("pow (both) SHOULD NOT be called")
        return ("bin", self.v, other)


class BothMatmul:
    def __init__(self, v):
        self.v = v

    def __imatmul__(self, other):
        print("imatmul (both) wins")
        return ("ip", self.v, other)

    def __matmul__(self, other):
        print("matmul (both) SHOULD NOT be called")
        return ("bin", self.v, other)


h = BothFloor(10)
h //= 2
print("both floordiv result", h)

i = BothPow(2)
i **= 3
print("both pow result", i)

j = BothMatmul(5)
j @= 6
print("both matmul result", j)


# ---- Case 3: __iX__ returns NotImplemented -> fall back to binary. ----
banner("notimplemented-fallback")


class FallFloor:
    def __init__(self, v):
        self.v = v

    def __ifloordiv__(self, other):
        print("ifloordiv returns NotImplemented")
        return NotImplemented

    def __floordiv__(self, other):
        print("floordiv fallback used")
        return ("bin-floordiv", self.v, other)


class FallShift:
    def __init__(self, v):
        self.v = v

    def __ilshift__(self, other):
        print("ilshift returns NotImplemented")
        return NotImplemented

    def __lshift__(self, other):
        print("lshift fallback used")
        return ("bin-lshift", self.v, other)


class FallMod:
    def __init__(self, v):
        self.v = v

    def __imod__(self, other):
        print("imod returns NotImplemented")
        return NotImplemented

    def __mod__(self, other):
        print("mod fallback used")
        return ("bin-mod", self.v, other)


k = FallFloor(20)
k //= 4
print("fallback floordiv result", k)

m = FallShift(1)
m <<= 3
print("fallback lshift result", m)

n = FallMod(20)
n %= 7
print("fallback mod result", n)


# ---- Case 3b: reflected fallback when only the RHS defines the binary op. ----
banner("reflected-fallback")


class LeftOnlyInplaceNI:
    def __init__(self, v):
        self.v = v

    def __imul__(self, other):
        print("imul NI (left)")
        return NotImplemented


class RightReflected:
    def __init__(self, v):
        self.v = v

    def __rmul__(self, other):
        print("rmul (right) used")
        return ("rmul", other.v, self.v)


# x *= y where x.__imul__ -> NotImplemented, x.__mul__ missing,
# y.__rmul__ defined: CPython uses y.__rmul__.
x = LeftOnlyInplaceNI(3)
y = RightReflected(4)
x *= y
print("reflected imul result", x)

# Reflected numeric opportunity precedes physical list/bytearray in-place slots.
import operator

sequence_events = []


class ReflectedSequenceOperand:
    def __radd__(self, other):
        sequence_events.append("radd")
        return 71

    def __rmul__(self, other):
        sequence_events.append("rmul")
        return 73

    def __iter__(self):
        sequence_events.append("iter")
        return iter([9])

    def __index__(self):
        sequence_events.append("index")
        return 2


for make in (list, bytearray):
    for operation in (operator.add, operator.iadd, operator.mul, operator.imul):
        original = make([1, 2])
        alias = original
        sequence_events.clear()
        result = operation(original, ReflectedSequenceOperand())
        print("sequence-numeric-first", make.__name__, operation.__name__, result,
              list(alias), sequence_events)

original = [1, 2]
sequence_events.clear()
result = operator.iconcat(original, ReflectedSequenceOperand())
print("explicit-iconcat", result is original, original, sequence_events)


class DecliningList(list):
    def __iadd__(self, other):
        sequence_events.append("iadd-NI")
        return NotImplemented

    def __add__(self, other):
        sequence_events.append("add-NI")
        return NotImplemented


left = DecliningList([1])
sequence_events.clear()
result = operator.iadd(left, ReflectedSequenceOperand())
print("mutated-list-slots", result, list(left), sequence_events)
del DecliningList.__iadd__
del DecliningList.__add__
sequence_events.clear()
result = operator.iconcat(left, ReflectedSequenceOperand())
print("restored-list-slots", result is left, list(left), sequence_events)

container_events = []


class ContainerReflected:
    def __ror__(self, other):
        container_events.append("ror")
        return 71

    def __rand__(self, other):
        container_events.append("rand")
        return 72

    def __rsub__(self, other):
        container_events.append("rsub")
        return 73

    def __rxor__(self, other):
        container_events.append("rxor")
        return 74


for operation, expected, event in ((operator.ior, 71, "ror"),
                                    (operator.iand, 72, "rand"),
                                    (operator.isub, 73, "rsub"),
                                    (operator.ixor, 74, "rxor")):
    left = {1, 2}
    container_events.clear()
    result = operation(left, ContainerReflected())
    assert result == expected and container_events == [event] and left == {1, 2}
    print("set-declines-reflected", operation.__name__, result, container_events)


class InheritedSet(set):
    pass


left = InheritedSet((1, 2))
alias = left
result = operator.iand(left, {2: 0}.keys())
assert result == {2} and alias == {1, 2} and result is not alias
print("inherited-set-view", sorted(result), sorted(alias))


class MappingPairs:
    def keys(self):
        container_events.append("keys")
        return ["x"]

    def __getitem__(self, key):
        container_events.append(("get", key))
        return 8


container_events.clear()
left = {}
result = dict.__ior__(left, MappingPairs())
assert result is left and left == {"x": 8}
assert container_events == ["keys", ("get", "x")]
print("dict-declared-ior", result is left, left, container_events)
