"""Purpose: differential coverage for yield nested scope."""


def outer():
    def inner(x=(yield "default")):
        return x

    return inner


gen = outer()
print("outer_next", next(gen))
try:
    gen.send("sent")
except StopIteration as exc:
    inner = exc.value
    print("outer_stop", type(inner).__name__, inner.__name__)
