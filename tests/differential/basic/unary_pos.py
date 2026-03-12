"""Purpose: differential coverage for unary + operator (__pos__)."""


if __name__ == "__main__":
    # int
    print("pos int", +42)
    print("pos neg int", +(-3))
    print("pos zero", +0)

    # float
    print("pos float", +3.14)
    print("pos neg float", +(-2.5))
    print("pos zero float", +0.0)

    # bool (subclass of int)
    print("pos true", +True)
    print("pos false", +False)

    # complex
    print("pos complex", +(1 + 2j))
    print("pos neg complex", +(-3 - 4j))

    # custom class with __pos__
    class Custom:
        def __init__(self, value):
            self.value = value

        def __pos__(self):
            return self.value * 2

        def __repr__(self):
            return f"Custom({self.value})"

    c = Custom(5)
    print("pos custom", +c)

    # custom returning self
    class Identity:
        def __init__(self, n):
            self.n = n

        def __pos__(self):
            return self

        def __repr__(self):
            return f"Identity({self.n})"

    i = Identity(7)
    r = +i
    print("pos identity", r)
    print("pos identity same", r is i)

    # no __pos__ raises TypeError
    class NoPos:
        pass

    try:
        result = +NoPos()
        print("no pos should not reach")
    except TypeError as e:
        print("no pos error", type(e).__name__)

    # __pos__ raising
    class RaisingPos:
        def __pos__(self):
            raise ValueError("pos not allowed")

    try:
        +RaisingPos()
    except ValueError as e:
        print("raising pos", str(e))

    # __pos__ on negative zero float
    import math
    neg_zero = -0.0
    pos_neg_zero = +neg_zero
    print("neg zero pos", pos_neg_zero, math.copysign(1, pos_neg_zero))

    # nested unary +
    print("nested pos", +(+(+42)))
    print("nested pos neg", +(+(-7)))
