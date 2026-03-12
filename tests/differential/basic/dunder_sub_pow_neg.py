"""Purpose: differential coverage for __sub__, __pow__, __neg__ dunder methods."""


class Vector:
    def __init__(self, x, y):
        self.x = x
        self.y = y

    def __repr__(self):
        return f"Vector({self.x}, {self.y})"

    def __sub__(self, other):
        if isinstance(other, Vector):
            return Vector(self.x - other.x, self.y - other.y)
        if isinstance(other, (int, float)):
            return Vector(self.x - other, self.y - other)
        return NotImplemented

    def __rsub__(self, other):
        if isinstance(other, (int, float)):
            return Vector(other - self.x, other - self.y)
        return NotImplemented

    def __pow__(self, exp):
        if isinstance(exp, int):
            return Vector(self.x ** exp, self.y ** exp)
        return NotImplemented

    def __rpow__(self, base):
        if isinstance(base, (int, float)):
            return Vector(base ** self.x, base ** self.y)
        return NotImplemented

    def __neg__(self):
        return Vector(-self.x, -self.y)


if __name__ == "__main__":
    a = Vector(5, 10)
    b = Vector(2, 3)

    # __sub__ basic
    print("sub", a - b)
    print("sub scalar", a - 1)

    # __rsub__
    print("rsub", 10 - a)

    # __pow__ basic
    print("pow", b ** 2)
    print("pow3", b ** 3)

    # __rpow__
    print("rpow", 2 ** b)

    # __neg__
    print("neg", -a)
    print("neg neg", -(-a))

    # __neg__ on zero
    z = Vector(0, 0)
    print("neg zero", -z)

    # __sub__ NotImplemented fallback
    try:
        result = a - "bad"
        print("sub string should not reach")
    except TypeError as e:
        print("sub string error", type(e).__name__)

    # __pow__ NotImplemented fallback
    try:
        result = a ** 1.5
        print("pow float should not reach")
    except TypeError as e:
        print("pow float error", type(e).__name__)

    # chained operations
    c = Vector(1, 1)
    print("chain", -(a - b) ** 2)

    # __sub__ with floats
    f = Vector(1.5, 2.5)
    print("sub float", f - 0.5)

    # __neg__ preserves type
    neg_a = -a
    print("neg type", type(neg_a).__name__)

    # int __sub__ (built-in)
    print("int sub", 10 - 3)
    print("int neg", -(7))

    # float __sub__
    print("float sub", round(3.5 - 1.2, 10))
    print("float neg", -3.14)

    # int __pow__
    print("int pow", 2 ** 10)
    print("int pow zero", 5 ** 0)
    print("int pow neg", 2 ** -1)
