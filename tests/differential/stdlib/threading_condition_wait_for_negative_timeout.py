import threading

cond = threading.Condition()
calls = {"n": 0}


def predicate() -> bool:
    calls["n"] += 1
    return False


with cond:
    out = cond.wait_for(predicate, timeout=-1.0)
print(out)
print(calls["n"])
