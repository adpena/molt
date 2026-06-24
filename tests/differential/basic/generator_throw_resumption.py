"""Purpose: differential coverage for generator.throw resumption through active handlers."""


def gen_inside_try():
    try:
        x = yield "a"
        yield "after-" + str(x)
    except ValueError as e:
        yield "caught:" + str(e)
    yield "tail"


def case_inside_try():
    g = gen_inside_try()
    print("1.first", next(g))
    print("1.throw", g.throw(ValueError("boom")))
    print("1.next", next(g))
    try:
        next(g)
        print("1.end", "missed")
    except StopIteration:
        print("1.end", "StopIteration")


def gen_simple():
    yield 1
    yield 2


def case_before_first():
    g = gen_simple()
    try:
        g.throw(RuntimeError("early"))
        print("2.throw", "missed")
    except RuntimeError as e:
        print("2.throw", "RuntimeError", str(e))


def gen_outside_then_try():
    yield "x"
    try:
        yield "y"
    except KeyError:
        yield "late-caught"


def case_outside_try():
    g = gen_outside_then_try()
    print("3.first", next(g))
    try:
        g.throw(KeyError("k"))
        print("3.throw", "missed")
    except KeyError as e:
        print("3.throw", "KeyError", str(e))


def gen_reraise():
    try:
        yield "r1"
    except ValueError:
        raise TypeError("reraised")


def case_reraise():
    g = gen_reraise()
    print("4.first", next(g))
    try:
        g.throw(ValueError("orig"))
        print("4.throw", "missed")
    except TypeError as e:
        print("4.throw", "TypeError", str(e))


def gen_finally():
    try:
        yield "f1"
    finally:
        print("5.finally-ran")


def case_finally():
    g = gen_finally()
    print("5.first", next(g))
    try:
        g.throw(IndexError("ix"))
        print("5.throw", "missed")
    except IndexError as e:
        print("5.throw", "IndexError", str(e))


def gen_close():
    try:
        yield "c1"
    finally:
        print("6.finally-ran")


def case_close():
    g = gen_close()
    print("6.first", next(g))
    g.close()
    print("6.closed", "ok")


def gen_capture():
    try:
        a = yield "p1"
        yield "got:" + str(a)
    except ValueError:
        b = yield "handler"
        yield "hgot:" + str(b)


def case_capture():
    g = gen_capture()
    print("7.first", next(g))
    print("7.throw", g.throw(ValueError("v")))
    print("7.send", g.send("S"))
    try:
        next(g)
        print("7.end", "missed")
    except StopIteration:
        print("7.end", "StopIteration")


case_inside_try()
case_before_first()
case_outside_try()
case_reraise()
case_finally()
case_close()
case_capture()
