"""Purpose: differential coverage for generator reentrancy guards."""


g = None


def gen():
    print("running_in", g.gi_running)
    yield "start"
    try:
        g.send(None)
    except Exception as exc:
        print("reenter_send", type(exc).__name__, str(exc))
    yield "end"


g = gen()
print("running0", g.gi_running)
print("first", next(g))
print("running1", g.gi_running)
print("second", next(g))
try:
    next(g)
except StopIteration:
    print("closed")
