def header(name: str) -> None:
    print(f"-- {name} --")


header("send")


def gen():
    x = yield 1
    y = yield x
    return y


g = gen()
print(g.send(None))
print(g.send(10))
try:
    g.send(20)
except StopIteration:
    print("stop")


header("throw")


def gen_throw():
    try:
        yield 1
    except ValueError:
        yield 2
    yield 3


g2 = gen_throw()
print(g2.send(None))
exc = None
try:
    raise ValueError("boom")
except ValueError as e:
    exc = e
print(g2.throw(exc))
print(g2.send(None))
try:
    g2.send(None)
except StopIteration:
    print("done")


header("yield_from_send")


def sub_send():
    x = yield 1
    yield x


def gen_yf():
    yield from sub_send()


g3 = gen_yf()
print(g3.send(None))
print(g3.send(7))
try:
    g3.send(None)
except StopIteration:
    print("stop")


header("yield_from_return")


def sub_ret():
    yield 1
    return 9


def gen_ret():
    res = yield from sub_ret()
    print("ret", res)


g4 = gen_ret()
print(g4.send(None))
try:
    g4.send(None)
except StopIteration:
    print("done")


header("close")


events = []


def sub_close():
    try:
        yield 1
    finally:
        events.append("sub_final")


def gen_close():
    try:
        yield from sub_close()
    finally:
        events.append("gen_final")


g5 = gen_close()
print(g5.send(None))
g5.close()
print(events)

header("close_raise")

events = []


def sub_raise():
    try:
        yield 1
    finally:
        events.append("sub_final")
        raise RuntimeError("boom")


def gen_raise():
    try:
        yield from sub_raise()
    finally:
        events.append("gen_final")


g6 = gen_raise()
print(g6.send(None))
try:
    g6.close()
except RuntimeError:
    print("raised")
print(events)

header("throw_finally")


def gen_throw_finally():
    try:
        yield 1
    finally:
        raise RuntimeError("boom")


g7 = gen_throw_finally()
print(g7.send(None))
try:
    g7.throw(ValueError("x"))
except RuntimeError:
    print("raised")

header("context")


def gen_ctx():
    raise RuntimeError("inner")
    yield 1


g8 = gen_ctx()
try:
    raise ValueError("outer")
except ValueError as outer:
    try:
        g8.send(None)
    except RuntimeError as exc:
        print(exc.__context__ is outer)

header("raise_from")


def gen_raise_from():
    try:
        raise ValueError("inner")
    except ValueError as err:
        raise RuntimeError("outer") from err
    yield 1


g9 = gen_raise_from()
try:
    g9.send(None)
except RuntimeError as exc:
    print(exc.__cause__ is None)
    print(exc.__context__ is exc.__cause__)
    print(exc.__suppress_context__)

header("raise_from_none")


def gen_raise_from_none():
    try:
        raise ValueError("inner")
    except ValueError:
        raise RuntimeError("outer") from None
    yield 1


g10 = gen_raise_from_none()
try:
    g10.send(None)
except RuntimeError as exc:
    print(exc.__cause__ is None)
    print(exc.__context__ is None)
    print(exc.__suppress_context__)
