"""Purpose: differential coverage for operator full surface."""

import operator


class Box:
    def __init__(self, value):
        self.value = value

    def __matmul__(self, other):
        return f"{self.value}@{other.value}"

    def __imatmul__(self, other):
        self.value = f"{self.value}@{other.value}"
        return self


class LenHint:
    def __length_hint__(self):
        return 4


print("abs", operator.abs(-7))
print("add", operator.add(2, 3))
print("sub", operator.sub(9, 4))
print("mul", operator.mul(3, 5))

print("truediv", operator.truediv(7, 2))
print("floordiv", operator.floordiv(7, 2))
print("mod", operator.mod(7, 2))
print("pow", operator.pow(2, 5))

print("lshift", operator.lshift(2, 3))
print("rshift", operator.rshift(9, 2))
print("and", operator.and_(6, 3))
print("or", operator.or_(6, 3))
print("xor", operator.xor(6, 3))

print("neg", operator.neg(5))
print("pos", operator.pos(-5))
print("invert", operator.invert(2))
print("not", operator.not_(0))
print("truth", operator.truth([1]))

print("eq", operator.eq(3, 3), operator.eq(3, 4))
print("ne", operator.ne(3, 4), operator.ne(3, 3))
print("lt", operator.lt(1, 2))
print("le", operator.le(2, 2))
print("gt", operator.gt(3, 2))
print("ge", operator.ge(3, 3))
box_a = Box(1)
box_b = Box(1)
print("is", operator.is_(box_a, box_a), operator.is_(box_a, box_b))
print("is_not", operator.is_not(1, 1), operator.is_not(box_a, box_b))

print("contains", operator.contains([1, 2, 3], 2))
print("countOf", operator.countOf([1, 2, 1, 3], 1))
print("length_hint", operator.length_hint(LenHint()))

items = [1, 2, 3]
print("getitem", operator.getitem(items, 1))
operator.setitem(items, 1, 9)
print("setitem", items)
operator.delitem(items, 0)
print("delitem", items)

print("concat", operator.concat([1], [2]))

lst = [1]
print("iconcat", operator.iconcat(lst, [2]))

print("iadd", operator.iadd([1], [2]))
print("isub", operator.isub(5, 3))
print("imul", operator.imul(2, 4))

box = Box("a")
print("matmul", operator.matmul(Box("x"), Box("y")))
result = operator.imatmul(box, Box("b"))
print("imatmul", result is box, box.value)

print("itruediv", operator.itruediv(9, 3))
print("ifloordiv", operator.ifloordiv(9, 2))
print("imod", operator.imod(9, 4))
print("ipow", operator.ipow(2, 4))

print("ilshift", operator.ilshift(1, 2))
print("irshift", operator.irshift(8, 2))
print("iand", operator.iand(6, 3))
print("ior", operator.ior(6, 3))
print("ixor", operator.ixor(6, 3))

print("index", operator.index(7))
