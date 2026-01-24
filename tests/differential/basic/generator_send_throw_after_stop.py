"""Purpose: differential coverage for generator send/throw after completion."""


def gen():
    if False:
        yield 1


g = gen()
try:
    next(g)
except Exception as exc:
    print("next", type(exc).__name__)

try:
    g.send(10)
except Exception as exc:
    print("send", type(exc).__name__)

try:
    g.throw(ValueError("boom"))
except Exception as exc:
    print("throw", type(exc).__name__)
