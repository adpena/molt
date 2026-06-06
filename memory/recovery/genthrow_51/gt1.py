def gen_inside_try():
    try:
        x = yield "a"
        yield "after-" + str(x)
    except ValueError as e:
        yield "caught:" + str(e)
    yield "tail"


g = gen_inside_try()
print("first", next(g))
print("throw", g.throw(ValueError("boom")))
print("next", next(g))
