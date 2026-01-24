"""Purpose: differential coverage for generator.throw signature behavior."""


def gen():
    yield "start"
    yield "middle"


# type only

g1 = gen()
print("case1", next(g1))
try:
    g1.throw(ValueError)
except Exception as exc:
    print("case1_exc", type(exc).__name__, exc.args)


# type + value

g2 = gen()
print("case2", next(g2))
try:
    g2.throw(ValueError, "boom")
except Exception as exc:
    print("case2_exc", type(exc).__name__, exc.args)


# instance

g3 = gen()
print("case3", next(g3))
err = ValueError("zap")
try:
    g3.throw(err)
except Exception as exc:
    print("case3_exc", type(exc).__name__, exc is err, exc.args)
