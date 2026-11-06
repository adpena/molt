"""A suspended call keeps successful arguments and preserves exception order."""

class Pause:
    def __await__(self):
        yield "paused"
        return 7


def effect(fail):
    print("effect", fail)
    if fail:
        raise ValueError("argument")
    return 11


async def invoke(fail):
    try:
        return (effect(fail), await Pause())
    except ValueError as error:
        return str(error)
    finally:
        print("finally", fail)


for fail in (False, True):
    iterator = invoke(fail).__await__()
    try:
        print("first", next(iterator))
        print("second", next(iterator))
    except StopIteration as stop:
        print("done", stop.value)


flag = 0
for raise_now in (False, True):
    try:
        if raise_now:
            raise ModuleNotFoundError("x")
    except ModuleNotFoundError:
        flag = True
    else:
        flag = False
    print("flag", flag)


def delegated():
    try:
        sent = yield "ready"
        print("sent", sent)
        yield "again"
    except ValueError as error:
        yield "caught " + str(error)
    finally:
        print("delegated cleanup")
    return "returned"


def delegating():
    result = yield from delegated()
    print("result", result)


for action in ("next", "send", "throw", "close"):
    iterator = delegating()
    print(action, next(iterator))
    try:
        if action == "send":
            print(iterator.send(5))
        elif action == "throw":
            print(iterator.throw(ValueError("injected")))
        elif action == "close":
            print(iterator.close())
        else:
            print(next(iterator))
        print(list(iterator))
    except StopIteration:
        print("finished")
